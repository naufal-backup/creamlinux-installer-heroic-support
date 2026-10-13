/**
 * Epic game discovered via Heroic/Legendary (native Heroic focus, Epic-only)
 */
export interface EpicGame {
  app_name: string
  title: string
  install_path: string
  executable: string
  box_art_url: string | null
  scream_installed: boolean
  koaloader_installed: boolean
  proxy_fallback_used: boolean
  /** Always "epic" in this fork. Optional for backward-compat with cache. */
  store?: string
  /** "heroic-native" | "legendary-standalone". Optional for backward-compat. */
  source?: string
  platform?: string | null
  version?: string | null
  heroic_game_config?: string | null
  wine_prefix?: string | null
}