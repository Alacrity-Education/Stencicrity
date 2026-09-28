//! RS-274X (Gerber X2) reader with planar geometry.
//!
//! Supports what KiCad emits: standard apertures (C/R/O/P), aperture macros
//! (primitives 1, 2/20, 21, 4, 5, 7), linear and circular interpolation,
//! regions (G36/G37), polarity and X2 file/aperture/object attributes.
//! Geometry is `geo::MultiPolygon<f64>` in file units (mm).

use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use geo::{unary_union, BooleanOps, ConvexHull, Coord, LineString, MultiPolygon, Polygon};

use crate::model::{Bounds, Geom};

/// Chord error (mm) used when discretising arcs.
pub const ARC_TOLERANCE: f64 = 0.004;

const TWO_PI: f64 = std::f64::consts::TAU;
/// Vertices per full circle: shapely's `quad_segs=24` (24 per quarter).
const CIRCLE_SEGS: usize = 96;

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct GerberError(pub String);

fn err<T>(msg: impl Into<String>) -> Result<T, GerberError> {
    Err(GerberError(msg.into()))
}

// --------------------------------------------------------------------------- //
// Geometry helpers
// --------------------------------------------------------------------------- //

/// The empty geometry.
fn empty() -> Geom {
    MultiPolygon::new(Vec::new())
}

fn is_empty(g: &Geom) -> bool {
    g.0.is_empty()
}

/// Drop degeneracies and turn self-touching rings into proper holes.
///
/// Stands in for shapely's `make_valid` / `buffer(0)`: an even-odd union with
/// the empty geometry, which i_overlay resolves into non-overlapping rings
/// (exteriors counter-clockwise, holes clockwise).
fn fix(g: &Geom) -> Geom {
    if is_empty(g) {
        return empty();
    }
    g.union(&empty())
}

fn ring_from(points: &[(f64, f64)]) -> LineString<f64> {
    LineString(points.iter().map(|&(x, y)| Coord { x, y }).collect())
}

/// Polygon from a ring of points, normalised (may split or gain holes).
fn poly_of(points: &[(f64, f64)]) -> Geom {
    if points.len() < 3 {
        return empty();
    }
    fix(&MultiPolygon::new(vec![Polygon::new(
        ring_from(points),
        Vec::new(),
    )]))
}

/// Circle approximated the way shapely's `Point.buffer(r, quad_segs=24)` does:
/// 96 vertices, the first at angle 0.
fn circle_ring(cx: f64, cy: f64, r: f64) -> LineString<f64> {
    let mut coords = Vec::with_capacity(CIRCLE_SEGS + 1);
    for i in 0..CIRCLE_SEGS {
        let a = TWO_PI * (i as f64) / (CIRCLE_SEGS as f64);
        coords.push(Coord {
            x: cx + r * a.cos(),
            y: cy + r * a.sin(),
        });
    }
    coords.push(coords[0]);
    LineString(coords)
}

fn circle(cx: f64, cy: f64, r: f64) -> Geom {
    if r <= 0.0 {
        return empty();
    }
    MultiPolygon::new(vec![Polygon::new(circle_ring(cx, cy, r), Vec::new())])
}

fn rect(minx: f64, miny: f64, maxx: f64, maxy: f64) -> Geom {
    if maxx <= minx || maxy <= miny {
        return empty();
    }
    let ring = ring_from(&[
        (minx, miny),
        (maxx, miny),
        (maxx, maxy),
        (minx, maxy),
        (minx, miny),
    ]);
    MultiPolygon::new(vec![Polygon::new(ring, Vec::new())])
}

fn translate(g: &Geom, dx: f64, dy: f64) -> Geom {
    use geo::MapCoords;
    g.map_coords(|c| Coord {
        x: c.x + dx,
        y: c.y + dy,
    })
}

fn rotate_pt(x: f64, y: f64, deg: f64) -> (f64, f64) {
    if deg == 0.0 {
        return (x, y);
    }
    let a = deg.to_radians();
    let (s, c) = a.sin_cos();
    (x * c - y * s, x * s + y * c)
}

fn close_pt(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9
}

/// Union of already-normalised, non-overlapping-ring polygons.
fn union_all(parts: &[Geom]) -> Geom {
    let polys: Vec<&Polygon<f64>> = parts.iter().flat_map(|g| g.0.iter()).collect();
    match polys.len() {
        0 => empty(),
        1 => MultiPolygon::new(vec![polys[0].clone()]),
        _ => unary_union(polys),
    }
}

/// Number of chords needed so the chord error stays below `ARC_TOLERANCE`.
fn arc_steps(radius: f64, sweep: f64) -> usize {
    let sweep = sweep.abs();
    let quarter = std::f64::consts::FRAC_PI_4;
    let theta = if radius <= ARC_TOLERANCE {
        quarter
    } else {
        let t = 2.0 * (1.0 - ARC_TOLERANCE / radius).clamp(-1.0, 1.0).acos();
        t.min(quarter)
    };
    let n = (sweep / theta).ceil();
    if !n.is_finite() {
        return 1;
    }
    n.clamp(1.0, 4000.0) as usize
}

/// Signed sweep angle (radians) from start to end around the centre.
pub fn arc_sweep(x0: f64, y0: f64, x1: f64, y1: f64, cx: f64, cy: f64, clockwise: bool) -> f64 {
    let a0 = (y0 - cy).atan2(x0 - cx);
    let a1 = (y1 - cy).atan2(x1 - cx);
    let mut sweep = if clockwise {
        -((a0 - a1).rem_euclid(TWO_PI))
    } else {
        (a1 - a0).rem_euclid(TWO_PI)
    };
    if sweep.abs() < 1e-9 && (x1 - x0).hypot(y1 - y0) < 1e-9 {
        sweep = if clockwise { -TWO_PI } else { TWO_PI }; // full circle
    }
    sweep
}

/// Discretise an arc into points including both end points.
pub fn arc_points(
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    cx: f64,
    cy: f64,
    clockwise: bool,
) -> Vec<(f64, f64)> {
    let r0 = (x0 - cx).hypot(y0 - cy);
    let r1 = (x1 - cx).hypot(y1 - cy);
    let sweep = arc_sweep(x0, y0, x1, y1, cx, cy, clockwise);
    if sweep.abs() < 1e-9 {
        return vec![(x0, y0), (x1, y1)];
    }
    let n = arc_steps(r0.max(r1), sweep);
    let a0 = (y0 - cy).atan2(x0 - cx);
    let mut pts = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let t = (i as f64) / (n as f64);
        let r = r0 + (r1 - r0) * t;
        let a = a0 + sweep * t;
        pts.push((cx + r * a.cos(), cy + r * a.sin()));
    }
    let last = pts.len() - 1;
    pts[0] = (x0, y0);
    pts[last] = (x1, y1);
    pts
}

/// Capsule (rectangle plus two round caps) around one straight segment.
///
/// Reproduces the vertex set GEOS' `LineString.buffer(r, quad_segs=24)` emits
/// for a two-point line: the cap arcs start on the segment normal and step in
/// 360/96 degree increments.
fn capsule(p0: (f64, f64), p1: (f64, f64), r: f64) -> Geom {
    let (dx, dy) = (p1.0 - p0.0, p1.1 - p0.1);
    let len = dx.hypot(dy);
    if len < 1e-12 {
        return circle(p0.0, p0.1, r);
    }
    let a = dy.atan2(dx) - std::f64::consts::FRAC_PI_2; // angle of -normal
    let half = CIRCLE_SEGS / 2;
    let step = TWO_PI / (CIRCLE_SEGS as f64);
    let mut pts: Vec<(f64, f64)> = Vec::with_capacity(CIRCLE_SEGS + 4);
    // bottom edge, then the cap around p1 from a to a + pi
    for (c, base) in [(p1, a), (p0, a + std::f64::consts::PI)] {
        for k in 0..=half {
            let ang = base + step * (k as f64);
            pts.push((c.0 + r * ang.cos(), c.1 + r * ang.sin()));
        }
    }
    pts.push(pts[0]);
    MultiPolygon::new(vec![Polygon::new(ring_from(&pts), Vec::new())])
}

/// Buffer a polyline with a round pen of radius `r` (shapely `buffer`).
fn buffer_path(pts: &[(f64, f64)], r: f64) -> Geom {
    if r <= 0.0 {
        return empty();
    }
    if pts.is_empty() {
        return empty();
    }
    if pts.len() == 1 {
        return circle(pts[0].0, pts[0].1, r);
    }
    let mut parts: Vec<Geom> = Vec::with_capacity(pts.len());
    for w in pts.windows(2) {
        if close_pt(w[0], w[1]) {
            continue;
        }
        parts.push(capsule(w[0], w[1], r));
    }
    if parts.is_empty() {
        return circle(pts[0].0, pts[0].1, r);
    }
    if parts.len() == 1 {
        return parts.pop().unwrap();
    }
    union_all(&parts)
}

// --------------------------------------------------------------------------- //
// Aperture macros
// --------------------------------------------------------------------------- //

/// Tiny recursive-descent evaluator for macro expressions (`$1+$1`, `2x$3`, ...).
struct Expr<'a> {
    s: Vec<char>,
    i: usize,
    vars: &'a BTreeMap<i64, f64>,
}

impl<'a> Expr<'a> {
    fn new(text: &str, vars: &'a BTreeMap<i64, f64>) -> Self {
        Self {
            s: text.chars().filter(|c| !c.is_whitespace()).collect(),
            i: 0,
            vars,
        }
    }

    fn text(&self) -> String {
        self.s.iter().collect()
    }

    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn take(&mut self) -> Option<char> {
        let c = self.s.get(self.i).copied();
        if c.is_some() {
            self.i += 1;
        }
        c
    }

    fn parse(&mut self) -> Result<f64, GerberError> {
        let v = self.expr()?;
        if self.i != self.s.len() {
            return err(format!("bad macro expression: {:?}", self.text()));
        }
        Ok(v)
    }

    fn expr(&mut self) -> Result<f64, GerberError> {
        let mut v = self.term()?;
        while matches!(self.peek(), Some('+') | Some('-')) {
            let op = self.take().unwrap();
            let t = self.term()?;
            v = if op == '+' { v + t } else { v - t };
        }
        Ok(v)
    }

    fn term(&mut self) -> Result<f64, GerberError> {
        let mut v = self.factor()?;
        while matches!(self.peek(), Some('x') | Some('X') | Some('/')) {
            let op = self.take().unwrap();
            let f = self.factor()?;
            v = if op == '/' { v / f } else { v * f };
        }
        Ok(v)
    }

    fn factor(&mut self) -> Result<f64, GerberError> {
        match self.peek() {
            Some('-') => {
                self.take();
                Ok(-self.factor()?)
            }
            Some('+') => {
                self.take();
                self.factor()
            }
            Some('(') => {
                self.take();
                let v = self.expr()?;
                if self.take() != Some(')') {
                    return err(format!("missing ')' in {:?}", self.text()));
                }
                Ok(v)
            }
            Some('$') => {
                self.take();
                let j = self.i;
                while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    self.take();
                }
                let digits: String = self.s[j..self.i].iter().collect();
                let idx: i64 = digits.parse().unwrap_or(0);
                Ok(self.vars.get(&idx).copied().unwrap_or(0.0))
            }
            c => {
                let j = self.i;
                while self
                    .peek()
                    .is_some_and(|ch| ch.is_ascii_digit() || ch == '.')
                {
                    self.take();
                }
                if j == self.i {
                    return err(format!(
                        "unexpected {:?} in macro expression {:?}",
                        c.map(String::from).unwrap_or_default(),
                        self.text()
                    ));
                }
                let num: String = self.s[j..self.i].iter().collect();
                num.parse::<f64>()
                    .map_err(|_| GerberError(format!("bad number {num:?} in macro expression")))
            }
        }
    }
}

/// A fully evaluated macro primitive, reduced to a circle or an outline.
#[derive(Clone, Debug, PartialEq)]
pub enum MacroPrim {
    Circle {
        exposure: bool,
        d: f64,
        cx: f64,
        cy: f64,
    },
    Outline {
        exposure: bool,
        points: Vec<(f64, f64)>,
    },
}

impl MacroPrim {
    pub fn exposure(&self) -> bool {
        match self {
            MacroPrim::Circle { exposure, .. } => *exposure,
            MacroPrim::Outline { exposure, .. } => *exposure,
        }
    }

    pub fn geometry(&self) -> Geom {
        match self {
            MacroPrim::Circle { d, cx, cy, .. } => circle(*cx, *cy, d / 2.0),
            MacroPrim::Outline { points, .. } => poly_of(points),
        }
    }

    pub fn mirrored(&self) -> MacroPrim {
        match self {
            MacroPrim::Circle {
                exposure,
                d,
                cx,
                cy,
            } => MacroPrim::Circle {
                exposure: *exposure,
                d: *d,
                cx: -*cx,
                cy: *cy,
            },
            MacroPrim::Outline { exposure, points } => MacroPrim::Outline {
                exposure: *exposure,
                points: points.iter().map(|&(x, y)| (-x, y)).collect(),
            },
        }
    }

    /// One primitive line of an aperture macro, e.g. `4,1,4,x1,y1,...,x1,y1,0*`.
    pub fn as_gerber(&self) -> String {
        match self {
            MacroPrim::Circle {
                exposure,
                d,
                cx,
                cy,
            } => {
                let exp = if *exposure { "1" } else { "0" };
                format!("1,{exp},{d:.6},{cx:.6},{cy:.6}*")
            }
            MacroPrim::Outline { exposure, points } => {
                let exp = if *exposure { "1" } else { "0" };
                let mut pts = points.clone();
                if pts.len() > 1 && close_pt(pts[0], pts[pts.len() - 1]) {
                    pts.pop();
                }
                if pts.is_empty() {
                    return format!("4,{exp},0,0*");
                }
                let n = pts.len();
                let first = pts[0];
                pts.push(first);
                let coords: Vec<String> =
                    pts.iter().map(|&(x, y)| format!("{x:.6},{y:.6}")).collect();
                format!("4,{exp},{n},{},0*", coords.join(","))
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct Macro {
    pub name: String,
    pub blocks: Vec<String>,
}

impl Macro {
    pub fn evaluate(&self, params: &[f64]) -> Result<Vec<MacroPrim>, GerberError> {
        let mut vars: BTreeMap<i64, f64> = BTreeMap::new();
        for (i, p) in params.iter().enumerate() {
            vars.insert(i as i64 + 1, *p);
        }
        let mut prims: Vec<MacroPrim> = Vec::new();
        for raw in &self.blocks {
            let blk = raw.trim();
            let first = match blk.chars().next() {
                None => continue,
                Some(c) => c,
            };
            if first == '0' {
                continue; // comment
            }
            if first == '$' {
                let (name, expr) = match blk.split_once('=') {
                    Some(v) => v,
                    None => return err(format!("bad macro variable {blk:?} in {}", self.name)),
                };
                let idx: i64 = name[1..].trim().parse().unwrap_or(0);
                let value = Expr::new(expr, &vars).parse()?;
                vars.insert(idx, value);
                continue;
            }
            let mut parts = blk.split(',');
            let code_txt = parts.next().unwrap_or("").trim();
            let code: i64 = code_txt.parse().map_err(|_| {
                GerberError(format!("bad macro primitive {blk:?} in {}", self.name))
            })?;
            let mut mods: Vec<f64> = Vec::new();
            for p in parts {
                mods.push(Expr::new(p, &vars).parse()?);
            }
            prims.extend(self.primitive(code, &mods)?);
        }
        Ok(prims)
    }

    fn primitive(&self, code: i64, m: &[f64]) -> Result<Vec<MacroPrim>, GerberError> {
        let at = |i: usize| -> f64 { m.get(i).copied().unwrap_or(0.0) };
        match code {
            1 => {
                let exp = at(0) != 0.0;
                let (d, cx, cy) = (at(1), at(2), at(3));
                let rot = if m.len() > 4 { m[4] } else { 0.0 };
                let (cx, cy) = rotate_pt(cx, cy, rot);
                Ok(vec![MacroPrim::Circle {
                    exposure: exp,
                    d,
                    cx,
                    cy,
                }])
            }
            2 | 20 => {
                let exp = at(0) != 0.0;
                let (w, x1, y1, x2, y2) = (at(1), at(2), at(3), at(4), at(5));
                let rot = if m.len() > 6 { m[6] } else { 0.0 };
                let (dx, dy) = (x2 - x1, y2 - y1);
                let length = dx.hypot(dy);
                if length < 1e-12 {
                    let (cx, cy) = rotate_pt(x1, y1, rot);
                    return Ok(vec![MacroPrim::Circle {
                        exposure: exp,
                        d: w,
                        cx,
                        cy,
                    }]);
                }
                let nx = -dy / length * w / 2.0;
                let ny = dx / length * w / 2.0;
                let pts = [
                    (x1 + nx, y1 + ny),
                    (x2 + nx, y2 + ny),
                    (x2 - nx, y2 - ny),
                    (x1 - nx, y1 - ny),
                ];
                Ok(vec![MacroPrim::Outline {
                    exposure: exp,
                    points: pts.iter().map(|&(x, y)| rotate_pt(x, y, rot)).collect(),
                }])
            }
            21 => {
                let exp = at(0) != 0.0;
                let (w, h, cx, cy) = (at(1), at(2), at(3), at(4));
                let rot = if m.len() > 5 { m[5] } else { 0.0 };
                let pts = [
                    (cx - w / 2.0, cy - h / 2.0),
                    (cx + w / 2.0, cy - h / 2.0),
                    (cx + w / 2.0, cy + h / 2.0),
                    (cx - w / 2.0, cy + h / 2.0),
                ];
                Ok(vec![MacroPrim::Outline {
                    exposure: exp,
                    points: pts.iter().map(|&(x, y)| rotate_pt(x, y, rot)).collect(),
                }])
            }
            4 => {
                let exp = at(0) != 0.0;
                let n = (at(1).round() as i64).clamp(0, 1_000_000) as usize;
                let rot_at = 2 + 2 * (n + 1);
                let end = rot_at.min(m.len());
                let coords = &m[2.min(m.len())..end.max(2.min(m.len()))];
                let rot = if m.len() > rot_at { m[rot_at] } else { 0.0 };
                let mut pts: Vec<(f64, f64)> = Vec::new();
                let mut i = 0;
                while i + 1 < coords.len() {
                    pts.push((coords[i], coords[i + 1]));
                    i += 2;
                }
                if pts.len() > 1 && close_pt(pts[0], pts[pts.len() - 1]) {
                    pts.pop();
                }
                Ok(vec![MacroPrim::Outline {
                    exposure: exp,
                    points: pts.iter().map(|&(x, y)| rotate_pt(x, y, rot)).collect(),
                }])
            }
            5 => {
                let exp = at(0) != 0.0;
                let n = at(1).round() as i64;
                let (cx, cy, d) = (at(2), at(3), at(4));
                let rot = if m.len() > 5 { m[5] } else { 0.0 };
                if n <= 0 {
                    return err(format!("bad polygon primitive in {}", self.name));
                }
                let pts: Vec<(f64, f64)> = (0..n)
                    .map(|i| {
                        let a = TWO_PI * (i as f64) / (n as f64);
                        (cx + d / 2.0 * a.cos(), cy + d / 2.0 * a.sin())
                    })
                    .collect();
                Ok(vec![MacroPrim::Outline {
                    exposure: exp,
                    points: pts.iter().map(|&(x, y)| rotate_pt(x, y, rot)).collect(),
                }])
            }
            7 => {
                let (cx, cy, od, idia, gap) = (at(0), at(1), at(2), at(3), at(4));
                let rot = if m.len() > 5 { m[5] } else { 0.0 };
                let ring = circle(cx, cy, od / 2.0).difference(&circle(cx, cy, idia / 2.0));
                let cross = rect(cx - od, cy - gap / 2.0, cx + od, cy + gap / 2.0).union(&rect(
                    cx - gap / 2.0,
                    cy - od,
                    cx + gap / 2.0,
                    cy + od,
                ));
                let geom = ring.difference(&cross);
                let mut out = Vec::new();
                for p in geom.0.iter() {
                    let mut pts: Vec<(f64, f64)> = p
                        .exterior()
                        .0
                        .iter()
                        .map(|c| rotate_pt(c.x, c.y, rot))
                        .collect();
                    if pts.len() > 1 {
                        pts.pop(); // shapely's coords[:-1]
                    }
                    out.push(MacroPrim::Outline {
                        exposure: true,
                        points: pts,
                    });
                }
                Ok(out)
            }
            6 => Ok(Vec::new()), // moiré: decoration only, ignore
            _ => err(format!(
                "unsupported macro primitive {code} in {}",
                self.name
            )),
        }
    }
}

// --------------------------------------------------------------------------- //
// Apertures
// --------------------------------------------------------------------------- //

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
    pub fn new(
        code: u32,
        template: &str,
        modifiers: Vec<f64>,
        attrs: BTreeMap<String, String>,
        macro_def: Option<Rc<Macro>>,
    ) -> Self {
        Self {
            code,
            template: template.to_string(),
            modifiers,
            attrs,
            macro_def,
            geom: OnceCell::new(),
            prims: OnceCell::new(),
        }
    }
    /// First token of the AperFunction attribute (e.g. "SMDPad").
    pub fn function(&self) -> String {
        self.attrs
            .get("AperFunction")
            .map(|v| v.split(',').next().unwrap_or("").trim().to_string())
            .unwrap_or_default()
    }
    /// Evaluated macro primitives (empty for standard apertures).
    pub fn prims(&self) -> &[MacroPrim] {
        self.prims.get_or_init(|| match &self.macro_def {
            Some(m) => m.evaluate(&self.modifiers).unwrap_or_default(),
            None => Vec::new(),
        })
    }
    /// Aperture shape centred on the origin (mm).
    pub fn geometry(&self) -> &Geom {
        self.geom.get_or_init(|| self.build_geometry())
    }

    fn build_geometry(&self) -> Geom {
        let m = &self.modifiers;
        let at = |i: usize| -> f64 { m.get(i).copied().unwrap_or(0.0) };
        let opt = |i: usize| -> f64 {
            if m.len() > i {
                m[i]
            } else {
                0.0
            }
        };
        let mut hole = 0.0;
        let geom = match self.template.as_str() {
            "C" => {
                hole = opt(1);
                circle(0.0, 0.0, at(0) / 2.0)
            }
            "R" => {
                hole = opt(2);
                let (w, h) = (at(0), at(1));
                rect(-w / 2.0, -h / 2.0, w / 2.0, h / 2.0)
            }
            "O" => {
                hole = opt(2);
                let (w, h) = (at(0), at(1));
                if (w - h).abs() < 1e-9 {
                    circle(0.0, 0.0, w / 2.0)
                } else if w > h {
                    buffer_path(&[(-(w - h) / 2.0, 0.0), ((w - h) / 2.0, 0.0)], h / 2.0)
                } else {
                    buffer_path(&[(0.0, -(h - w) / 2.0), (0.0, (h - w) / 2.0)], w / 2.0)
                }
            }
            "P" => {
                hole = opt(3);
                let d = at(0);
                let n = at(1).round() as i64;
                let rot = opt(2);
                if n <= 0 {
                    empty()
                } else {
                    let pts: Vec<(f64, f64)> = (0..n)
                        .map(|i| {
                            let a = TWO_PI * (i as f64) / (n as f64);
                            rotate_pt(d / 2.0 * a.cos(), d / 2.0 * a.sin(), rot)
                        })
                        .collect();
                    poly_of(&pts)
                }
            }
            _ if self.macro_def.is_some() => {
                let mut geom = empty();
                for prim in self.prims() {
                    let g = prim.geometry();
                    geom = if prim.exposure() {
                        geom.union(&g)
                    } else {
                        geom.difference(&g)
                    };
                }
                geom
            }
            _ => empty(),
        };
        if hole > 0.0 {
            return geom.difference(&circle(0.0, 0.0, hole / 2.0));
        }
        geom
    }

    /// Short human-readable shape, e.g. "R 0.28x0.52", "C ⌀0.20", "RoundRect 1.20x0.80".
    pub fn describe(&self) -> String {
        let m = &self.modifiers;
        let at = |i: usize| -> f64 { m.get(i).copied().unwrap_or(0.0) };
        let t = self.template.as_str();
        match t {
            "C" => return format!("C ⌀{:.2}", at(0)),
            "R" | "O" => return format!("{t} {:.2}x{:.2}", at(0), at(1)),
            "P" => return format!("P ⌀{:.2} n={}", at(0), at(1) as i64),
            _ => {}
        }
        match geom_bounds(self.geometry()) {
            Some((minx, miny, maxx, maxy)) => {
                // Boolean ops snap to a fixed-point grid (~1e-8 mm here), which
                // can tip a size that is exactly on a rounding boundary. Quantise
                // to the gerber file resolution (1 nm) before showing 1/100 mm.
                let q = |v: f64| (v * 1e6).round() / 1e6;
                format!("{t} {:.2}x{:.2}", q(maxx - minx), q(maxy - miny))
            }
            None => format!("{t} 0.00x0.00"),
        }
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
        let attrs: Vec<String> = self.attrs.iter().map(|(k, v)| format!("{k}={v}")).collect();
        let attrs = attrs.join(";");
        if self.template == "MACRO" {
            let body: String = self.prims.iter().map(|p| p.as_gerber()).collect();
            return format!("MACRO|{body}|{attrs}");
        }
        let mods: Vec<String> = self.modifiers.iter().map(|v| format!("{v:.6}")).collect();
        format!("{},{}|{attrs}", self.template, mods.join("X"))
    }
}

/// Resolve an aperture for output, optionally mirrored about the Y axis.
pub fn bake_aperture(ap: &Aperture, mirror: bool) -> BakedAperture {
    if ap.macro_def.is_some() {
        let prims: Vec<MacroPrim> = if mirror {
            ap.prims().iter().map(|p| p.mirrored()).collect()
        } else {
            ap.prims().to_vec()
        };
        return BakedAperture {
            template: "MACRO".to_string(),
            modifiers: Vec::new(),
            prims,
            attrs: ap.attrs.clone(),
        };
    }
    let mut mods = ap.modifiers.clone();
    if ap.template == "P" && mirror {
        let rot = if mods.len() > 2 { mods[2] } else { 0.0 };
        if mods.len() > 2 {
            mods[2] = 180.0 - rot;
        } else {
            mods.push(180.0 - rot);
        }
    }
    BakedAperture {
        template: ap.template.clone(),
        modifiers: mods,
        prims: Vec::new(),
        attrs: ap.attrs.clone(),
    }
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
        Self {
            x0,
            y0,
            x1,
            y1,
            arc: false,
            cx: 0.0,
            cy: 0.0,
            clockwise: false,
        }
    }
    pub fn points(&self) -> Vec<(f64, f64)> {
        if self.arc {
            arc_points(
                self.x0,
                self.y0,
                self.x1,
                self.y1,
                self.cx,
                self.cy,
                self.clockwise,
            )
        } else {
            vec![(self.x0, self.y0), (self.x1, self.y1)]
        }
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
        self.file_attrs
            .get("FileFunction")
            .map(String::as_str)
            .unwrap_or("")
    }
    /// Bounding box of all objects' geometry, None when empty.
    pub fn bounds(&self) -> Option<Bounds> {
        let mut out: Option<Bounds> = None;
        for obj in &self.objects {
            let g = object_geometry(obj);
            let b = match geom_bounds(&g) {
                Some(b) => b,
                None => continue,
            };
            out = Some(match out {
                None => b,
                Some(p) => (p.0.min(b.0), p.1.min(b.1), p.2.max(b.2), p.3.max(b.3)),
            });
        }
        out
    }
}

/// Geometry of a graphic object in file coordinates (mm).
pub fn object_geometry(obj: &GraphicObject) -> Geom {
    match obj {
        GraphicObject::Flash(f) => translate(f.aperture.geometry(), f.x, f.y),
        GraphicObject::Stroke(s) => {
            let ap = &s.aperture;
            let pts = s.seg.points();
            if ap.template == "C" {
                let width = ap.modifiers.first().copied().unwrap_or(0.0);
                if pts.is_empty() {
                    return empty();
                }
                if pts.len() < 2 || pts.iter().all(|p| close_pt(*p, pts[0])) {
                    return circle(pts[0].0, pts[0].1, width / 2.0);
                }
                return buffer_path(&pts, width / 2.0);
            }
            // Non-round stroke aperture: hull of the aperture swept along the path
            let base = ap.geometry();
            if is_empty(base) || pts.is_empty() {
                return empty();
            }
            let mut coords: Vec<Coord<f64>> = Vec::new();
            for p in base.0.iter() {
                for c in p.exterior().0.iter() {
                    for &(dx, dy) in &pts {
                        coords.push(Coord {
                            x: c.x + dx,
                            y: c.y + dy,
                        });
                    }
                }
            }
            if coords.len() < 3 {
                return empty();
            }
            let hull = LineString(coords).convex_hull();
            if hull.exterior().0.len() < 4 {
                return empty();
            }
            MultiPolygon::new(vec![hull])
        }
        GraphicObject::Region(r) => {
            let mut polys: Vec<Geom> = Vec::new();
            for contour in &r.contours {
                let pts = contour_points(contour);
                if pts.len() >= 3 {
                    let g = poly_of(&pts);
                    if !is_empty(&g) {
                        polys.push(g);
                    }
                }
            }
            union_all(&polys)
        }
    }
}

/// Points along a region contour (arcs discretised).
pub fn contour_points(contour: &[Segment]) -> Vec<(f64, f64)> {
    let mut pts: Vec<(f64, f64)> = Vec::new();
    for seg in contour {
        let sp = seg.points();
        let skip = if !pts.is_empty() && !sp.is_empty() && close_pt(pts[pts.len() - 1], sp[0]) {
            1
        } else {
            0
        };
        pts.extend_from_slice(&sp[skip..]);
    }
    pts
}

/// Bounding box of a geometry, None when empty.
pub fn geom_bounds(g: &Geom) -> Option<Bounds> {
    use geo::BoundingRect;
    g.bounding_rect()
        .map(|r| (r.min().x, r.min().y, r.max().x, r.max().y))
}

// --------------------------------------------------------------------------- //
// Parser
// --------------------------------------------------------------------------- //

enum Block<'a> {
    Ext(&'a str),
    Data(&'a str),
}

/// Split the file text into extended commands and data blocks.
fn iter_blocks(text: &str) -> Result<Vec<Block<'_>>, GerberError> {
    let b = text.as_bytes();
    let n = b.len();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < n {
        let c = b[i];
        if c == b' ' || c == b'\r' || c == b'\n' || c == b'\t' {
            i += 1;
            continue;
        }
        if c == b'%' {
            let j = match text[i + 1..].find('%') {
                Some(k) => i + 1 + k,
                None => return err("unterminated extended command"),
            };
            out.push(Block::Ext(&text[i + 1..j]));
            i = j + 1;
        } else {
            let j = match text[i..].find('*') {
                Some(k) => i + k,
                None => return err("unterminated data block"),
            };
            out.push(Block::Data(text[i..j].trim()));
            i = j + 1;
        }
    }
    Ok(out)
}

/// `([A-Z])([+-]?[0-9.]*)` over one data block.
fn tokens(block: &str) -> Vec<(char, String)> {
    let b: Vec<char> = block.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_uppercase() {
            i += 1;
            let start = i;
            if i < b.len() && (b[i] == '+' || b[i] == '-') {
                i += 1;
            }
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == '.') {
                i += 1;
            }
            out.push((c, b[start..i].iter().collect()));
        } else {
            i += 1;
        }
    }
    out
}

/// `^ADD(\d+)([A-Za-z_.$][A-Za-z0-9_.$]*)(?:,(.*))?$`
fn parse_ad(cmd: &str) -> Option<(u32, String, Option<String>)> {
    if !cmd.starts_with("ADD") {
        return None;
    }
    let b: Vec<char> = cmd.chars().collect();
    let mut i = 3usize;
    let ds = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i == ds {
        return None;
    }
    let code: u32 = b[ds..i].iter().collect::<String>().parse().ok()?;
    let ts = i;
    let head_ok = |c: char| c.is_ascii_alphabetic() || c == '_' || c == '.' || c == '$';
    let tail_ok = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '$';
    if i >= b.len() || !head_ok(b[i]) {
        return None;
    }
    i += 1;
    while i < b.len() && tail_ok(b[i]) {
        i += 1;
    }
    let template: String = b[ts..i].iter().collect();
    if i == b.len() {
        return Some((code, template, None));
    }
    if b[i] != ',' {
        return None;
    }
    let rest: String = b[i + 1..].iter().collect();
    Some((code, template, Some(rest)))
}

/// `X([2-9]|\d\d)|Y([2-9]|\d\d)`
fn has_repeat(cmd: &str) -> bool {
    let b: Vec<char> = cmd.chars().collect();
    for i in 0..b.len() {
        if b[i] != 'X' && b[i] != 'Y' {
            continue;
        }
        let j = i + 1;
        if j < b.len() && ('2'..='9').contains(&b[j]) {
            return true;
        }
        if j + 1 < b.len() && b[j].is_ascii_digit() && b[j + 1].is_ascii_digit() {
            return true;
        }
    }
    false
}

/// `[AB]-?0*[1-9]|[AB]0*\.0*[1-9]`
fn has_nonzero_offset(cmd: &str) -> bool {
    let b: Vec<char> = cmd.chars().collect();
    let zeros_then_nonzero = |mut k: usize| -> bool {
        while k < b.len() && b[k] == '0' {
            k += 1;
        }
        k < b.len() && ('1'..='9').contains(&b[k])
    };
    for i in 0..b.len() {
        if b[i] != 'A' && b[i] != 'B' {
            continue;
        }
        // branch 1: -?0*[1-9]
        let mut k = i + 1;
        if k < b.len() && b[k] == '-' {
            k += 1;
        }
        if zeros_then_nonzero(k) {
            return true;
        }
        // branch 2: 0*\.0*[1-9]
        let mut k = i + 1;
        while k < b.len() && b[k] == '0' {
            k += 1;
        }
        if k < b.len() && b[k] == '.' && zeros_then_nonzero(k + 1) {
            return true;
        }
    }
    false
}

struct Parser {
    gf: GerberFile,
    int_digits: usize,
    dec_digits: usize,
    trailing_zero_omission: bool,
    incremental: bool,
    scale: f64,
    cur_ta: BTreeMap<String, String>,
    cur_to: BTreeMap<String, String>,
    aperture: Option<Rc<Aperture>>,
    interp: u8,
    multi_quadrant: bool,
    dark: bool,
    x: f64,
    y: f64,
    in_region: bool,
    contours: Vec<Vec<Segment>>,
    region_attrs: BTreeMap<String, String>,
    done: bool,
}

impl Parser {
    fn new() -> Self {
        Self {
            gf: GerberFile::default(),
            int_digits: 4,
            dec_digits: 6,
            trailing_zero_omission: false,
            incremental: false,
            scale: 1.0,
            cur_ta: BTreeMap::new(),
            cur_to: BTreeMap::new(),
            aperture: None,
            interp: 1,
            multi_quadrant: true,
            dark: true,
            x: 0.0,
            y: 0.0,
            in_region: false,
            contours: Vec::new(),
            region_attrs: BTreeMap::new(),
            done: false,
        }
    }

    fn parse(mut self, text: &str, name: &str) -> Result<GerberFile, GerberError> {
        self.gf.name = name.to_string();
        for block in iter_blocks(text)? {
            if self.done {
                break;
            }
            match block {
                Block::Ext(c) => self.extended(c)?,
                Block::Data(c) => self.data(c)?,
            }
        }
        if self.in_region {
            self.end_region();
        }
        Ok(self.gf)
    }

    fn extended(&mut self, content: &str) -> Result<(), GerberError> {
        let parts: Vec<&str> = content
            .split('*')
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
            .collect();
        if parts.is_empty() {
            return Ok(());
        }
        if let Some(rest) = parts[0].strip_prefix("AM") {
            let name = rest.trim().to_string();
            let blocks: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();
            self.gf
                .macros
                .insert(name.clone(), Rc::new(Macro { name, blocks }));
            return Ok(());
        }
        for cmd in parts {
            self.ext_cmd(cmd)?;
        }
        Ok(())
    }

    fn ext_cmd(&mut self, cmd: &str) -> Result<(), GerberError> {
        let code: String = cmd.chars().take(2).collect();
        match code.as_str() {
            "FS" => {
                let b: Vec<char> = cmd.chars().collect();
                let mut i = 2usize;
                let zero = if i < b.len() && (b[i] == 'L' || b[i] == 'T') {
                    let c = b[i];
                    i += 1;
                    c
                } else {
                    ' '
                };
                if i < b.len() && (b[i] == 'A' || b[i] == 'I') {
                    self.incremental = b[i] == 'I';
                    i += 1;
                } else {
                    self.incremental = false;
                }
                if i + 5 >= b.len()
                    || b[i] != 'X'
                    || !b[i + 1].is_ascii_digit()
                    || !b[i + 2].is_ascii_digit()
                    || b[i + 3] != 'Y'
                    || !b[i + 4].is_ascii_digit()
                    || !b[i + 5].is_ascii_digit()
                {
                    return err(format!("bad format spec {cmd:?}"));
                }
                self.trailing_zero_omission = zero == 'T';
                self.int_digits = b[i + 1] as usize - '0' as usize;
                self.dec_digits = b[i + 2] as usize - '0' as usize;
            }
            "MO" => {
                let unit: String = cmd.chars().skip(2).take(2).collect();
                self.scale = if unit == "IN" { 25.4 } else { 1.0 };
            }
            "AD" => self.define_aperture(cmd)?,
            "LP" => {
                self.dark = cmd.chars().nth(2) != Some('C');
            }
            "TF" => {
                let (k, v) = split_attr(&cmd[2..]);
                self.gf.file_attrs.insert(k, v);
            }
            "TA" => {
                let (k, v) = split_attr(&cmd[2..]);
                self.cur_ta.insert(k, v);
            }
            "TO" => {
                let (k, v) = split_attr(&cmd[2..]);
                self.cur_to.insert(k, v);
            }
            "TD" => {
                let name = cmd[2..].trim_start_matches('.');
                if name.is_empty() {
                    self.cur_ta.clear();
                    self.cur_to.clear();
                } else {
                    self.cur_ta.remove(name);
                    self.cur_to.remove(name);
                }
            }
            "LM" | "LR" | "LS" => {
                let val = &cmd[2..];
                let bad = match code.as_str() {
                    "LM" => val != "N",
                    "LR" => {
                        if val.is_empty() {
                            false
                        } else {
                            val.trim().parse::<f64>().map(|v| v != 0.0).unwrap_or(true)
                        }
                    }
                    _ => {
                        if val.is_empty() {
                            false
                        } else {
                            val.trim().parse::<f64>().map(|v| v != 1.0).unwrap_or(true)
                        }
                    }
                };
                if bad {
                    return err(format!("aperture transformation {cmd:?} is not supported"));
                }
            }
            "SR" => {
                if has_repeat(cmd) {
                    return err("step and repeat (SR) is not supported");
                }
            }
            "AS" => {
                let v = cmd[2..].trim();
                if !v.is_empty() && v != "AXBY" {
                    return err(format!("axis select {cmd:?} is not supported"));
                }
            }
            "OF" | "SF" | "MI" if has_nonzero_offset(cmd) => {
                return err(format!(
                    "deprecated transformation {cmd:?} is not supported"
                ));
            }
            _ => {} // IP, IN, LN, IJ, IR ... are ignored
        }
        Ok(())
    }

    fn define_aperture(&mut self, cmd: &str) -> Result<(), GerberError> {
        let (code, template, raw) = match parse_ad(cmd) {
            Some(v) => v,
            None => return err(format!("bad aperture definition {cmd:?}")),
        };
        let parse_mods = |scale: f64| -> Vec<f64> {
            match &raw {
                Some(s) => s
                    .split('X')
                    .map(|v| v.trim().parse::<f64>().unwrap_or(0.0) * scale)
                    .collect(),
                None => Vec::new(),
            }
        };
        let mut macro_def = None;
        let mods = if matches!(template.as_str(), "C" | "R" | "O" | "P") {
            parse_mods(self.scale)
        } else {
            match self.gf.macros.get(&template) {
                Some(m) => macro_def = Some(Rc::clone(m)),
                None => return err(format!("aperture D{code} uses unknown macro {template:?}")),
            }
            // macro parameters are not lengths in every position; KiCad only
            // uses mm so we simply do not scale them
            parse_mods(1.0)
        };
        self.gf.apertures.insert(
            code,
            Rc::new(Aperture::new(
                code,
                &template,
                mods,
                self.cur_ta.clone(),
                macro_def,
            )),
        );
        Ok(())
    }

    fn coord(&self, raw: &str) -> f64 {
        let neg = raw.starts_with('-');
        let digits = raw.trim_start_matches(['+', '-']);
        let val = if digits.contains('.') {
            digits.parse::<f64>().unwrap_or(0.0)
        } else {
            let mut d = digits.to_string();
            if self.trailing_zero_omission {
                let want = self.int_digits + self.dec_digits;
                while d.len() < want {
                    d.push('0');
                }
            }
            let n: i64 = if d.is_empty() {
                0
            } else {
                d.parse().unwrap_or(0)
            };
            (n as f64) / 10f64.powi(self.dec_digits as i32)
        };
        (if neg { -val } else { val }) * self.scale
    }

    fn data(&mut self, block: &str) -> Result<(), GerberError> {
        if block.is_empty() {
            return Ok(());
        }
        if block.starts_with("G04") || block.starts_with("G4 ") {
            return Ok(());
        }
        if matches!(block, "M02" | "M00" | "M2" | "M0") {
            self.done = true;
            return Ok(());
        }
        let (mut x, mut y): (Option<f64>, Option<f64>) = (None, None);
        let (mut i, mut j) = (0.0f64, 0.0f64);
        let mut op: Option<i32> = None;
        for (letter, num) in tokens(block) {
            match letter {
                'G' => {
                    let g: i32 = if num.is_empty() {
                        0
                    } else {
                        num.parse().unwrap_or(0)
                    };
                    match g {
                        1..=3 => self.interp = g as u8,
                        36 => self.begin_region(),
                        37 => self.end_region(),
                        74 => self.multi_quadrant = false,
                        75 => self.multi_quadrant = true,
                        70 => self.scale = 25.4,
                        71 => self.scale = 1.0,
                        91 => self.incremental = true,
                        90 => self.incremental = false,
                        _ => {} // G54/G55 are harmless prefixes
                    }
                }
                'X' => x = Some(self.coord(&num)),
                'Y' => y = Some(self.coord(&num)),
                'I' => i = self.coord(&num),
                'J' => j = self.coord(&num),
                'D' => {
                    let d: i32 = if num.is_empty() {
                        0
                    } else {
                        num.parse().unwrap_or(0)
                    };
                    if d >= 10 {
                        match self.gf.apertures.get(&(d as u32)) {
                            Some(ap) => self.aperture = Some(Rc::clone(ap)),
                            None => return err(format!("undefined aperture D{d}")),
                        }
                    } else {
                        op = Some(d);
                    }
                }
                'M' => {
                    let m: i32 = if num.is_empty() {
                        0
                    } else {
                        num.parse().unwrap_or(0)
                    };
                    if m == 0 || m == 2 {
                        self.done = true;
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
        if op.is_none() {
            if x.is_some() || y.is_some() {
                // bare coordinates: deprecated, means repeat last op (D01)
                op = Some(1);
            } else {
                return Ok(());
            }
        }
        let nx = match x {
            None => self.x,
            Some(v) => {
                if self.incremental {
                    self.x + v
                } else {
                    v
                }
            }
        };
        let ny = match y {
            None => self.y,
            Some(v) => {
                if self.incremental {
                    self.y + v
                } else {
                    v
                }
            }
        };
        match op {
            Some(2) => {
                if self.in_region {
                    self.close_contour();
                }
                self.x = nx;
                self.y = ny;
            }
            Some(1) => {
                let seg = self.segment(nx, ny, i, j);
                if self.in_region {
                    if self.contours.is_empty() {
                        self.contours.push(Vec::new());
                    }
                    self.contours.last_mut().unwrap().push(seg);
                } else {
                    let ap = match &self.aperture {
                        Some(a) => Rc::clone(a),
                        None => return err("D01 without a selected aperture"),
                    };
                    self.gf.objects.push(GraphicObject::Stroke(Stroke {
                        seg,
                        aperture: ap,
                        attrs: self.cur_to.clone(),
                        dark: self.dark,
                    }));
                }
                self.x = nx;
                self.y = ny;
            }
            Some(3) => {
                let ap = match &self.aperture {
                    Some(a) => Rc::clone(a),
                    None => return err("D03 without a selected aperture"),
                };
                self.x = nx;
                self.y = ny;
                self.gf.objects.push(GraphicObject::Flash(Flash {
                    x: nx,
                    y: ny,
                    aperture: ap,
                    attrs: self.cur_to.clone(),
                    dark: self.dark,
                }));
            }
            _ => {}
        }
        Ok(())
    }

    fn segment(&self, nx: f64, ny: f64, i: f64, j: f64) -> Segment {
        if self.interp == 1 {
            return Segment::line(self.x, self.y, nx, ny);
        }
        let cw = self.interp == 2;
        let (cx, cy) = if self.multi_quadrant {
            (self.x + i, self.y + j)
        } else {
            self.single_quadrant_centre(nx, ny, i.abs(), j.abs(), cw)
        };
        Segment {
            x0: self.x,
            y0: self.y,
            x1: nx,
            y1: ny,
            arc: true,
            cx,
            cy,
            clockwise: cw,
        }
    }

    fn single_quadrant_centre(&self, nx: f64, ny: f64, i: f64, j: f64, cw: bool) -> (f64, f64) {
        let mut best = (self.x + i, self.y + j);
        let mut best_err = f64::INFINITY;
        for sx in [1.0f64, -1.0] {
            for sy in [1.0f64, -1.0] {
                let cx = self.x + sx * i;
                let cy = self.y + sy * j;
                let r0 = (self.x - cx).hypot(self.y - cy);
                let r1 = (nx - cx).hypot(ny - cy);
                let sweep = arc_sweep(self.x, self.y, nx, ny, cx, cy, cw);
                if sweep.abs() > std::f64::consts::FRAC_PI_2 + 1e-6 {
                    continue;
                }
                let e = (r0 - r1).abs();
                if e < best_err {
                    best = (cx, cy);
                    best_err = e;
                }
            }
        }
        best
    }

    fn begin_region(&mut self) {
        self.in_region = true;
        self.contours = Vec::new();
        self.region_attrs = self.cur_to.clone();
    }

    fn close_contour(&mut self) {
        if self.contours.last().is_some_and(|c| !c.is_empty()) {
            self.contours.push(Vec::new());
        }
    }

    fn end_region(&mut self) {
        let contours: Vec<Vec<Segment>> = self
            .contours
            .iter()
            .filter(|c| c.len() >= 2)
            .cloned()
            .collect();
        if !contours.is_empty() {
            self.gf.objects.push(GraphicObject::Region(Region {
                contours,
                attrs: self.region_attrs.clone(),
                dark: self.dark,
            }));
        }
        self.in_region = false;
        self.contours = Vec::new();
    }
}

fn split_attr(body: &str) -> (String, String) {
    let body = body.trim_start_matches('.');
    match body.split_once(',') {
        Some((k, v)) => (k.trim().to_string(), v.trim().to_string()),
        None => (body.trim().to_string(), String::new()),
    }
}

/// Parse a gerber file's text.
pub fn parse_gerber(text: &str, name: &str) -> Result<GerberFile, GerberError> {
    Parser::new().parse(text, name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::Area;

    fn area(g: &Geom) -> f64 {
        g.unsigned_area()
    }

    #[test]
    fn macro_expression_grammar() {
        let mut vars = BTreeMap::new();
        vars.insert(1i64, 3.0);
        vars.insert(2i64, 4.0);
        let eval = |s: &str| Expr::new(s, &vars).parse().unwrap();
        assert_eq!(eval("$1+$2"), 7.0);
        assert_eq!(eval("$1 + $2"), 7.0);
        assert_eq!(eval("2x$1"), 6.0);
        assert_eq!(eval("2X$1"), 6.0);
        assert_eq!(eval("$2/2"), 2.0);
        assert_eq!(eval("-$1"), -3.0);
        assert_eq!(eval("+$1"), 3.0);
        assert_eq!(eval("(1+2)x3"), 9.0);
        assert_eq!(eval("1+2x3"), 7.0);
        assert_eq!(eval("$9"), 0.0); // undefined variable
        assert_eq!(eval("0.5"), 0.5);
        assert_eq!(eval(" ( $1 + 1 ) / 2 "), 2.0);
        assert!(Expr::new("1+", &vars).parse().is_err());
        assert!(Expr::new("(1", &vars).parse().is_err());
        assert!(Expr::new("1)", &vars).parse().is_err());
        assert!(Expr::new("@", &vars).parse().is_err());
    }

    #[test]
    fn macro_variables_and_prims() {
        let m = Macro {
            name: "T".into(),
            blocks: vec![
                "0 comment".into(),
                "$3=$1x2".into(),
                "1,1,$3,0,0".into(),
                "21,1,$1,$2,0,0,0".into(),
            ],
        };
        let prims = m.evaluate(&[1.0, 2.0]).unwrap();
        assert_eq!(prims.len(), 2);
        assert_eq!(
            prims[0],
            MacroPrim::Circle {
                exposure: true,
                d: 2.0,
                cx: 0.0,
                cy: 0.0
            }
        );
        match &prims[1] {
            MacroPrim::Outline { points, .. } => assert_eq!(points.len(), 4),
            _ => panic!("expected outline"),
        }
    }

    #[test]
    fn macro_prim_as_gerber() {
        let c = MacroPrim::Circle {
            exposure: true,
            d: 0.5,
            cx: -0.25,
            cy: 0.0,
        };
        assert_eq!(c.as_gerber(), "1,1,0.500000,-0.250000,0.000000*");
        let o = MacroPrim::Outline {
            exposure: false,
            points: vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 0.0)],
        };
        // the closing point is dropped from the count but re-appended
        assert_eq!(
            o.as_gerber(),
            "4,0,3,0.000000,0.000000,1.000000,0.000000,1.000000,1.000000,0.000000,0.000000,0*"
        );
    }

    #[test]
    fn unsupported_commands_rejected() {
        let base = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.1*%\n";
        assert!(parse_gerber(&format!("{base}%ASAYBX*%\nM02*\n"), "t").is_err());
        assert!(parse_gerber(&format!("{base}%SRX2Y1I1J1*%\nM02*\n"), "t").is_err());
        assert!(parse_gerber(&format!("{base}%SRX1Y10I1J1*%\nM02*\n"), "t").is_err());
        assert!(parse_gerber(&format!("{base}%LMXY*%\nM02*\n"), "t").is_err());
        assert!(parse_gerber(&format!("{base}%LR90*%\nM02*\n"), "t").is_err());
        assert!(parse_gerber(&format!("{base}%LS2*%\nM02*\n"), "t").is_err());
        assert!(parse_gerber(&format!("{base}%MIA1B0*%\nM02*\n"), "t").is_err());
        assert!(parse_gerber(&format!("{base}%OFA0.1B0*%\nM02*\n"), "t").is_err());
        // accepted forms
        assert!(parse_gerber(&format!("{base}%ASAXBY*%\nM02*\n"), "t").is_ok());
        assert!(parse_gerber(&format!("{base}%SRX1Y1I0J0*%\nM02*\n"), "t").is_ok());
        assert!(parse_gerber(&format!("{base}%LMN*%\nM02*\n"), "t").is_ok());
        assert!(parse_gerber(&format!("{base}%LR0*%\nM02*\n"), "t").is_ok());
        assert!(parse_gerber(&format!("{base}%LS1*%\nM02*\n"), "t").is_ok());
        assert!(parse_gerber(&format!("{base}%MIA0B0*%\nM02*\n"), "t").is_ok());
        assert!(parse_gerber(&format!("{base}%OFA0B0.0*%\nM02*\n"), "t").is_ok());
    }

    #[test]
    fn trailing_zero_omission() {
        // FSTAX23: 2 integer + 3 decimal digits, trailing zeros omitted
        let text = "%FSTAX23Y23*%\n%MOMM*%\n%ADD10C,0.1*%\nD10*\nX1Y-25D03*\nM02*\n";
        let gf = parse_gerber(text, "t").unwrap();
        match &gf.objects[0] {
            GraphicObject::Flash(f) => {
                assert!((f.x - 10.0).abs() < 1e-9, "x={}", f.x);
                assert!((f.y + 25.0).abs() < 1e-9, "y={}", f.y);
            }
            _ => panic!("expected flash"),
        }
        // leading zero omission (the default) reads the same digits as 1e-3 mm
        let text = "%FSLAX23Y23*%\n%MOMM*%\n%ADD10C,0.1*%\nD10*\nX1Y-25D03*\nM02*\n";
        let gf = parse_gerber(text, "t").unwrap();
        match &gf.objects[0] {
            GraphicObject::Flash(f) => {
                assert!((f.x - 0.001).abs() < 1e-12);
                assert!((f.y + 0.025).abs() < 1e-12);
            }
            _ => panic!("expected flash"),
        }
    }

    #[test]
    fn inch_units() {
        let text = "%FSLAX24Y24*%\n%MOIN*%\n%ADD10C,0.1*%\nD10*\nX10000Y0D03*\nM02*\n";
        let gf = parse_gerber(text, "t").unwrap();
        let ap = gf.apertures.get(&10).unwrap();
        assert!((ap.modifiers[0] - 2.54).abs() < 1e-9);
        match &gf.objects[0] {
            GraphicObject::Flash(f) => assert!((f.x - 25.4).abs() < 1e-9),
            _ => panic!("expected flash"),
        }
    }

    #[test]
    fn single_quadrant_arc() {
        // quarter circle from (1,0) to (0,1) around the origin, counter clockwise
        let text = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.1*%\nD10*\nG74*\nG03*\n\
                    X1000000Y0D02*\nX0Y1000000I1000000J0D01*\nM02*\n";
        let gf = parse_gerber(text, "t").unwrap();
        match &gf.objects[0] {
            GraphicObject::Stroke(s) => {
                assert!(s.seg.arc);
                assert!(s.seg.cx.abs() < 1e-9, "cx={}", s.seg.cx);
                assert!(s.seg.cy.abs() < 1e-9, "cy={}", s.seg.cy);
                let pts = s.seg.points();
                assert!(pts.len() > 3);
                for p in &pts {
                    assert!((p.0.hypot(p.1) - 1.0).abs() < 1e-6);
                }
            }
            _ => panic!("expected stroke"),
        }
        // multi quadrant (G75) uses the offset directly
        let text = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.1*%\nD10*\nG75*\nG03*\n\
                    X1000000Y0D02*\nX0Y1000000I-1000000J0D01*\nM02*\n";
        let gf = parse_gerber(text, "t").unwrap();
        match &gf.objects[0] {
            GraphicObject::Stroke(s) => {
                assert!(s.seg.cx.abs() < 1e-9);
                assert!(s.seg.cy.abs() < 1e-9);
            }
            _ => panic!("expected stroke"),
        }
    }

    #[test]
    fn arc_sweep_full_circle() {
        let s = arc_sweep(1.0, 0.0, 1.0, 0.0, 0.0, 0.0, false);
        assert!((s - TWO_PI).abs() < 1e-12);
        let s = arc_sweep(1.0, 0.0, 1.0, 0.0, 0.0, 0.0, true);
        assert!((s + TWO_PI).abs() < 1e-12);
        let s = arc_sweep(1.0, 0.0, 0.0, 1.0, 0.0, 0.0, false);
        assert!((s - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
    }

    #[test]
    fn circle_matches_shapely_discretisation() {
        let c = circle(0.0, 0.0, 0.5);
        assert_eq!(c.0[0].exterior().0.len(), 97);
        // shapely: Point(0,0).buffer(0.5, quad_segs=24).area
        assert!((area(&c) - 0.7848375507617169).abs() < 1e-12);
    }

    #[test]
    fn capsule_matches_shapely_buffer() {
        let g = buffer_path(&[(-1.0, 0.0), (1.0, 0.0)], 0.25);
        // shapely: LineString([(-1,0),(1,0)]).buffer(0.25, quad_segs=24).area
        assert!(
            (area(&g) - 1.1962093876904292).abs() < 1e-12,
            "{}",
            area(&g)
        );
        assert_eq!(g.0[0].exterior().0.len(), 99);
    }

    #[test]
    fn aperture_describe_and_geometry() {
        let c = Aperture::new(10, "C", vec![0.2], BTreeMap::new(), None);
        assert_eq!(c.describe(), "C ⌀0.20");
        let r = Aperture::new(11, "R", vec![0.28, 0.52], BTreeMap::new(), None);
        assert_eq!(r.describe(), "R 0.28x0.52");
        assert!((r.geometry().unsigned_area() - 0.28 * 0.52).abs() < 1e-12);
        let o = Aperture::new(12, "O", vec![1.0, 0.5], BTreeMap::new(), None);
        assert_eq!(o.describe(), "O 1.00x0.50");
        let p = Aperture::new(13, "P", vec![1.0, 6.0], BTreeMap::new(), None);
        assert_eq!(p.describe(), "P ⌀1.00 n=6");
        // hole subtraction
        let ch = Aperture::new(14, "C", vec![1.0, 0.5], BTreeMap::new(), None);
        let a = ch.geometry().unsigned_area();
        let full = circle(0.0, 0.0, 0.5).unsigned_area();
        let hole = circle(0.0, 0.0, 0.25).unsigned_area();
        assert!((a - (full - hole)).abs() < 1e-9);
    }

    #[test]
    fn bake_aperture_mirrors() {
        let p = Aperture::new(10, "P", vec![1.0, 6.0, 30.0], BTreeMap::new(), None);
        assert_eq!(bake_aperture(&p, false).modifiers[2], 30.0);
        assert_eq!(bake_aperture(&p, true).modifiers[2], 150.0);
        let p2 = Aperture::new(10, "P", vec![1.0, 6.0], BTreeMap::new(), None);
        assert_eq!(bake_aperture(&p2, true).modifiers, vec![1.0, 6.0, 180.0]);
        let mut attrs = BTreeMap::new();
        attrs.insert("AperFunction".to_string(), "SMDPad,CuDef".to_string());
        let c = Aperture::new(11, "C", vec![0.2], attrs, None);
        assert_eq!(
            bake_aperture(&c, false).key(),
            "C,0.200000|AperFunction=SMDPad,CuDef"
        );
        assert_eq!(c.function(), "SMDPad");
    }

    #[test]
    fn region_with_cut_in_becomes_hole() {
        // a square with a self-touching notch cut back to the outer ring
        let pts = vec![
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 6.0),
            (6.0, 6.0),
            (6.0, 4.0),
            (0.0, 4.0),
        ];
        let g = poly_of(&pts);
        assert!(
            (g.unsigned_area() - (100.0 - 12.0)).abs() < 1e-9,
            "{}",
            g.unsigned_area()
        );
    }

    #[test]
    fn region_object_geometry() {
        let text = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.1*%\nD10*\nG36*\n\
                    X0Y0D02*\nG01*\nX1000000Y0D01*\nX1000000Y1000000D01*\nX0Y1000000D01*\nX0Y0D01*\n\
                    G37*\nM02*\n";
        let gf = parse_gerber(text, "t").unwrap();
        assert_eq!(gf.objects.len(), 1);
        let g = object_geometry(&gf.objects[0]);
        assert!((g.unsigned_area() - 1.0).abs() < 1e-9);
        assert_eq!(gf.bounds(), Some((0.0, 0.0, 1.0, 1.0)));
    }

    #[test]
    fn polarity_and_attributes() {
        let text = "%TF.FileFunction,Paste,Top*%\n%FSLAX46Y46*%\n%MOMM*%\n\
                    %TA.AperFunction,SMDPad,CuDef*%\n%ADD10C,0.2*%\n%TD*%\n\
                    D10*\n%TO.C,U1*%\nX0Y0D03*\n%LPC*%\nX1000000Y0D03*\n%TD*%\nM02*\n";
        let gf = parse_gerber(text, "t").unwrap();
        assert_eq!(gf.file_function(), "Paste,Top");
        assert_eq!(gf.objects.len(), 2);
        assert!(gf.objects[0].dark());
        assert!(!gf.objects[1].dark());
        assert_eq!(
            gf.objects[0].attrs().get("C").map(String::as_str),
            Some("U1")
        );
        assert_eq!(
            gf.apertures[&10]
                .attrs
                .get("AperFunction")
                .map(String::as_str),
            Some("SMDPad,CuDef")
        );
    }

    #[test]
    fn deprecated_bare_coordinates_repeat_d01() {
        let text = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.1*%\nD10*\nX0Y0D02*\nX1000000Y0*\nM02*\n";
        let gf = parse_gerber(text, "t").unwrap();
        assert_eq!(gf.objects.len(), 1);
        matches!(&gf.objects[0], GraphicObject::Stroke(_));
    }

    #[test]
    fn stroke_non_round_aperture_uses_hull() {
        let text =
            "%FSLAX46Y46*%\n%MOMM*%\n%ADD10R,1.0X1.0*%\nD10*\nX0Y0D02*\nX2000000Y0D01*\nM02*\n";
        let gf = parse_gerber(text, "t").unwrap();
        let g = object_geometry(&gf.objects[0]);
        // 1x1 square swept 2 mm along x -> 3 x 1 rectangle
        assert!(
            (g.unsigned_area() - 3.0).abs() < 1e-9,
            "{}",
            g.unsigned_area()
        );
    }
}
