use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpicGame {
    pub app_name: String,
    pub title: String,
    pub install_path: String,
    pub executable: String,
    pub box_art_url: Option<String>,
    pub scream_installed: bool,
    pub koaloader_installed: bool,
    /// True when Koaloader was installed using version.dll as a fallback
    /// because no matching proxy import was detected in the game's PE files.
    pub proxy_fallback_used: bool,
    /// Store identifier. Always "epic" in this fork (Epic-only scope).
    #[serde(default = "default_store")]
    pub store: String,
    /// Where the entry was discovered:
    /// "heroic-native" (Heroic native install) or "legendary-standalone".
    #[serde(default = "default_source")]
    pub source: String,
    /// Platform reported by Legendary (e.g. "Windows"). Heroic/Epic games
    /// on Linux are virtually always Windows builds run via Wine/Proton.
    #[serde(default)]
    pub platform: Option<String>,
    /// Installed version reported by Legendary, if present.
    #[serde(default)]
    pub version: Option<String>,
    /// Path to Heroic's per-game config (GameConfig/<app>.json), if found.
    /// Useful for debugging Wine prefix / runner settings.
    #[serde(default)]
    pub heroic_game_config: Option<String>,
    /// Wine prefix configured in Heroic's GameConfig, if present.
    #[serde(default)]
    pub wine_prefix: Option<String>,
}

fn default_store() -> String {
    "epic".to_string()
}

fn default_source() -> String {
    "heroic-native".to_string()
}

/// Minimal fields we need from installed.json entries.
/// Tolerant: Heroic/Legendary writes vary across versions (missing
/// `executable` for some titles, extra `version`/`platform` fields).
#[derive(Debug, Deserialize)]
struct InstalledEntry {
    #[serde(default)]
    title: Option<String>,
    install_path: String,
    #[serde(default)]
    executable: Option<String>,
    #[serde(default)]
    is_dlc: bool,
    #[serde(default)]
    platform: Option<String>,
    #[serde(default)]
    version: Option<String>,
}

/// One discovered Legendary config location plus its origin label.
#[derive(Debug, Clone)]
struct LegendaryLocation {
    source: &'static str,
    legendary_dir: PathBuf,
    heroic_root: Option<PathBuf>,
}

fn config_base_dirs() -> Vec<PathBuf> {
    let mut bases = Vec::new();

    // Respect XDG_CONFIG_HOME when set (native Heroic follows XDG).
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        let p = PathBuf::from(xdg);
        if p.is_absolute() {
            bases.push(p);
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        bases.push(PathBuf::from(home).join(".config"));
    }

    bases
}

/// Native-only discovery (Epic scope of this fork):
/// 1. Heroic native: `<config>/heroic/legendaryConfig/legendary`
/// 2. Standalone Legendary fallback: `<config>/legendary`
/// Flatpak/Snap paths are intentionally NOT scanned here.
fn legendary_config_dirs() -> Vec<LegendaryLocation> {
    let mut out = Vec::new();

    for base in config_base_dirs() {
        let heroic_legendary = base.join("heroic").join("legendaryConfig").join("legendary");
        if heroic_legendary.is_dir() {
            out.push(LegendaryLocation {
                source: "heroic-native",
                legendary_dir: heroic_legendary,
                heroic_root: Some(base.join("heroic")),
            });
        }

        let standalone = base.join("legendary");
        if standalone.is_dir() {
            // Avoid double-pushing when XDG_CONFIG_HOME == ~/.config
            if !out.iter().any(|l| l.legendary_dir == standalone) {
                out.push(LegendaryLocation {
                    source: "legendary-standalone",
                    legendary_dir: standalone,
                    heroic_root: None,
                });
            }
        }
    }

    if out.is_empty() {
        warn!(
            "No native Heroic/Legendary config found (checked <config>/heroic/legendaryConfig/legendary and <config>/legendary)"
        );
    }

    out
}

/// Normalize the `executable` value stored by Legendary:
/// - converts Windows separators to `/`
/// - trims whitespace / leading `./` / leading `/`
pub fn normalize_executable(raw: &str) -> String {
    let mut s = raw.replace('\\', "/");
    s = s.trim().to_string();
    while let Some(stripped) = s.strip_prefix("./") {
        s = stripped.to_string();
    }
    // Legendary sometimes stores an absolute Windows path (C:/...);
    // keep it as-is but strip a single leading slash for unix-join safety.
    if s.len() > 2 && s.chars().nth(1) == Some(':') {
        return s;
    }
    s.trim_start_matches('/').to_string()
}

/// Try to read Heroic's per-game config to surface the Wine prefix.
/// GameConfig schema varies by Heroic version, so parse tolerantly.
fn heroic_game_info(heroic_root: Option<&Path>, app_name: &str) -> (Option<String>, Option<String>) {
    let root = match heroic_root {
        Some(r) => r,
        None => return (None, None),
    };
    let cfg_path = root.join("GameConfig").join(format!("{}.json", app_name));
    if !cfg_path.is_file() {
        return (None, None);
    }
    let content = match fs::read_to_string(&cfg_path) {
        Ok(c) => c,
        Err(_) => return (Some(cfg_path.to_string_lossy().to_string()), None),
    };
    let val: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return (Some(cfg_path.to_string_lossy().to_string()), None),
    };

    let prefix = ["winePrefix", "wine_prefix", "prefix", "winePrefixPath"]
        .iter()
        .filter_map(|k| val.get(*k).and_then(|v| v.as_str()))
        .next()
        .or_else(|| {
            val.get("wine")
                .and_then(|w| w.get("prefix"))
                .and_then(|v| v.as_str())
        })
        .map(str::to_owned);

    (
        Some(cfg_path.to_string_lossy().to_string()),
        prefix,
    )
}

fn scan_location(loc: &LegendaryLocation, games: &mut Vec<EpicGame>, seen: &mut std::collections::HashSet<String>) {
    let legendary_dir = &loc.legendary_dir;
    let installed_path = legendary_dir.join("installed.json");
    if !installed_path.exists() {
        warn!("installed.json not found at: {}", installed_path.display());
        return;
    }

    let content = match fs::read_to_string(&installed_path) {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to read installed.json: {}", e);
            return;
        }
    };

    let installed: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            warn!("Failed to parse installed.json: {}", e);
            return;
        }
    };

    let metadata_dir = legendary_dir.join("metadata");

    if let Some(obj) = installed.as_object() {
        for (app_name, entry_val) in obj {
            if seen.contains(app_name) {
                continue;
            }

            let entry: InstalledEntry = match serde_json::from_value(entry_val.clone()) {
                Ok(e) => e,
                Err(e) => {
                    warn!("Failed to parse installed entry {}: {}", app_name, e);
                    continue;
                }
            };

            if entry.is_dlc {
                continue;
            }

            let install_path = PathBuf::from(&entry.install_path);
            if !install_path.exists() {
                warn!(
                    "Install path does not exist for {}: {}",
                    app_name, entry.install_path
                );
                continue;
            }

            let title = entry.title.clone().unwrap_or_else(|| app_name.clone());
            let executable = normalize_executable(entry.executable.as_deref().unwrap_or(""));
            let box_art_url = get_box_art(&metadata_dir, app_name);
            let scream_installed = check_screamapi_installed(&install_path);
            let koaloader_installed = check_koaloader_installed(&install_path);
            let (heroic_game_config, wine_prefix) =
                heroic_game_info(loc.heroic_root.as_deref(), app_name);

            info!(
                "Found Epic game [{}]: {} ({}), ScreamAPI={}, Koaloader={}",
                loc.source, title, app_name, scream_installed, koaloader_installed
            );

            seen.insert(app_name.clone());
            games.push(EpicGame {
                app_name: app_name.clone(),
                title,
                install_path: entry.install_path,
                executable,
                box_art_url,
                scream_installed,
                koaloader_installed,
                proxy_fallback_used: false,
                store: "epic".to_string(),
                source: loc.source.to_string(),
                platform: entry.platform,
                version: entry.version,
                heroic_game_config,
                wine_prefix,
            });
        }
    }
}

pub fn scan_epic_games() -> Vec<EpicGame> {
    let locations = legendary_config_dirs();
    if locations.is_empty() {
        return Vec::new();
    }

    let mut games = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for loc in &locations {
        scan_location(loc, &mut games, &mut seen);
    }

    info!("Found {} Epic games (Heroic native)", games.len());
    games
}

/// Extract the "DieselGameBox" image URL from a game's metadata JSON.
/// We read the top-level keyImages array directly from the JSON value,
/// which avoids pulling in DLC images from dlcItemList.
fn get_box_art(metadata_dir: &Path, app_name: &str) -> Option<String> {
    let meta_path = metadata_dir.join(format!("{}.json", app_name));
    if !meta_path.exists() {
        return None;
    }

    let content = fs::read_to_string(&meta_path).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;

    let key_images = val
        .get("metadata")
        .and_then(|m| m.get("keyImages"))
        .and_then(|k| k.as_array())?;

    // Prefer landscape (DieselGameBox), fall back to portrait or logo
    for preferred in &["DieselGameBox", "DieselGameBoxTall", "DieselGameBoxLogo"] {
        if let Some(url) = key_images.iter().find_map(|img| {
            if img.get("type").and_then(|t| t.as_str()) == Some(preferred) {
                img.get("url").and_then(|u| u.as_str()).map(str::to_owned)
            } else {
                None
            }
        }) {
            return Some(url);
        }
    }

    None
}

pub fn check_screamapi_installed(install_path: &Path) -> bool {
    for entry in WalkDir::new(install_path)
        .max_depth(8)
        .into_iter()
        .filter_map(Result::ok)
    {
        let filename = entry.file_name().to_string_lossy().to_lowercase();
        if filename.starts_with("eossdk-win") && filename.ends_with("_o.dll") {
            return true;
        }
    }
    false
}

pub fn check_koaloader_installed(install_path: &Path) -> bool {
    for entry in WalkDir::new(install_path)
        .max_depth(4)
        .into_iter()
        .filter_map(Result::ok)
    {
        if entry.file_name().to_string_lossy() == "Koaloader.config.json" {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_executable_converts_separators() {
        assert_eq!(
            normalize_executable("Game\\Bin\\Game.exe"),
            "Game/Bin/Game.exe"
        );
        assert_eq!(normalize_executable("./Game.exe"), "Game.exe");
        assert_eq!(normalize_executable("/Game.exe"), "Game.exe");
        assert_eq!(normalize_executable(""), "");
    }

    #[test]
    fn installed_entry_tolerates_missing_fields() {
        let v: InstalledEntry = serde_json::from_value(serde_json::json!({
            "install_path": "/tmp/game",
        }))
        .unwrap();
        assert!(v.title.is_none());
        assert!(v.executable.is_none());
        assert!(!v.is_dlc);
    }

    #[test]
    fn scan_location_skips_dlc_and_missing_paths() {
        let dir = tempfile::tempdir().unwrap();
        let legendary = dir.path().join("legendary");
        std::fs::create_dir_all(&legendary).unwrap();
        std::fs::write(
            legendary.join("installed.json"),
            r#"{
                "game1": {"title": "G1", "install_path": "/nope/missing", "executable": "G1.exe"},
                "dlc1": {"title": "D1", "install_path": "/tmp", "executable": "D1.exe", "is_dlc": true}
            }"#,
        )
        .unwrap();

        let loc = LegendaryLocation {
            source: "heroic-native",
            legendary_dir: legendary,
            heroic_root: None,
        };
        let mut games = Vec::new();
        let mut seen = std::collections::HashSet::new();
        scan_location(&loc, &mut games, &mut seen);
        assert!(games.is_empty());
    }

    #[test]
    fn scan_location_dedupes_and_reads_heroic_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let heroic = dir.path().join("heroic");
        let legendary = heroic.join("legendaryConfig").join("legendary");
        std::fs::create_dir_all(legendary.join("metadata")).unwrap();
        std::fs::create_dir_all(heroic.join("GameConfig")).unwrap();
        let game_dir = dir.path().join("game");
        std::fs::create_dir_all(&game_dir).unwrap();
        std::fs::write(
            legendary.join("installed.json"),
            serde_json::json!({
                "abc123": {
                    "title": "Test Game",
                    "install_path": game_dir.to_string_lossy(),
                    "executable": "Bin\\Test.exe",
                    "platform": "Windows",
                    "version": "1.0"
                }
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            heroic.join("GameConfig").join("abc123.json"),
            r#"{"winePrefix": "/tmp/prefix"}"#,
        )
        .unwrap();

        let loc = LegendaryLocation {
            source: "heroic-native",
            legendary_dir: legendary,
            heroic_root: Some(heroic),
        };
        let mut games = Vec::new();
        let mut seen = std::collections::HashSet::new();
        scan_location(&loc, &mut games, &mut seen);
        // second scan of same app must not duplicate
        scan_location(&loc, &mut games, &mut seen);
        assert_eq!(games.len(), 1);
        assert_eq!(games[0].executable, "Bin/Test.exe");
        assert_eq!(games[0].source, "heroic-native");
        assert_eq!(games[0].wine_prefix.as_deref(), Some("/tmp/prefix"));
    }
}