//! Where the (external, never-copied-into-the-repo) game data lives.

use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const DEFAULT_MAP: &str = "mp_rr_canyonlands_staging_mu1";
pub const DEFAULT_ASSETS: &str = r"D:\R5Rust\assets";

/// Settings shared by the client and the dedicated server (`r5rust.toml`).
#[derive(Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    pub assets_root: Option<String>,
    pub map: String,
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
    pub vsync: bool,
    pub max_texture_size: u32,
    pub sensitivity: f32,
    pub fov: f32,
    pub fly_speed: f32,
    pub backend: String,
    pub port: u16,
    /// linear exposure before the tonemap
    pub exposure: f32,
    /// auto-exposure target: average scene luminance is scaled to this
    pub exposure_key: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            assets_root: None,
            map: DEFAULT_MAP.into(),
            width: 1600,
            height: 900,
            fullscreen: false,
            vsync: false,
            max_texture_size: 1024,
            sensitivity: 1.6,
            fov: 100.0,
            fly_speed: 1200.0,
            backend: "vulkan".into(),
            port: 37015,
            exposure: 1.0,
            exposure_key: 0.18,
        }
    }
}

impl Config {
    /// Reads `r5rust.toml` from the working directory or next to the exe. Missing file = defaults.
    pub fn load() -> Self {
        let mut cands = vec![PathBuf::from("r5rust.toml")];
        if let Ok(exe) = std::env::current_exe() {
            if let Some(d) = exe.parent() {
                cands.push(d.join("r5rust.toml"));
                cands.push(d.join("../../r5rust.toml"));
            }
        }
        for c in cands {
            if let Ok(s) = std::fs::read_to_string(&c) {
                match toml::from_str::<Config>(&s) {
                    Ok(cfg) => {
                        log::info!("config: {}", c.display());
                        return cfg;
                    }
                    Err(e) => log::warn!("bad config {}: {e}", c.display()),
                }
            }
        }
        Config::default()
    }

    /// env R5RUST_ASSETS > config assets_root > default.
    pub fn assets_root(&self) -> PathBuf {
        if let Ok(v) = std::env::var("R5RUST_ASSETS") {
            if !v.is_empty() {
                return PathBuf::from(v);
            }
        }
        if let Some(v) = &self.assets_root {
            return PathBuf::from(v);
        }
        PathBuf::from(DEFAULT_ASSETS)
    }

    pub fn map_dir(&self) -> PathBuf {
        self.assets_root().join("maps").join(&self.map)
    }
    pub fn world_glb(&self) -> PathBuf {
        self.map_dir().join("world.glb")
    }
    pub fn meta_json(&self) -> PathBuf {
        self.map_dir().join("meta.json")
    }

    /// First PNG in images/ui/maps_<map>_loadscreen/ (the owner's extracted loading screen).
    pub fn loadscreen(&self) -> Option<PathBuf> {
        let d = self
            .assets_root()
            .join("images")
            .join("ui")
            .join(format!("maps_{}_loadscreen", self.map));
        first_png(&d)
    }
}

fn first_png(dir: &Path) -> Option<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x.eq_ignore_ascii_case("png")).unwrap_or(false))
        .collect();
    v.sort();
    v.into_iter().next()
}

impl Config {
    /// Raw lighting lumps copied from the map's BSP (`bsp/<lump hex>.bin`).
    pub fn bsp_dir(&self) -> PathBuf {
        self.map_dir().join("bsp")
    }
}
