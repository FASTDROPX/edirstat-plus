#[cfg(not(target_family = "wasm"))]
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[cfg(not(target_family = "wasm"))]
use directories::ProjectDirs;

use crate::{
    gui::{Locale, theme::ThemePreference},
    stats::treemap::TreemapStyle,
    time_utils::TimeFormat,
};

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserPreferences {
    #[serde(default)]
    pub map_defaults_version: u32,
    #[serde(default)]
    pub animation_defaults_version: u32,
    #[serde(default)]
    pub monospace_paths: bool,
    #[serde(default)]
    pub highlight_duplicates: bool,
    #[serde(default)]
    pub time_format: TimeFormat,
    #[serde(default = "default_true")]
    pub deletion_confirmation: bool,
    #[serde(default = "default_true")]
    pub trash_confirmation: bool,
    #[serde(default)]
    pub treemap_borders: bool,
    #[serde(default)]
    pub theme: ThemePreference,
    #[serde(default)]
    pub treemap_style: TreemapStyle,
    #[serde(default)]
    pub map_layout: crate::stats::treemap::MapLayout,
    #[serde(default)]
    pub map_color: crate::stats::treemap::MapColor,
    #[serde(default)]
    pub map_detail: crate::stats::treemap::MapDetail,
    #[serde(default)]
    pub map_animation: crate::stats::treemap::MapAnimation,
    #[serde(default)]
    pub map_palette: crate::stats::treemap::MapPalette,
    #[serde(default)]
    pub auto_elevate: bool,
    #[serde(default = "default_true")]
    pub live_tree: bool,
    #[serde(default)]
    pub locale: Option<Locale>,
}

const fn default_true() -> bool {
    true
}

impl Default for UserPreferences {
    fn default() -> Self {
        Self {
            map_defaults_version: 1,
            animation_defaults_version: 1,
            monospace_paths: false,
            highlight_duplicates: false,
            time_format: TimeFormat::default(),
            deletion_confirmation: true,
            trash_confirmation: true,
            treemap_borders: false,
            theme: ThemePreference::default(),
            treemap_style: TreemapStyle::default(),
            map_layout: Default::default(),
            map_color: Default::default(),
            map_detail: Default::default(),
            map_animation: Default::default(),
            map_palette: Default::default(),
            auto_elevate: false,
            live_tree: true,
            locale: None,
        }
    }
}

fn migrate_animation_defaults(prefs: &mut UserPreferences) {
    if prefs.animation_defaults_version < 1 {
        prefs.map_animation = crate::stats::treemap::MapAnimation::Soft;
        prefs.animation_defaults_version = 1;
    }
}

#[cfg(not(target_family = "wasm"))]
fn get_config_path() -> Option<PathBuf> {
    ProjectDirs::from("", "", "eDirStat").map(|dirs| dirs.config_dir().join("config.toml"))
}

#[cfg(not(target_family = "wasm"))]
#[must_use]
pub fn load_preferences() -> UserPreferences {
    if let Some(path) = get_config_path()
        && let Ok(contents) = std::fs::read_to_string(&path)
        && let Ok(mut prefs) = toml::from_str::<UserPreferences>(&contents)
    {
        if prefs.map_defaults_version < 1 {
            prefs.map_detail = crate::stats::treemap::MapDetail::Folders;
            prefs.map_defaults_version = 1;
        }
        migrate_animation_defaults(&mut prefs);
        return prefs;
    }
    UserPreferences::default() // Continue with default settings safely if unreadable/absent
}

/// On wasm there is no filesystem config; browser storage (localStorage)
/// support is a planned follow-up, but defaults always win for now.
#[cfg(target_family = "wasm")]
#[must_use]
pub fn load_preferences() -> UserPreferences {
    UserPreferences::default()
}

#[cfg(not(target_family = "wasm"))]
pub fn save_preferences(prefs: &UserPreferences) {
    if let Some(path) = get_config_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(contents) = toml::to_string_pretty(prefs) {
            let _ = std::fs::write(path, contents);
        }
    }
}

/// On wasm there is no filesystem config; saving is a no-op until browser
/// storage (localStorage) support lands.
#[cfg(target_family = "wasm")]
pub fn save_preferences(_prefs: &UserPreferences) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previous_automatic_off_is_migrated_once_but_user_choice_is_preserved() {
        let mut prefs: UserPreferences = toml::from_str("map_animation = 'Off'").unwrap();
        migrate_animation_defaults(&mut prefs);
        assert_eq!(
            prefs.map_animation,
            crate::stats::treemap::MapAnimation::Soft
        );
        prefs.map_animation = crate::stats::treemap::MapAnimation::Off;
        let mut restored: UserPreferences =
            toml::from_str(&toml::to_string(&prefs).unwrap()).unwrap();
        migrate_animation_defaults(&mut restored);
        assert_eq!(
            restored.map_animation,
            crate::stats::treemap::MapAnimation::Off
        );
    }

    #[test]
    fn test_user_preferences_default() {
        let prefs = UserPreferences::default();
        assert!(prefs.deletion_confirmation);
        assert!(prefs.trash_confirmation);
        assert!(!prefs.monospace_paths);
        assert!(!prefs.highlight_duplicates);
        assert!(!prefs.treemap_borders);
        assert_eq!(prefs.theme, ThemePreference::System);
        assert_eq!(prefs.treemap_style, TreemapStyle::OffsetVerticalGradient);
        assert_eq!(prefs.locale, None);
        assert_eq!(
            prefs.map_animation,
            crate::stats::treemap::MapAnimation::Soft
        );
    }

    #[test]
    fn test_deserialize_legacy_config() -> Result<(), toml::de::Error> {
        // Legacy config missing the confirmation fields should default to true,
        // missing treemap_borders should default to false, and missing locale to None
        let legacy_toml = r"
            monospace_paths = true
            highlight_duplicates = false
        ";
        let prefs: UserPreferences = toml::from_str(legacy_toml)?;
        assert!(prefs.monospace_paths);
        assert!(!prefs.highlight_duplicates);
        assert!(prefs.deletion_confirmation);
        assert!(prefs.trash_confirmation);
        assert!(!prefs.treemap_borders);
        assert_eq!(prefs.theme, ThemePreference::System);
        assert_eq!(prefs.treemap_style, TreemapStyle::OffsetVerticalGradient);
        assert_eq!(prefs.locale, None);
        assert_eq!(
            prefs.map_animation,
            crate::stats::treemap::MapAnimation::Soft
        );

        Ok(())
    }

    #[test]
    fn test_roundtrip_config() -> anyhow::Result<()> {
        let prefs = UserPreferences {
            deletion_confirmation: false,
            trash_confirmation: false,
            monospace_paths: true,
            theme: ThemePreference::Light,
            treemap_style: TreemapStyle::Cushion,
            locale: Some(Locale::TrTr),
            map_detail: crate::stats::treemap::MapDetail::Folders,
            map_palette: crate::stats::treemap::MapPalette::Vivid,
            auto_elevate: true,
            live_tree: false,
            map_animation: crate::stats::treemap::MapAnimation::Colors,
            ..Default::default()
        };

        let serialized = toml::to_string(&prefs)?;
        let deserialized: UserPreferences = toml::from_str(&serialized)?;
        assert_eq!(prefs, deserialized);

        Ok(())
    }
}

#[cfg(not(target_family = "wasm"))]
pub fn scan_cache_directory() -> Option<PathBuf> {
    ProjectDirs::from("", "", "eDirStat").map(|dirs| dirs.cache_dir().join("scan-cache-v1"))
}
