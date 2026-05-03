use std::path::PathBuf;

use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("TOML parse error: {0}")]
    Parse(#[from] toml::de::Error),
}

/// Top-level configuration loaded once at startup.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub window: WindowConfig,
    pub theme: ThemeConfig,
    pub font: FontConfig,
    pub keys: KeysConfig,
    pub behavior: BehaviorConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WindowConfig {
    /// Width of the launcher window in logical pixels.
    pub width: u32,
    /// Max height of the results list.
    pub max_height: u32,
    /// Number of result rows visible at once.
    pub max_entries: usize,
    /// Anchor position: "top", "center", "bottom".
    pub anchor: Anchor,
    /// Vertical offset from anchor in pixels.
    pub margin_top: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Anchor {
    Top,
    Center,
    Bottom,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    pub background: Color,
    pub foreground: Color,
    pub selection_bg: Color,
    pub selection_fg: Color,
    pub input_bg: Color,
    pub border: Color,
    pub border_width: u32,
    pub corner_radius: u32,
    pub row_padding: u32,
    pub input_padding: u32,
}

/// RGBA color with components in [0, 255].
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Premultiplied ARGB u32 for direct buffer writes.
    #[inline]
    pub fn to_argb_premul(self) -> u32 {
        let a = self.a as u32;
        let r = (self.r as u32 * a / 255) & 0xFF;
        let g = (self.g as u32 * a / 255) & 0xFF;
        let b = (self.b as u32 * a / 255) & 0xFF;
        (a << 24) | (r << 16) | (g << 8) | b
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FontConfig {
    /// Path to a TTF/OTF font file. If empty, a built-in fallback is used.
    pub path: Option<PathBuf>,
    pub size: f32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct KeysConfig {
    pub confirm: String,
    pub cancel: String,
    pub next: String,
    pub prev: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BehaviorConfig {
    /// Launch with single Enter even if query is empty.
    pub launch_on_empty: bool,
    /// Strip %f/%u field codes from Exec before running.
    pub strip_field_codes: bool,
}

// ── Defaults ────────────────────────────────────────────────────────────────

impl Default for Config {
    fn default() -> Self {
        Self {
            window: WindowConfig::default(),
            theme: ThemeConfig::default(),
            font: FontConfig::default(),
            keys: KeysConfig::default(),
            behavior: BehaviorConfig::default(),
        }
    }
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            width: 640,
            max_height: 480,
            max_entries: 10,
            anchor: Anchor::Center,
            margin_top: 0,
        }
    }
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            background:    Color::new(0x1E, 0x1E, 0x2E, 0xFF),
            foreground:    Color::new(0xCD, 0xD6, 0xF4, 0xFF),
            selection_bg:  Color::new(0x58, 0x5B, 0x70, 0xFF),
            selection_fg:  Color::new(0xCD, 0xD6, 0xF4, 0xFF),
            input_bg:      Color::new(0x31, 0x32, 0x44, 0xFF),
            border:        Color::new(0x89, 0xB4, 0xFA, 0xFF),
            border_width:  1,
            corner_radius: 8,
            row_padding:   6,
            input_padding: 8,
        }
    }
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            path: None,
            size: 16.0,
        }
    }
}

impl Default for KeysConfig {
    fn default() -> Self {
        Self {
            confirm: "Return".into(),
            cancel:  "Escape".into(),
            next:    "Down".into(),
            prev:    "Up".into(),
        }
    }
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            launch_on_empty: false,
            strip_field_codes: true,
        }
    }
}

// ── Loader ───────────────────────────────────────────────────────────────────

impl Config {
    /// Load config from the standard path, falling back to defaults on missing file.
    pub fn load() -> Result<Self, ConfigError> {
        let path = config_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(&path)?;
        let cfg: Self = toml::from_str(&raw)?;
        Ok(cfg)
    }

    #[allow(dead_code)]
    pub fn write_example() -> Result<(), ConfigError> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, EXAMPLE_CONFIG)?;
        Ok(())
    }
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("beelauncher")
        .join("config.toml")
}

// ── Example config ────────────────────────────────────────────────────────────

const EXAMPLE_CONFIG: &str = r#"
[window]
width       = 640
max_entries = 10
anchor      = "center"
margin_top  = 0

[theme]
background   = { r = 30,  g = 30,  b = 46,  a = 255 }
foreground   = { r = 205, g = 214, b = 244, a = 255 }
selection_bg = { r = 88,  g = 91,  b = 112, a = 255 }
selection_fg = { r = 205, g = 214, b = 244, a = 255 }
input_bg     = { r = 49,  g = 50,  b = 68,  a = 255 }
border       = { r = 137, g = 180, b = 250, a = 255 }
border_width  = 1
corner_radius = 8
row_padding   = 6
input_padding = 8

[font]
# path = "/usr/share/fonts/TTF/JetBrainsMono-Regular.ttf"
size = 16.0

[keys]
confirm = "Return"
cancel  = "Escape"
next    = "Down"
prev    = "Up"

[behavior]
launch_on_empty   = false
strip_field_codes = true
"#;
