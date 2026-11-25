use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThemeMode {
    #[default]
    Dark,
    Ember,
    Light,
    Spruce,
    Cobalt,
    Nether,
    Frost,
    Sculk,
    End,
    Cherry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AccentTone {
    #[default]
    Auto,
    Copper,
    Emerald,
    Cyan,
    Amber,
    Crimson,
    Amethyst,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CornerStyle {
    Sharp,
    #[default]
    Balanced,
    Soft,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LayoutMode {
    #[default]
    Sidebar,
    Topbar,
    CompactDeck,
    RightRail,
    MinimalRail,
    DeckMode,
}

/// Validates and normalizes `#RRGGBB` hex color strings to lowercase `#rrggbb`.
pub fn normalize_hex_color(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let hex = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(format!("#{}", hex.to_ascii_lowercase()))
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GraphicsPreset {
    Low,
    Medium,
    High,
    Ultra,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SkinModel {
    #[default]
    Auto,
    Slim,
    Classic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum UiLanguage {
    #[default]
    En,
    Tr,
    De,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LoaderKind {
    #[default]
    Vanilla,
    Fabric,
    Forge,
}

impl LoaderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vanilla => "vanilla",
            Self::Fabric => "fabric",
            Self::Forge => "forge",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JvmGcMode {
    G1GC,
    ZGC,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphicsSettings {
    pub preset: GraphicsPreset,
    pub render_distance: u32,
    pub simulation_distance: u32,
    pub fov_degrees: f32,
    pub vsync: bool,
    pub max_fps: u32,
    pub mouse_sensitivity: f32,
    pub smooth_lighting: bool,
    pub fancy_graphics: bool,
    pub jvm_ram_mb: u32,
    pub jvm_gc: JvmGcMode,
    pub show_fps_hud: bool,
    pub auto_configured: bool,
}

impl Default for GraphicsSettings {
    fn default() -> Self {
        Self {
            preset: GraphicsPreset::High,
            render_distance: 12,
            simulation_distance: 10,
            fov_degrees: 75.0,
            vsync: false,
            max_fps: 260,
            mouse_sensitivity: 0.5,
            smooth_lighting: true,
            fancy_graphics: true,
            jvm_ram_mb: 3072,
            jvm_gc: JvmGcMode::G1GC,
            show_fps_hud: true,
            auto_configured: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedServer {
    pub name: String,
    pub address: String,
}

pub const DEFAULT_MC_VERSION: &str = "1.21.4";

pub const SUPPORTED_MC_VERSIONS: &[&str] = &[
    "1.21.4",
    "1.21.3",
    "1.21.2",
    "1.21.1",
    "1.21",
    "1.20.6",
    "1.20.4",
    "1.20.2",
    "1.20.1",
    "1.20",
    "1.19.4",
    "1.19.2",
    "1.18.2",
    "1.17.1",
    "1.16.5",
];

pub fn normalize_mc_version(raw: &str) -> String {
    let trimmed = raw.trim();
    if SUPPORTED_MC_VERSIONS.contains(&trimmed) {
        return trimmed.to_string();
    }
    DEFAULT_MC_VERSION.to_string()
}

fn default_mc_version() -> String {
    DEFAULT_MC_VERSION.to_string()
}

fn default_legacy_config_version() -> u8 {
    0
}

fn default_true() -> bool {
    true
}

fn default_download_threads() -> u8 {
    24
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    #[serde(default = "default_legacy_config_version")]
    pub config_version: u8,
    #[serde(default = "default_mc_version")]
    pub mc_version: String,
    #[serde(default)]
    pub mod_loader: LoaderKind,
    pub username: String,
    pub skin_file: Option<PathBuf>,
    pub skin_model: SkinModel,
    pub theme: ThemeMode,
    pub ui_accent: AccentTone,
    pub custom_accent_hex: Option<String>,
    pub ui_corner: CornerStyle,
    pub ui_layout: LayoutMode,
    pub ui_compact: bool,
    pub ui_reduced_motion: bool,
    pub ui_sounds: bool,
    pub ui_language: UiLanguage,
    #[serde(default = "default_true")]
    pub discord_rpc: bool,
    #[serde(default = "default_download_threads")]
    pub download_threads: u8,
    #[serde(default = "default_true")]
    pub network_boost: bool,
    #[serde(default = "default_true")]
    pub process_boost: bool,
    pub last_save_id: Option<String>,
    pub saved_servers: Vec<SavedServer>,
    pub graphics: GraphicsSettings,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            config_version: 1,
            mc_version: DEFAULT_MC_VERSION.to_string(),
            mod_loader: LoaderKind::Vanilla,
            username: "Player".to_string(),
            skin_file: None,
            skin_model: SkinModel::Auto,
            theme: ThemeMode::Dark,
            ui_accent: AccentTone::Auto,
            custom_accent_hex: None,
            ui_corner: CornerStyle::Balanced,
            ui_layout: LayoutMode::Sidebar,
            ui_compact: false,
            ui_reduced_motion: false,
            ui_sounds: false,
            ui_language: UiLanguage::En,
            discord_rpc: true,
            download_threads: 24,
            network_boost: true,
            process_boost: true,
            last_save_id: None,
            saved_servers: vec![SavedServer {
                name: "Local Dev Server".to_string(),
                address: "127.0.0.1:25565".to_string(),
            }],
            graphics: GraphicsSettings::default(),
        }
    }
}

pub struct AppPaths {
    pub root_dir: PathBuf,
    pub config_file: PathBuf,
    pub skins_dir: PathBuf,
    pub runtime_dir: PathBuf,
}

impl AppPaths {
    pub fn resolve() -> Self {
        let root_dir = if let Some(proj) = directories::ProjectDirs::from("dev", "Basalt", "Basalt")
        {
            proj.data_local_dir().to_path_buf()
        } else {
            std::env::temp_dir().join("Basalt").join("data")
        };
        let config_file = root_dir.join("config.toml");
        let skins_dir = root_dir.join("skins");
        let runtime_dir = crate::vanilla::resolve_ascii_safe_runtime_dir(&root_dir);
        Self {
            root_dir,
            config_file,
            skins_dir,
            runtime_dir,
        }
    }

    pub fn ensure_dirs(&self) -> Result<(), String> {
        fs::create_dir_all(&self.root_dir)
            .map_err(|e| format!("Failed to create root data directory: {e}"))?;
        fs::create_dir_all(&self.skins_dir)
            .map_err(|e| format!("Failed to create skins directory: {e}"))?;
        fs::create_dir_all(&self.runtime_dir)
            .map_err(|e| format!("Failed to create runtime directory: {e}"))?;
        Ok(())
    }
}

impl AppConfig {
    pub fn sanitize(&mut self) {
        self.mc_version = normalize_mc_version(&self.mc_version);
        self.download_threads = match self.download_threads {
            0..=11 => 8,
            12..=19 => 16,
            20..=28 => 24,
            _ => 32,
        };
        if !self.graphics.fov_degrees.is_finite() {
            self.graphics.fov_degrees = 75.0;
        }
        self.graphics.fov_degrees = self.graphics.fov_degrees.clamp(30.0, 110.0);
        if !self.graphics.mouse_sensitivity.is_finite() {
            self.graphics.mouse_sensitivity = 0.5;
        }
        self.graphics.mouse_sensitivity = self.graphics.mouse_sensitivity.clamp(0.0, 1.0);
        self.graphics.render_distance = self.graphics.render_distance.clamp(2, 32);
        self.graphics.simulation_distance = self.graphics.simulation_distance.clamp(5, 32);
        self.graphics.max_fps = self.graphics.max_fps.clamp(10, 260);
        self.graphics.jvm_ram_mb = self.graphics.jvm_ram_mb.clamp(1024, 16384);
    }

    pub fn load(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(text) => {
                let mut cfg: Self = match toml::from_str(&text) {
                    Ok(parsed) => parsed,
                    Err(_) => {
                        let bak = path.with_extension("toml.bak");
                        let _ = fs::copy(path, &bak);
                        Self::default()
                    }
                };
                cfg.sanitize();
                // Migrate pre-v1 configs so desktop starts windowed & quiet while Console Mode activates sound/fullscreen cleanly.
                if cfg.config_version == 0 {
                    cfg.config_version = 1;
                    cfg.ui_sounds = false;
                    if cfg.ui_layout == LayoutMode::DeckMode {
                        cfg.ui_layout = LayoutMode::Sidebar;
                    }
                    let _ = cfg.save(path);
                }
                cfg
            }
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create settings directory: {e}"))?;
        }
        let body =
            toml::to_string_pretty(self).map_err(|e| format!("Failed to serialize settings: {e}"))?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, &body).map_err(|e| format!("Failed to write temporary settings file: {e}"))?;
        if fs::rename(&tmp, path).is_err() {
            fs::write(path, &body).map_err(|e| format!("Failed to write settings file: {e}"))?;
            let _ = fs::remove_file(&tmp);
        }
        Ok(())
    }
}
