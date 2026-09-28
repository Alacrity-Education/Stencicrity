//! RS-274X (Gerber X2) reader with planar geometry.
//!
//! Supports what KiCad emits: standard apertures (C/R/O/P), aperture macros
//! (primitives 1, 2/20, 21, 4, 5, 7), linear and circular interpolation,
//! regions (G36/G37), polarity and X2 file/aperture/object attributes.
//! Geometry is `geo::MultiPolygon<f64>` in file units (mm).

use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::model::{Bounds, Geom};

/// Chord error (mm) used when discretising arcs.
pub const ARC_TOLERANCE: f64 = 0.004;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct GerberError(pub String);

/// Discretise an arc into points including both end points.
pub fn arc_points(x0: f64, y0: f64, x1: f64, y1: f64, cx: f64, cy: f64, clockwise: bool) -> Vec<(f64, f64)> {
    let _ = (x0, y0, x1, y1, cx, cy, clockwise);
    todo!("arc discretisation")
}

/// Signed sweep angle (radians) from start to end around the centre.
pub fn arc_sweep(x0: f64, y0: f64, x1: f64, y1: f64, cx: f64, cy: f64, clockwise: bool) -> f64 {
    let _ = (x0, y0, x1, y1, cx, cy, clockwise);
    todo!()
}

/// A fully evaluated macro primitive, reduced to a circle or an outline.
#[derive(Clone, Debug, PartialEq)]
pub enum MacroPrim {
    Circle { exposure: bool, d: f64, cx: f64, cy: f64 },
    Outline { exposure: bool, points: Vec<(f64, f64)> },
}

impl MacroPrim {
    pub fn geometry(&self) -> Geom {
        todo!()
    }
    pub fn mirrored(&self) -> MacroPrim {
        todo!()
    }
    /// One primitive line of an aperture macro, e.g. `4,1,4,x1,y1,...,x1,y1,0*`.
    pub fn as_gerber(&self) -> String {
        todo!()
    }
}

#[derive(Clone, Debug)]
pub struct Macro {
    pub name: String,
    pub blocks: Vec<String>,
}

impl Macro {
    pub fn evaluate(&self, params: &[f64]) -> Result<Vec<MacroPrim>, GerberError> {
        let _ = params;
        todo!()
    }
}

#[derive(Debug)]
pub struct Aperture {
    pub code: u32,
    /// "C", "R", "O", "P" or a macro name
    pub template: String,
    pub modifiers: Vec<f64>,
    /// aperture attributes (TA.*), e.g. AperFunction -> "SMDPad,CuDef"
    pub attrs: BTreeMap<String, String>,
    pub macro_def: Option<Rc<Macro>>,
    geom: OnceCell<Geom>,
    prims: OnceCell<Vec<MacroPrim>>,
}

impl Aperture {
    pub fn new(code: u32, template: &str, modifiers: Vec<f64>, attrs: BTreeMap<String, String>, macro_def: Option<Rc<Macro>>) -> Self {
        Self { code, template: template.to_string(), modifiers, attrs, macro_def, geom: OnceCell::new(), prims: OnceCell::new() }
    }
    /// First token of the AperFunction attribute (e.g. "SMDPad").
    pub fn function(&self) -> String {
        self.attrs.get("AperFunction").map(|v| v.split(',').next().unwrap_or("").trim().to_string()).unwrap_or_default()
    }
    /// Evaluated macro primitives (empty for standard apertures).
    pub fn prims(&self) -> &[MacroPrim] {
        let _ = &self.prims;
        todo!()
    }
    /// Aperture shape centred on the origin (mm).
    pub fn geometry(&self) -> &Geom {
        let _ = &self.geom;
        todo!()
    }
    /// Short human-readable shape, e.g. "R 0.28x0.52", "C ⌀0.20", "RoundRect 1.20x0.80".
    pub fn describe(&self) -> String {
        todo!()
    }
}

/// An aperture with all macro variables resolved; what the writer emits.
#[derive(Clone, Debug)]
pub struct BakedAperture {
    /// "C", "R", "O", "P" or "MACRO"
    pub template: String,
    pub modifiers: Vec<f64>,
    pub prims: Vec<MacroPrim>,
    pub attrs: BTreeMap<String, String>,
}

impl BakedAperture {
    /// Deduplication key (includes attrs).
    pub fn key(&self) -> String {
        todo!()
    }
}

/// Resolve an aperture for output, optionally mirrored about the Y axis.
pub fn bake_aperture(ap: &Aperture, mirror: bool) -> BakedAperture {
    let _ = (ap, mirror);
    todo!()
}

/// One contour segment of a region, or the path of a stroke.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
    pub arc: bool,
    pub cx: f64,
    pub cy: f64,
    pub clockwise: bool,
}

impl Segment {
    pub fn line(x0: f64, y0: f64, x1: f64, y1: f64) -> Self {
        Self { x0, y0, x1, y1, arc: false, cx: 0.0, cy: 0.0, clockwise: false }
    }
    pub fn points(&self) -> Vec<(f64, f64)> {
        if self.arc { arc_points(self.x0, self.y0, self.x1, self.y1, self.cx, self.cy, self.clockwise) } else { vec![(self.x0, self.y0), (self.x1, self.y1)] }
    }
}

#[derive(Clone, Debug)]
pub struct Flash {
    pub x: f64,
    pub y: f64,
    pub aperture: Rc<Aperture>,
    /// object attributes (TO.*), e.g. P -> "D1,1,VDD_1", N -> "+5V", C -> "U1"
    pub attrs: BTreeMap<String, String>,
    pub dark: bool,
}

#[derive(Clone, Debug)]
pub struct Stroke {
    pub seg: Segment,
    pub aperture: Rc<Aperture>,
    pub attrs: BTreeMap<String, String>,
    pub dark: bool,
}

#[derive(Clone, Debug)]
pub struct Region {
    pub contours: Vec<Vec<Segment>>,
    pub attrs: BTreeMap<String, String>,
    pub dark: bool,
}

#[derive(Clone, Debug)]
pub enum GraphicObject {
    Flash(Flash),
    Stroke(Stroke),
    Region(Region),
}

impl GraphicObject {
    pub fn attrs(&self) -> &BTreeMap<String, String> {
        match self {
            GraphicObject::Flash(f) => &f.attrs,
            GraphicObject::Stroke(s) => &s.attrs,
            GraphicObject::Region(r) => &r.attrs,
        }
    }
    pub fn dark(&self) -> bool {
        match self {
            GraphicObject::Flash(f) => f.dark,
            GraphicObject::Stroke(s) => s.dark,
            GraphicObject::Region(r) => r.dark,
        }
    }
}

#[derive(Debug, Default)]
pub struct GerberFile {
    pub name: String,
    /// X2 file attributes (TF.*), e.g. FileFunction -> "Paste,Top", ProjectId -> "RBARF,<guid>,rev?"
    pub file_attrs: BTreeMap<String, String>,
    pub macros: BTreeMap<String, Rc<Macro>>,
    pub apertures: BTreeMap<u32, Rc<Aperture>>,
    pub objects: Vec<GraphicObject>,
}

impl GerberFile {
    pub fn file_function(&self) -> &str {
        self.file_attrs.get("FileFunction").map(String::as_str).unwrap_or("")
    }
    /// Bounding box of all objects' geometry, None when empty.
    pub fn bounds(&self) -> Option<Bounds> {
        todo!()
    }
}

/// Parse a gerber file's text.
pub fn parse_gerber(text: &str, name: &str) -> Result<GerberFile, GerberError> {
    let _ = (text, name);
    todo!()
}

/// Geometry of a graphic object in file coordinates (mm).
pub fn object_geometry(obj: &GraphicObject) -> Geom {
    let _ = obj;
    todo!()
}

/// Points along a region contour (arcs discretised).
pub fn contour_points(contour: &[Segment]) -> Vec<(f64, f64)> {
    let _ = contour;
    todo!()
}

/// Bounding box of a geometry, None when empty.
pub fn geom_bounds(g: &Geom) -> Option<Bounds> {
    use geo::BoundingRect;
    g.bounding_rect().map(|r| (r.min().x, r.min().y, r.max().x, r.max().y))
}
