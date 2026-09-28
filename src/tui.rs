//! Terminal UI (ratatui + crossterm): pages Pads / Sides / Stencil / Layout.

use crate::model::{Config, Layout, Project, SideId};

/// Callbacks the TUI needs from the caller.
pub struct TuiHooks<'a> {
    /// Re-render the preview with the live state; `selected` pad highlighted; open the viewer when asked. Returns the PNG path.
    pub on_preview: &'a mut dyn FnMut(&[Project], &Config, Option<(SideId, usize)>, bool) -> String,
    /// Layout for the live state (or for an alternative config).
    pub compute_layout: &'a mut dyn FnMut(&[Project], &Config) -> Layout,
}

/// Run the TUI. Mutates side.enabled, pad.state and `config` in place.
/// Returns true when the user confirmed generation (`w`), false on quit (`q`).
pub fn run_tui(projects: &mut [Project], sides: &[SideId], config: &mut Config, hooks: TuiHooks<'_>, title: &str) -> anyhow::Result<bool> {
    let _ = (projects, sides, config, hooks, title);
    todo!()
}
