//! Design tokens: every colour the UI uses, for dark and light appearance.
//! Views read tokens, never literal colours, so a theme is one table.
//! Tokens are plain `0xRRGGBB` values, independent of the rendering backend;
//! [`Theme::color`] converts at the edge.

use gpui::{Rgba, WindowAppearance, rgb, rgba};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub bg: u32,
    pub sidebar: u32,
    pub surface: u32,
    pub surface_hover: u32,
    pub border: u32,
    pub text: u32,
    pub muted: u32,
    pub faint: u32,
    pub accent: u32,
    pub accent_text: u32,
    pub ok: u32,
    pub warn: u32,
    pub danger: u32,
    pub danger_text: u32,
}

impl Theme {
    pub const DARK: Theme = Theme {
        bg: 0x15171c,
        sidebar: 0x101216,
        surface: 0x1d2027,
        surface_hover: 0x262a33,
        border: 0x2c303a,
        text: 0xe6e8ee,
        muted: 0x9aa1b1,
        faint: 0x5c6372,
        accent: 0x4c8dff,
        accent_text: 0xffffff,
        ok: 0x3fb950,
        warn: 0xd29922,
        danger: 0xf85149,
        danger_text: 0xffffff,
    };

    pub const LIGHT: Theme = Theme {
        bg: 0xffffff,
        sidebar: 0xf4f5f7,
        surface: 0xf7f8fa,
        surface_hover: 0xeceef2,
        border: 0xdfe2e8,
        text: 0x1b1f27,
        muted: 0x5b6271,
        faint: 0x9aa1b1,
        accent: 0x1f6feb,
        accent_text: 0xffffff,
        ok: 0x1a7f37,
        warn: 0x9a6700,
        danger: 0xcf222e,
        danger_text: 0xffffff,
    };

    pub fn for_appearance(appearance: WindowAppearance) -> Theme {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::DARK,
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::LIGHT,
        }
    }

    /// A token as a GPUI colour.
    pub fn color(hex: u32) -> Rgba {
        rgb(hex)
    }

    /// A token at `alpha` (0–255), for tinted backgrounds.
    pub fn tint(hex: u32, alpha: u8) -> Rgba {
        rgba((hex << 8) | u32::from(alpha))
    }
}

/// Which theme the person wants.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemePreference {
    /// Follow the operating system's dark or light appearance.
    #[default]
    System,
    Dark,
    Light,
}

impl ThemePreference {
    pub fn parse(word: &str) -> Option<Self> {
        match word.to_ascii_lowercase().as_str() {
            "system" => Some(Self::System),
            "dark" => Some(Self::Dark),
            "light" => Some(Self::Light),
            _ => None,
        }
    }

    pub fn resolve(self, appearance: WindowAppearance) -> Theme {
        match self {
            Self::System => Theme::for_appearance(appearance),
            Self::Dark => Theme::DARK,
            Self::Light => Theme::LIGHT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tints_keep_the_colour_and_set_alpha() {
        let t = Theme::tint(0x3fb950, 0x20);
        let solid = Theme::color(0x3fb950);
        assert_eq!(
            (t.red, t.green, t.blue),
            (solid.red, solid.green, solid.blue)
        );
        assert!((t.alpha - 32.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn dark_and_light_appearances_pick_their_themes() {
        assert_eq!(
            Theme::for_appearance(WindowAppearance::VibrantDark),
            Theme::DARK
        );
        assert_eq!(Theme::for_appearance(WindowAppearance::Light), Theme::LIGHT);
    }

    #[test]
    fn an_explicit_preference_overrides_the_system() {
        assert_eq!(
            ThemePreference::Dark.resolve(WindowAppearance::Light),
            Theme::DARK
        );
        assert_eq!(
            ThemePreference::Light.resolve(WindowAppearance::Dark),
            Theme::LIGHT
        );
        assert_eq!(
            ThemePreference::System.resolve(WindowAppearance::Dark),
            Theme::DARK
        );
        assert_eq!(ThemePreference::parse("DARK"), Some(ThemePreference::Dark));
        assert_eq!(ThemePreference::parse("blue"), None);
    }
}
