#![allow(dead_code)]

//! Shared helpers for the gerber parity tests.

use std::path::{Path, PathBuf};

use serde_json::Value;
use stencicrity::gerber::{geom_bounds, GraphicObject};
use stencicrity::model::{Bounds, Geom};

/// Scratchpad holding the extracted example gerbers and the Python reference
/// dump, when this checkout has one.
const DEFAULT_REF: &str = "/tmp/claude-2017/-home-alex-lucaci-comanda-stencil/\
                           d66c9e56-b68d-4402-9c4d-b6e685a5f7d2/scratchpad";

pub const SKIP: &str =
    "skipping: no reference data (set STENCICRITY_GERBER_REF to a directory with gb/ and rust-gerber/ref/)";

/// Areas are compared with this relative tolerance ...
pub const AREA_REL_TOL: f64 = 1e-4;
/// ... or this absolute one (mm^2) for very small shapes.
pub const AREA_ABS_TOL: f64 = 1e-6;
/// Bounds tolerance (mm).
pub const BOUND_TOL: f64 = 1e-6;
/// Bounds tolerance for a stroked *arc* with a round aperture. The buffer is a
/// union of per-segment capsules, so the round joins of the discretised arc
/// land on slightly different vertices than GEOS' offset curve; the residue is
/// a fraction of the pen radius times the 3.75 degree chord sagitta.
pub const ARC_BOUND_TOL: f64 = 1e-5;

/// Tolerance for one object's bounds, by object kind.
pub fn bound_tol(obj: &GraphicObject) -> f64 {
    match obj {
        GraphicObject::Stroke(s) if s.seg.arc && s.aperture.template == "C" => ARC_BOUND_TOL,
        _ => BOUND_TOL,
    }
}

pub fn approx(got: f64, want: f64) -> bool {
    let d = (got - want).abs();
    d <= AREA_ABS_TOL || d <= AREA_REL_TOL * want.abs()
}

pub fn kind_of(obj: &GraphicObject) -> &'static str {
    match obj {
        GraphicObject::Flash(_) => "flash",
        GraphicObject::Stroke(_) => "stroke",
        GraphicObject::Region(_) => "region",
    }
}

pub fn bounds_of(g: &Geom) -> Option<Bounds> {
    geom_bounds(g)
}

pub struct Ref {
    root: PathBuf,
    index: Value,
}

impl Ref {
    pub fn open() -> Option<Ref> {
        let root = std::env::var("STENCICRITY_GERBER_REF")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(DEFAULT_REF));
        let index_path = root.join("rust-gerber/ref/index.json");
        if !index_path.is_file() || !root.join("gb").is_dir() {
            return None;
        }
        let index: Value =
            serde_json::from_str(&std::fs::read_to_string(&index_path).ok()?).ok()?;
        Some(Ref { root, index })
    }

    pub fn ref_dir(&self) -> PathBuf {
        self.root.join("rust-gerber/ref")
    }

    /// Version string the reference writer output was produced with.
    pub fn version(&self) -> String {
        self.index["version"]
            .as_str()
            .unwrap_or("0.0.0")
            .to_string()
    }

    /// `(relative gerber path, reference json)` for every example file.
    pub fn files(&self) -> Vec<(String, Value)> {
        self.index["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                let rel = f["rel"].as_str().unwrap().to_string();
                let path = self.ref_dir().join(f["json"].as_str().unwrap());
                let v: Value =
                    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
                (rel, v)
            })
            .collect()
    }

    pub fn gerber_text(&self, rel: &str) -> String {
        read_lossy(&self.root.join("gb").join(rel))
    }

    /// Reference writer output for one file and transform tag.
    pub fn writer_text(&self, rel: &str, tag: &str) -> String {
        let safe = rel.replace('/', "__");
        read_lossy(
            &self
                .ref_dir()
                .join("writer")
                .join(format!("{safe}.{tag}.gbr")),
        )
    }
}

fn read_lossy(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    String::from_utf8_lossy(&bytes).into_owned()
}
