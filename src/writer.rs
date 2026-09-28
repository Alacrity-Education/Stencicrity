//! RS-274X (X2) writer: emits merged layers in mm with a 4.6 coordinate format.

use std::collections::BTreeMap;
use std::path::Path;

use crate::gerber::GraphicObject;
use crate::model::Transform;

pub struct GerberWriter {
    file_function: String,
    polarity: String,
    software: String,
    version: String,
}

impl GerberWriter {
    pub fn new(file_function: &str) -> Self {
        Self { file_function: file_function.to_string(), polarity: "Positive".to_string(), software: "stencicrity".to_string(), version: crate::VERSION.to_string() }
    }
    /// G04 comment line (`*` and `%` are stripped from the text).
    pub fn comment(&mut self, text: &str) {
        let _ = text;
        todo!()
    }
    /// Flash / stroke / region with the transform applied (mirror-aware apertures and arcs).
    pub fn add_object(&mut self, obj: &GraphicObject, tr: &Transform) {
        let _ = (obj, tr);
        todo!()
    }
    /// Flash of a round aperture at sheet coordinates.
    pub fn add_circle(&mut self, x: f64, y: f64, dia: f64, attrs: Option<&BTreeMap<String, String>>) {
        let _ = (x, y, dia, attrs);
        todo!()
    }
    /// Flash of an obround (`O,wXh`) aperture at sheet coordinates.
    pub fn add_obround(&mut self, cx: f64, cy: f64, w: f64, h: f64, attrs: Option<&BTreeMap<String, String>>) {
        let _ = (cx, cy, w, h, attrs);
        todo!()
    }
    /// Filled region from points (skipped when degenerate).
    pub fn add_polygon(&mut self, points: &[(f64, f64)]) {
        let _ = points;
        todo!()
    }
    /// Stroke with a round aperture.
    pub fn add_line(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, width: f64) {
        let _ = (x0, y0, x1, y1, width);
        todo!()
    }
    pub fn add_rect_outline(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, width: f64) {
        let _ = (x0, y0, x1, y1, width);
        todo!()
    }
    /// The complete gerber file text.
    pub fn render(&self) -> String {
        let _ = (&self.file_function, &self.polarity, &self.software, &self.version);
        todo!()
    }
    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, self.render())
    }
}
