//! Discovery of gerber sets (zips or directories) and layer classification.

use crate::gerber::GerberFile;
use crate::model::Project;

/// One of "copper_top", "copper_bottom", "paste_top", "paste_bottom", "outline", or None.
/// Prefers the X2 `%TF.FileFunction` header, falls back to KiCad file name patterns.
pub fn classify_layer(filename: &str, text: &str) -> Option<&'static str> {
    let _ = (filename, text);
    todo!()
}

/// `{relative file name: text}` for the candidate gerber files of a zip or a directory.
pub fn load_source(path: &str) -> anyhow::Result<Vec<(String, String)>> {
    let _ = path;
    todo!()
}

/// Options for `discover_projects`.
#[derive(Clone, Debug, Default)]
pub struct DiscoverOptions {
    pub mirror_bottom: bool,
    /// absolute paths never scanned
    pub exclude_dirs: Vec<String>,
}

/// Find projects in explicit inputs, or (when `inputs` is empty) every `*.zip`
/// and gerber subdirectory of `cwd`. Warnings go to `warn`.
pub fn discover_projects(inputs: &[String], cwd: &str, opts: &DiscoverOptions, warn: &mut dyn FnMut(&str)) -> anyhow::Result<Vec<Project>> {
    let _ = (inputs, cwd, opts, warn);
    todo!()
}

/// True for gerbers produced by stencicrity itself (never an input).
pub fn is_own_output(gf: &GerberFile) -> bool {
    gf.file_attrs.get("GenerationSoftware").map(|v| v.to_lowercase().contains("pcbstencil") || v.to_lowercase().contains("stencicrity")).unwrap_or(false)
}
