//! Video settings (the in-game Video tab, the launcher's Graphics card, `[graphics]` in r5rust_settings.toml).
//! Presets fill every knob; touching a knob makes the preset Custom. Everything applies live except
//! `texture_quality` (read when the map loads). No shading options on purpose: lighting, reflections, bloom,
//! exposure and fog always render like Apex (Kainz, 2026-11-18). Model Detail = Apex's r_lod_switch_scale (props.rs).

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    Lowest,
    Low,
    Medium,
    High,
    Ultra,
    Custom,
}

impl Preset {
    pub const FIXED: [Preset; 5] = [Preset::Lowest, Preset::Low, Preset::Medium, Preset::High, Preset::Ultra];
    pub fn label(self) -> &'static str {
        match self {
            Preset::Lowest => "Lowest",
            Preset::Low => "Low",
            Preset::Medium => "Medium",
            Preset::High => "High",
            Preset::Ultra => "Ultra",
            Preset::Custom => "Custom",
        }
    }
    pub fn parse(s: &str) -> Option<Preset> {
        Preset::FIXED.into_iter().chain([Preset::Custom]).find(|p| p.label().eq_ignore_ascii_case(s))
    }
}

/// Texture detail: drop the top mips of every material texture at load ("picmip"), never below a floor size.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextureQuality {
    Lowest,
    Low,
    Medium,
    High,
    Ultra,
}

impl TextureQuality {
    pub const ALL: [TextureQuality; 5] = [TextureQuality::Lowest, TextureQuality::Low, TextureQuality::Medium, TextureQuality::High, TextureQuality::Ultra];
    pub fn label(self) -> &'static str {
        match self {
            TextureQuality::Lowest => "Lowest",
            TextureQuality::Low => "Low",
            TextureQuality::Medium => "Medium",
            TextureQuality::High => "High",
            TextureQuality::Ultra => "Ultra",
        }
    }
    /// (mips to skip, smallest longest side a texture is cut down to)
    pub fn picmip(self) -> (u32, u32) {
        match self {
            TextureQuality::Ultra => (0, 0),
            TextureQuality::High => (1, 512),
            TextureQuality::Medium => (1, 256),
            TextureQuality::Low => (2, 128),
            TextureQuality::Lowest => (3, 64),
        }
    }
}

/// Apex's Model Detail: the static-prop LOD switch scale (`r_lod_switch_scale`, read off Kainz's retail
/// videoconfig.txt: 0.6 on Low). Lower = props drop to their lower LODs closer to you.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelDetail {
    Low,
    Medium,
    High,
}

impl ModelDetail {
    pub const ALL: [ModelDetail; 3] = [ModelDetail::Low, ModelDetail::Medium, ModelDetail::High];
    pub fn label(self) -> &'static str {
        match self {
            ModelDetail::Low => "Low",
            ModelDetail::Medium => "Medium",
            ModelDetail::High => "High",
        }
    }
    /// r_lod_switch_scale
    pub fn lod_scale(self) -> f32 {
        match self {
            ModelDetail::Low => 0.6,
            ModelDetail::Medium => 0.8,
            ModelDetail::High => 1.0,
        }
    }
}

/// Real-time sun shadows. The baked sun visibility is always there; cascades add sharp moving-free detail near you.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shadows {
    Off,
    Low,
    High,
}

impl Shadows {
    pub const ALL: [Shadows; 3] = [Shadows::Off, Shadows::Low, Shadows::High];
    pub fn label(self) -> &'static str {
        match self {
            Shadows::Off => "Off (baked only)",
            Shadows::Low => "Low",
            Shadows::High => "High",
        }
    }
    /// (shadow map resolution, cascades)
    pub fn cascades(self) -> (u32, usize) {
        match self {
            Shadows::Off => (1024, 0),
            Shadows::Low => (1024, 3),
            Shadows::High => (2048, 4),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GraphicsSettings {
    pub preset: Preset,
    /// restart (map reload) to apply
    pub texture_quality: TextureQuality,
    /// live
    pub model_detail: ModelDetail,
    /// 1, 2, 4, 8 or 16
    pub anisotropic: u32,
    pub sun_shadows: Shadows,
    /// 1 = off, 4 = 4x MSAA
    pub msaa: u32,
    /// percent of the window resolution the world renders at (50..=100)
    pub render_scale: u32,
    /// stretched resolution like Apex's (e.g. 1300x1070): the game and HUD render at this size and are stretched to
    /// the window; [0, 0] = off (the window's own resolution)
    pub stretch: [u32; 2],
    pub vsync: bool,
    /// 0 = unlimited
    pub fps_cap: u32,
}

impl Default for GraphicsSettings {
    fn default() -> Self {
        GraphicsSettings::preset(Preset::High)
    }
}

impl GraphicsSettings {
    pub fn preset(p: Preset) -> Self {
        // Ultra = the full 1:1 look; shading is always Apex's own, presets only trade detail and resolution
        let (tex, model, aniso, shadows, msaa) = match p {
            Preset::Lowest => (TextureQuality::Lowest, ModelDetail::Low, 1, Shadows::Off, 1),
            Preset::Low => (TextureQuality::Low, ModelDetail::Low, 2, Shadows::Off, 1),
            Preset::Medium => (TextureQuality::Medium, ModelDetail::Medium, 4, Shadows::Low, 1),
            Preset::High => (TextureQuality::High, ModelDetail::High, 8, Shadows::High, 4),
            Preset::Ultra | Preset::Custom => (TextureQuality::Ultra, ModelDetail::High, 16, Shadows::High, 4),
        };
        GraphicsSettings {
            preset: p,
            texture_quality: tex,
            model_detail: model,
            anisotropic: aniso,
            sun_shadows: shadows,
            msaa,
            render_scale: 100,
            stretch: [0, 0],
            vsync: false,
            fps_cap: 0,
        }
    }

    /// Switches to `p`, keeping the display options (vsync, fps cap, render scale) that no preset sets.
    pub fn apply_preset(&mut self, p: Preset) {
        let (vsync, cap, scale, stretch) = (self.vsync, self.fps_cap, self.render_scale, self.stretch);
        *self = GraphicsSettings::preset(p);
        self.stretch = stretch;
        self.vsync = vsync;
        self.fps_cap = cap;
        self.render_scale = scale;
    }

    /// The preset these knobs match, else Custom.
    pub fn detect_preset(&self) -> Preset {
        Preset::FIXED
            .into_iter()
            .find(|&p| {
                let mut g = GraphicsSettings::preset(p);
                g.vsync = self.vsync;
                g.fps_cap = self.fps_cap;
                g.render_scale = self.render_scale;
                g.stretch = self.stretch;
                g == GraphicsSettings { preset: p, ..self.clone() }
            })
            .unwrap_or(Preset::Custom)
    }

    /// Values a hand-edited file may have out of range, pulled back in.
    pub fn sanitised(mut self) -> Self {
        self.msaa = if self.msaa >= 4 { 4 } else { 1 };
        self.anisotropic = [1, 2, 4, 8, 16].into_iter().rev().find(|&a| a <= self.anisotropic.max(1)).unwrap_or(1);
        self.render_scale = self.render_scale.clamp(50, 100);
        if self.stretch[0] < 320 || self.stretch[1] < 240 {
            self.stretch = [0, 0];
        } else {
            self.stretch = [self.stretch[0].min(7680), self.stretch[1].min(4320)];
        }
        if self.preset != Preset::Custom {
            self.preset = self.detect_preset();
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_round_trip() {
        for p in Preset::FIXED {
            assert_eq!(GraphicsSettings::preset(p).detect_preset(), p);
        }
        let mut g = GraphicsSettings::preset(Preset::Medium);
        g.anisotropic = 16;
        assert_eq!(g.detect_preset(), Preset::Custom);
        g.anisotropic = 4;
        g.vsync = true;
        g.render_scale = 70;
        assert_eq!(g.detect_preset(), Preset::Medium);
    }

    #[test]
    fn toml_names() {
        let g: GraphicsSettings = toml::from_str("preset = \"low\"\nsun_shadows = \"off\"\ntexture_quality = \"lowest\"\nmsaa = 3\n").unwrap();
        assert_eq!((g.preset, g.sun_shadows, g.texture_quality), (Preset::Low, Shadows::Off, TextureQuality::Lowest));
        assert_eq!(g.sanitised().msaa, 1);
    }
}
