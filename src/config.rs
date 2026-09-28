//! The `.stencicrity` configuration file: sections stencil/layout/rules/sides/pads.

use std::path::Path;

use crate::model::{Config, Project};

pub const CONFIG_FILENAME: &str = ".stencicrity";
pub const LEGACY_CONFIG_SUFFIX: &str = ".stencil";

/// Missing file -> defaults; bad values warn and keep the default (never fails on content).
pub fn load_config(path: &Path, warn: &mut dyn FnMut(&str)) -> std::io::Result<Config> {
    let _ = (path, warn);
    todo!()
}

/// Parse file text (see `load_config`).
pub fn parse_config(text: &str, warn: &mut dyn FnMut(&str)) -> Config {
    let _ = (text, warn);
    todo!()
}

/// side.enabled and pad.state from the config dicts (defaults when absent).
pub fn apply_config(config: &Config, projects: &mut [Project]) {
    let _ = (config, projects);
    todo!()
}

/// The reverse: every relevant side and every candidate/closed pad into the config dicts.
pub fn collect_config(config: &mut Config, projects: &[Project]) {
    let _ = (config, projects);
    todo!()
}

/// The file text.
pub fn format_config(config: &Config, projects: &[Project]) -> String {
    let _ = (config, projects);
    todo!()
}

/// collect_config + atomic write (temp file + rename, keeping an existing mode).
pub fn save_config(path: &Path, config: &mut Config, projects: &[Project]) -> std::io::Result<()> {
    let _ = (path, config, projects);
    todo!()
}
