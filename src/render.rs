//! Preview PNG of the whole sheet with rulers.

use std::path::Path;

use crate::model::{Layout, Project, SideId};

#[derive(Clone, Debug)]
pub struct RenderOptions {
    pub px_per_mm: f64,
    pub max_px: u32,
    /// pad highlighted in cyan: (side, index into side.pads)
    pub selected: Option<(SideId, usize)>,
    pub title: String,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self { px_per_mm: 20.0, max_px: 12000, selected: None, title: String::new() }
    }
}

/// Write the preview PNG.
pub fn render_preview(projects: &[Project], layout: &Layout, path: &Path, opts: &RenderOptions) -> anyhow::Result<()> {
    let _ = (projects, layout, path, opts);
    todo!()
}

/// Open a file with xdg-open (detached). Returns false when it could not be launched.
pub fn open_file(path: &Path) -> bool {
    std::process::Command::new("xdg-open").arg(path).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().is_ok()
}
