//! Pad detection on copper layers, default states and ordering.

use crate::model::{Pad, Project, Side, SideId};

pub const SMD_FUNCTIONS: [&str; 6] = ["SMDPad", "BGAPad", "HeatsinkPad", "FiducialPad", "TestPad", "ConnectorPad"];
pub const THT_FUNCTIONS: [&str; 2] = ["ComponentPad", "CastellatedPad"];
pub const NON_PAD_FUNCTIONS: [&str; 9] = ["ViaPad", "Conductor", "NonConductor", "EtchedComponent", "Profile", "WasherPad", "AntiPad", "Other", "Drawing"];

pub fn pad_key(project: &str, side: &str, ref_: &str, pin: &str, x: f64, y: f64) -> String {
    format!("{project}/{side}/{ref_}.{pin}@{x:.3},{y:.3}")
}

/// `^(?:PREFIX)\d`, case-insensitive.
pub fn matches_prefix(ref_: &str, prefixes: &[String]) -> bool {
    let _ = (ref_, prefixes);
    todo!()
}

/// Pasted pad -> open; candidate whose ref matches an ignore prefix -> ignore; else undefined.
pub fn default_state(pad: &Pad, ignore_prefixes: &[String]) -> &'static str {
    let _ = (pad, ignore_prefixes);
    todo!()
}

#[derive(Clone, Debug)]
pub struct DetectOptions {
    pub include_tht: bool,
    /// paste must cover at least this fraction of the pad area (or its centroid) to count
    pub min_overlap: f64,
    pub ignore_prefixes: Vec<String>,
}

impl Default for DetectOptions {
    fn default() -> Self {
        Self { include_tht: false, min_overlap: 0.10, ignore_prefixes: crate::model::DEFAULT_IGNORE_PREFIXES.iter().map(|s| s.to_string()).collect() }
    }
}

/// Fill `side.pads` from the copper flashes (with paste_indices and default states).
pub fn detect_pads(side: &mut Side, opts: &DetectOptions) {
    let _ = (side, opts);
    todo!()
}

/// Every pad (or only candidates) of the given sides, ordered by
/// (natural project name, top before bottom, natural ref, natural pin, x, y).
pub fn sorted_pads(projects: &[Project], sides: &[SideId], all_pads: bool) -> Vec<(SideId, usize)> {
    let _ = (projects, sides, all_pads);
    todo!()
}

/// (undefined, open, ignore) counts over candidates.
pub fn state_counts<'a>(pads: impl Iterator<Item = &'a Pad>) -> (usize, usize, usize) {
    let _ = pads;
    todo!()
}
