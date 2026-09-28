//! Cells, MaxRects packing, dotted borders, alignment features and the report.

use crate::model::{Config, Layout, Project, SideId};

/// Pack the enabled sides (ordered by config.layout.sort) onto the sheet.
pub fn pack(projects: &[Project], sides: &[SideId], config: &Config) -> Layout {
    let _ = (projects, sides, config);
    todo!()
}

/// Human readable report of the layout (stencil, block, datum, per cell).
pub fn layout_report(projects: &[Project], layout: &Layout, config: &Config) -> String {
    let _ = (projects, layout, config);
    todo!()
}

/// The two strokes of the X marker: [(x0,y0,x1,y1), (x0,y0,x1,y1)].
pub fn marker_strokes(center: (f64, f64), size: f64) -> [(f64, f64, f64, f64); 2] {
    let _ = (center, size);
    todo!()
}
