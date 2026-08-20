//! Where reel keeps user-installed themes and templates.
//!
//! `$REEL_CONFIG_DIR` overrides everything (and is what tests use). The
//! default is `~/.config/reel` everywhere unix-ish (deliberately including
//! macOS — CLI tools live in ~/.config by convention) and `%APPDATA%\reel`
//! on Windows.

use std::path::PathBuf;

pub fn config_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("REEL_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return Some(PathBuf::from(xdg).join("reel"));
        }
    }
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return Some(PathBuf::from(appdata).join("reel"));
        }
    }
    #[allow(deprecated)] // un-deprecated in modern Rust; fine on all targets
    std::env::home_dir().map(|h| h.join(".config").join("reel"))
}

pub fn themes_dir() -> Option<PathBuf> {
    config_dir().map(|d| d.join("themes"))
}

pub fn templates_dir() -> Option<PathBuf> {
    config_dir().map(|d| d.join("templates"))
}

/// Extra fonts loaded alongside the system's — drop .ttf/.otf files here to
/// use them without installing system-wide.
pub fn fonts_dir() -> Option<PathBuf> {
    config_dir().map(|d| d.join("fonts"))
}
