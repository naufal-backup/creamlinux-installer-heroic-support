use super::Unlocker;
use async_trait::async_trait;
use log::info;
use reqwest;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::tempdir;
use walkdir::WalkDir;
use zip::ZipArchive;

const SCREAMAPI_REPO: &str = "acidicoala/ScreamAPI";

pub struct ScreamAPI;

#[async_trait]
impl Unlocker for ScreamAPI {
    async fn get_latest_version() -> Result<String, String> {
        info!("Fetching latest ScreamAPI version...");

        let client = reqwest::Client::new();
        let releases_url = format!(
            "https://api.github.com/repos/{}/releases/latest",
            SCREAMAPI_REPO
        );

        let response = client
            .get(&releases_url)
            .header("User-Agent", "CreamLinux")
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| format!("Failed to fetch ScreamAPI releases: {}", e))?;

        if !response.status().is_success() {
            return Err(format!(
                "Failed to fetch ScreamAPI releases: HTTP {}",
                response.status()
            ));
        }

        let release_info: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse release info: {}", e))?;

        let version = release_info
            .get("tag_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Failed to extract version from release info".to_string())?
            .to_string();

        info!("Latest ScreamAPI version: {}", version);
        Ok(version)
    }

    async fn download_to_cache() -> Result<String, String> {
        // Manual override: the upstream ScreamAPI GitHub repo is ToS-blocked,
        // so api.github.com answers 403 for it. Users can point the app at a
        // self-obtained release zip instead:
        //   SCREAMAPI_ZIP_PATH=/path/to/ScreamAPI-vX.Y.Z.zip  (local file)
        //   SCREAMAPI_ZIP_URL=https://.../ScreamAPI-vX.Y.Z.zip (direct URL)
        //   SCREAMAPI_VERSION=vX.Y.Z  (optional, else derived from file name)
        if let Some((version, zip_path)) = Self::manual_zip_source().await? {
            let version_dir = crate::cache::get_screamapi_version_dir(&version)?;
            Self::extract_dlls(&zip_path, &version_dir)?;
            info!(
                "ScreamAPI version {} seeded from manual source successfully",
                version
            );
            return Ok(version);
        }

        let version = Self::get_latest_version().await.map_err(|e| {
            format!(
                "{}. NOTE: the upstream acidicoala/ScreamAPI repo is currently ToS-blocked on GitHub (API returns 403); set SCREAMAPI_ZIP_PATH or SCREAMAPI_ZIP_URL to a self-obtained release zip instead",
                e
            )
        })?;
        info!("Downloading ScreamAPI version {}...", version);

        let client = reqwest::Client::new();

        let releases_url = format!(
            "https://api.github.com/repos/{}/releases/latest",
            SCREAMAPI_REPO
        );
        let release_info: serde_json::Value = client
            .get(&releases_url)
            .header("User-Agent", "CreamLinux")
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| format!("Failed to fetch ScreamAPI release: {}", e))?
            .json()
            .await
            .map_err(|e| format!("Failed to parse release info: {}", e))?;

        let zip_url = release_info
            .get("assets")
            .and_then(|a| a.as_array())
            .and_then(|assets| {
                assets.iter().find(|asset| {
                    asset
                        .get("name")
                        .and_then(|n| n.as_str())
                        .map(|n| n.ends_with(".zip"))
                        .unwrap_or(false)
                })
            })
            .and_then(|asset| asset.get("browser_download_url"))
            .and_then(|u| u.as_str())
            .ok_or_else(|| "No zip asset found in ScreamAPI release".to_string())?
            .to_string();

        info!("Downloading ScreamAPI from: {}", zip_url);

        let response = client
            .get(&zip_url)
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .map_err(|e| format!("Failed to download ScreamAPI: {}", e))?;

        if !response.status().is_success() {
            return Err(format!(
                "Failed to download ScreamAPI: HTTP {}",
                response.status()
            ));
        }

        let temp_dir = tempdir().map_err(|e| format!("Failed to create temp dir: {}", e))?;
        let zip_path = temp_dir.path().join("screamapi.zip");
        let content = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read response bytes: {}", e))?;
        fs::write(&zip_path, &content)
            .map_err(|e| format!("Failed to write zip file: {}", e))?;

        let version_dir = crate::cache::get_screamapi_version_dir(&version)?;
        Self::extract_dlls(&zip_path, &version_dir)?;

        info!("ScreamAPI version {} downloaded to cache successfully", version);
        Ok(version)
    }


    /// context = "" -> direct install (replace EOSSDK DLLs)
    /// context = "koaloader" -> payload install (drop DLL in exe dir)
    async fn install_to_game(game_path: &str, context: &str) -> Result<(), String> {
        if context == "koaloader" {
            Self::install_as_koaloader_payload(game_path).await
        } else {
            Self::install_direct(game_path).await
        }
    }

    async fn uninstall_from_game(game_path: &str, context: &str) -> Result<(), String> {
        if context == "koaloader" {
            Self::uninstall_as_koaloader_payload(game_path).await
        } else {
            Self::uninstall_direct(game_path).await
        }
    }

    fn name() -> &'static str {
        "ScreamAPI"
    }
}

impl ScreamAPI {
    // Direct install

    /// Extract ScreamAPI DLLs (+ default config) from a release zip.
    /// Shared by the GitHub flow and the manual SCREAMAPI_ZIP_* override.
    fn extract_dlls(zip_path: &Path, version_dir: &Path) -> Result<(), String> {
        let file =
            fs::File::open(zip_path).map_err(|e| format!("Failed to open zip: {}", e))?;
        let mut archive =
            ZipArchive::new(file).map_err(|e| format!("Failed to read zip archive: {}", e))?;

        for i in 0..archive.len() {
            let mut file = archive
                .by_index(i)
                .map_err(|e| format!("Failed to access zip entry: {}", e))?;

            let file_name = file.name().to_string();
            let base_name = Path::new(&file_name)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            let should_extract = base_name.to_lowercase().ends_with(".dll")
                || base_name == "ScreamAPI.config.json";

            if should_extract {
                let output_path = version_dir.join(&base_name);
                let mut outfile = fs::File::create(&output_path)
                    .map_err(|e| format!("Failed to create output file: {}", e))?;
                io::copy(&mut file, &mut outfile)
                    .map_err(|e| format!("Failed to extract file: {}", e))?;
                info!("Extracted: {}", output_path.display());
            }
        }

        // Sanity check: the cache validator requires both bitness DLLs.
        for required in &["ScreamAPI32.dll", "ScreamAPI64.dll"] {
            if !version_dir.join(required).exists() {
                return Err(format!(
                    "Zip did not contain required file {} (is this a ScreamAPI release zip?)",
                    required
                ));
            }
        }

        Ok(())
    }

    /// Resolve a manual ScreamAPI zip source from the environment.
    /// Returns (version, local zip path) or None when no override is set.
    /// A temp copy is made for URL sources; for local paths the file is used
    /// directly. The temp dir is deliberately leaked so the zip outlives the
    /// download (it is tiny and lives in $TMPDIR).
    async fn manual_zip_source() -> Result<Option<(String, PathBuf)>, String> {
        let local = std::env::var("SCREAMAPI_ZIP_PATH")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let url = std::env::var("SCREAMAPI_ZIP_URL")
            .ok()
            .filter(|s| !s.trim().is_empty());

        if local.is_none() && url.is_none() {
            return Ok(None);
        }

        if let Some(path) = local {
            let zip_path = PathBuf::from(&path);
            if !zip_path.is_file() {
                return Err(format!(
                    "SCREAMAPI_ZIP_PATH does not exist: {}",
                    zip_path.display()
                ));
            }
            let version = Self::manual_version(Some(&zip_path));
            info!("Using manual ScreamAPI zip: {}", zip_path.display());
            return Ok(Some((version, zip_path)));
        }

        let url = url.expect("checked above");
        info!("Downloading manual ScreamAPI zip from: {}", url);
        let response = reqwest::Client::new()
            .get(&url)
            .timeout(Duration::from_secs(120))
            .send()
            .await
            .map_err(|e| format!("Failed to download SCREAMAPI_ZIP_URL: {}", e))?;
        if !response.status().is_success() {
            return Err(format!(
                "Failed to download SCREAMAPI_ZIP_URL: HTTP {}",
                response.status()
            ));
        }
        let content = response
            .bytes()
            .await
            .map_err(|e| format!("Failed to read response bytes: {}", e))?;

        let file_name = url.rsplit('/').next().unwrap_or("screamapi.zip");
        let temp_dir = tempdir().map_err(|e| format!("Failed to create temp dir: {}", e))?;
        let zip_path = temp_dir.path().join(file_name);
        fs::write(&zip_path, &content)
            .map_err(|e| format!("Failed to write zip file: {}", e))?;
        // Keep the temp dir alive past this function (see doc comment).
        let _ = temp_dir.keep();

        let version = Self::manual_version(Some(&zip_path));
        Ok(Some((version, zip_path)))
    }

    /// Version label for a manual zip: explicit SCREAMAPI_VERSION env wins,
    /// else a `vX.Y.Z` infix is parsed from the file name, else "manual".
    fn manual_version(zip_path: Option<&Path>) -> String {
        if let Ok(v) = std::env::var("SCREAMAPI_VERSION") {
            if !v.trim().is_empty() {
                return v.trim().to_string();
            }
        }
        if let Some(p) = zip_path {
            if let Some(v) = Self::version_from_filename(p) {
                return v;
            }
        }
        "manual".to_string()
    }

    /// Parse e.g. "ScreamAPI-v4.0.0.zip" -> "v4.0.0".
    fn version_from_filename(path: &Path) -> Option<String> {
        let name = path.file_name()?.to_string_lossy().to_lowercase();
        // find a 'v' followed by digit.digit
        let bytes = name.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'v' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
                let mut j = i + 1;
                while j < bytes.len()
                    && (bytes[j].is_ascii_digit() || bytes[j] == b'.')
                {
                    j += 1;
                }
                let candidate = name[i..j].trim_matches('.');
                if candidate.contains('.') {
                    return Some(candidate.to_string());
                }
            }
            i += 1;
        }
        None
    }

    async fn install_direct(game_path: &str) -> Result<(), String> {
        info!("Installing ScreamAPI (direct) to: {}", game_path);

        let install_path = Path::new(game_path);
        let eos_dlls = Self::find_eossdk_dlls(install_path);

        if eos_dlls.is_empty() {
            return Err(format!(
                "No EOSSDK-Win*-Shipping.dll found in {}",
                game_path
            ));
        }

        info!("Found {} EOSSDK DLL(s)", eos_dlls.len());

        let versions = crate::cache::read_versions()?;
        if versions.screamapi.latest.is_empty() {
            return Err("ScreamAPI is not cached. Please restart the app.".to_string());
        }
        let scream_dir = crate::cache::get_screamapi_version_dir(&versions.screamapi.latest)?;

        for eos_dll in &eos_dlls {
            let filename = eos_dll.file_name().unwrap_or_default().to_string_lossy();
            let is_64bit = filename.to_lowercase().contains("64");

            let stem = filename.trim_end_matches(".dll");
            let backup = eos_dll.with_file_name(format!("{}_o.dll", stem));

            if !backup.exists() && eos_dll.exists() {
                fs::copy(eos_dll, &backup)
                    .map_err(|e| format!("Failed to backup {}: {}", filename, e))?;
                info!("Backed up {} -> {}", eos_dll.display(), backup.display());
            }

            let scream_dll_name = if is_64bit { "ScreamAPI64.dll" } else { "ScreamAPI32.dll" };
            let src = scream_dir.join(scream_dll_name);
            if !src.exists() {
                return Err(format!("ScreamAPI DLL not found in cache: {}", src.display()));
            }

            fs::copy(&src, eos_dll)
                .map_err(|e| format!("Failed to install ScreamAPI DLL: {}", e))?;
            info!("Installed {} as {}", scream_dll_name, eos_dll.display());
        }

        let config_dir = eos_dlls[0].parent().ok_or("Failed to get parent of EOS DLL")?;
        crate::screamapi_config::write_default_config(config_dir)?;

        info!("ScreamAPI (direct) installation complete for: {}", game_path);
        Ok(())
    }

    async fn uninstall_direct(game_path: &str) -> Result<(), String> {
        info!("Uninstalling ScreamAPI (direct) from: {}", game_path);

        let install_path = Path::new(game_path);

        for entry in WalkDir::new(install_path)
            .max_depth(8)
            .into_iter()
            .filter_map(Result::ok)
        {
            let path = entry.path();
            let filename = path.file_name().unwrap_or_default().to_string_lossy();
            let lower = filename.to_lowercase();

            if lower.starts_with("eossdk-win") && lower.ends_with("_o.dll") {
                let original_name = filename.trim_end_matches("_o.dll").to_string() + ".dll";
                let original = path.parent().unwrap_or(install_path).join(&original_name);

                fs::copy(path, &original)
                    .map_err(|e| format!("Failed to restore {}: {}", original_name, e))?;
                fs::remove_file(path)
                    .map_err(|e| format!("Failed to remove backup file: {}", e))?;
                info!("Restored {} from backup", original.display());
            }
        }

        crate::screamapi_config::delete_config(game_path)?;
        info!("ScreamAPI (direct) uninstallation complete for: {}", game_path);
        Ok(())
    }

    // Koaloader payload

    async fn install_as_koaloader_payload(exe_dir: &str) -> Result<(), String> {
        info!("Installing ScreamAPI as Koaloader payload in: {}", exe_dir);

        let versions = crate::cache::read_versions()?;
        if versions.screamapi.latest.is_empty() {
            return Err("ScreamAPI is not cached. Please restart the app.".to_string());
        }
        let scream_dir = crate::cache::get_screamapi_version_dir(&versions.screamapi.latest)?;
        let exe_dir_path = Path::new(exe_dir);

        for dll_name in &["ScreamAPI32.dll", "ScreamAPI64.dll"] {
            let src = scream_dir.join(dll_name);
            if src.exists() {
                let dest = exe_dir_path.join(dll_name);
                fs::copy(&src, &dest)
                    .map_err(|e| format!("Failed to copy {}: {}", dll_name, e))?;
                info!("Placed {} in exe dir", dll_name);
            }
        }

        crate::screamapi_config::write_default_config(exe_dir_path)?;
        info!("ScreamAPI (Koaloader payload) install complete");
        Ok(())
    }

    async fn uninstall_as_koaloader_payload(exe_dir: &str) -> Result<(), String> {
        info!("Removing ScreamAPI Koaloader payload from: {}", exe_dir);

        let exe_dir_path = Path::new(exe_dir);
        for dll_name in &["ScreamAPI32.dll", "ScreamAPI64.dll"] {
            let path = exe_dir_path.join(dll_name);
            if path.exists() {
                fs::remove_file(&path)
                    .map_err(|e| format!("Failed to remove {}: {}", dll_name, e))?;
                info!("Removed {}", dll_name);
            }
        }

        let cfg = exe_dir_path.join("ScreamAPI.config.json");
        if cfg.exists() {
            fs::remove_file(&cfg).ok();
        }

        info!("ScreamAPI (Koaloader payload) uninstall complete");
        Ok(())
    }

    // Helpers

    pub fn find_eossdk_dlls(root: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for entry in WalkDir::new(root)
            .max_depth(8)
            .into_iter()
            .filter_map(Result::ok)
        {
            let path = entry.path();
            let lower = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();

            if lower.starts_with("eossdk-win")
                && lower.ends_with("-shipping.dll")
                && !lower.contains("_o")
            {
                found.push(path.to_path_buf());
            }
        }
        found
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn version_from_filename_parses_tag() {
        assert_eq!(
            ScreamAPI::version_from_filename(Path::new("ScreamAPI-v4.0.0.zip")),
            Some("v4.0.0".to_string())
        );
        assert_eq!(
            ScreamAPI::version_from_filename(Path::new("/tmp/ScreamAPI-v3.2.1.zip")),
            Some("v3.2.1".to_string())
        );
        assert_eq!(
            ScreamAPI::version_from_filename(Path::new("screamapi.zip")),
            None
        );
    }

    #[test]
    fn manual_version_prefers_env() {
        // SAFETY: tests run single-threaded per binary by default only with
        // --test-threads=1; env mutation here is scoped and restored.
        std::env::set_var("SCREAMAPI_VERSION", "v9.9.9");
        let v = ScreamAPI::manual_version(Some(Path::new("ScreamAPI-v4.0.0.zip")));
        std::env::remove_var("SCREAMAPI_VERSION");
        assert_eq!(v, "v9.9.9");
    }

    #[test]
    fn manual_version_falls_back_to_filename_then_manual() {
        std::env::remove_var("SCREAMAPI_VERSION");
        assert_eq!(
            ScreamAPI::manual_version(Some(Path::new("ScreamAPI-v4.0.0.zip"))),
            "v4.0.0"
        );
        assert_eq!(
            ScreamAPI::manual_version(Some(Path::new("random.zip"))),
            "manual"
        );
        assert_eq!(ScreamAPI::manual_version(None), "manual");
    }

    #[test]
    fn extract_dlls_accepts_release_zip_and_rejects_junk() {        let dir = tempfile::tempdir().unwrap();

        // Build a fake release zip with both DLLs + noise.
        let zip_path = dir.path().join("ScreamAPI-v4.0.0.zip");
        {
            let f = fs::File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(f);
            let opts =
                zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
            zip.start_file("ScreamAPI32.dll", opts).unwrap();
            zip.write_all(b"dll32").unwrap();
            zip.start_file("nested/ScreamAPI64.dll", opts).unwrap();
            zip.write_all(b"dll64").unwrap();
            zip.start_file("readme.txt", opts).unwrap();
            zip.write_all(b"noise").unwrap();
            zip.finish().unwrap();
        }

        let out = dir.path().join("out");
        fs::create_dir_all(&out).unwrap();
        ScreamAPI::extract_dlls(&zip_path, &out).unwrap();
        assert!(out.join("ScreamAPI32.dll").exists());
        assert!(out.join("ScreamAPI64.dll").exists());
        assert!(!out.join("readme.txt").exists());

        // Junk zip missing the 64-bit DLL must be rejected.
        let bad = dir.path().join("bad.zip");
        {
            let f = fs::File::create(&bad).unwrap();
            let mut zip = zip::ZipWriter::new(f);
            let opts =
                zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
            zip.start_file("ScreamAPI32.dll", opts).unwrap();
            zip.write_all(b"dll32").unwrap();
            zip.finish().unwrap();
        }
        let out2 = dir.path().join("out2");
        fs::create_dir_all(&out2).unwrap();
        assert!(ScreamAPI::extract_dlls(&bad, &out2).is_err());
    }

    /// Serialize SCREAMAPI_* env access across tests (env is process-global).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn write_fake_zip(path: &Path) {
        let f = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(f);
        let opts =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("ScreamAPI32.dll", opts).unwrap();
        zip.write_all(b"dll32").unwrap();
        zip.start_file("ScreamAPI64.dll", opts).unwrap();
        zip.write_all(b"dll64").unwrap();
        zip.finish().unwrap();
    }

    #[tokio::test]
    async fn manual_zip_source_none_when_unset() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::remove_var("SCREAMAPI_ZIP_PATH");
        std::env::remove_var("SCREAMAPI_ZIP_URL");
        let res = ScreamAPI::manual_zip_source().await.unwrap();
        assert!(res.is_none());
    }

    #[tokio::test]
    async fn manual_zip_source_local_path_ok() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("ScreamAPI-v4.0.0.zip");
        write_fake_zip(&zip_path);
        std::env::set_var("SCREAMAPI_ZIP_PATH", &zip_path);
        std::env::remove_var("SCREAMAPI_ZIP_URL");
        std::env::remove_var("SCREAMAPI_VERSION");
        let (version, path) = ScreamAPI::manual_zip_source()
            .await
            .unwrap()
            .expect("should resolve local zip");
        assert_eq!(version, "v4.0.0");
        assert_eq!(path, zip_path);
        std::env::remove_var("SCREAMAPI_ZIP_PATH");
    }

    #[tokio::test]
    async fn manual_zip_source_missing_path_errors() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("SCREAMAPI_ZIP_PATH", "/nope/missing.zip");
        std::env::remove_var("SCREAMAPI_ZIP_URL");
        let err = ScreamAPI::manual_zip_source().await.unwrap_err();
        assert!(err.contains("does not exist"), "unexpected: {}", err);
        std::env::remove_var("SCREAMAPI_ZIP_PATH");
    }
}
