//! Half-block rasteriser: a terminal picture of the stencil sheet around one
//! pad, for the TUI's split view (`v`).
//!
//! The picture lives in *sheet* coordinates (the same space the preview PNG
//! draws): a [`Raster`] is a window of `cols x subrows` square sub-pixels
//! centred on the selected pad, where two vertically stacked sub-pixels share
//! one character cell and are drawn with `▀` / `▄` / `█` and a foreground and
//! background colour. A terminal cell is about twice as tall as it is wide, so
//! half a cell is roughly square and one millimetre is the same number of
//! screen pixels across and down.
//!
//! Every sub-pixel carries the [`Class`] of the last thing painted over it;
//! the classes are painted in the order of the PNG preview (copper, pads,
//! board outline, ignored, openings, undefined, the selected pad), so a later
//! class always wins - exactly like the PNG's compositing.
//!
//! The expensive part is [`object_geometry`], which runs boolean operations for
//! regions: it is computed once per side into a [`GeomCache`], indexed for
//! point queries ([`Shape`]) and then only *sampled*. Sampling asks the shape
//! whether it contains a sub-pixel centre, so no geometry is ever transformed:
//! the sample point is mapped back to board coordinates instead (a mirror and
//! a translation).

use std::collections::HashMap;

use geo::indexed::IntervalTreeMultiPolygon;
use geo::{Contains, Coord};
use ratatui::style::Color;

use crate::gerber::{contour_points, geom_bounds, object_geometry, GraphicObject};
use crate::layout::marker_strokes;
use crate::model::{
    Area, Bounds, Geom, Layout, Project, Side, SideId, Transform, STATE_IGNORE, STATE_OPEN,
    STATE_UNDEFINED,
};

/// What a sub-pixel shows. The order is the painting order of the preview PNG:
/// a class painted later covers every class before it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    /// nothing at all (the terminal background)
    #[default]
    Empty,
    /// bare copper
    Copper,
    /// a pad flash on copper
    Pad,
    /// the board outline (or its bounding box)
    Outline,
    /// an ignored candidate, or a paste opening the user closed
    Ignored,
    /// anything that is cut: paste, opened pads, dots, slots, holes, the marker
    Opening,
    /// a candidate pad that still needs a decision
    Undefined,
    /// the pad under the cursor
    Selected,
}

/// The colour of a class - the very same values the preview PNG uses.
pub fn class_color(class: Class) -> Color {
    match class {
        Class::Empty => Color::Reset,
        Class::Copper => Color::Rgb(0x2e, 0x2e, 0x2e),
        Class::Pad => Color::Rgb(0x5a, 0x5a, 0x5a),
        Class::Outline => Color::Rgb(0xc8, 0xc8, 0xc8),
        Class::Ignored => Color::Rgb(0x3a, 0x5f, 0xcd),
        Class::Opening => Color::Rgb(0xff, 0x2a, 0x2a),
        Class::Undefined => Color::Rgb(0xff, 0xd0, 0x00),
        Class::Selected => Color::Rgb(0x00, 0xe5, 0xff),
    }
}

/// Upper half block: its foreground is the top sub-pixel.
pub const UPPER_HALF: &str = "▀";
/// Lower half block: its foreground is the bottom sub-pixel.
pub const LOWER_HALF: &str = "▄";
/// Full block: both sub-pixels have the same colour.
pub const FULL_BLOCK: &str = "█";

/// The character and the `(foreground, background)` of one character cell.
///
/// An empty sub-pixel is left to the terminal background (`Color::Reset`), so
/// the panel blends into whatever colour scheme the user runs.
pub fn half_block(top: Class, bottom: Class) -> (&'static str, Color, Color) {
    match (top, bottom) {
        (Class::Empty, Class::Empty) => (" ", Color::Reset, Color::Reset),
        (t, Class::Empty) => (UPPER_HALF, class_color(t), Color::Reset),
        (Class::Empty, b) => (LOWER_HALF, class_color(b), Color::Reset),
        (t, b) if t == b => (FULL_BLOCK, class_color(t), Color::Reset),
        (t, b) => (UPPER_HALF, class_color(t), class_color(b)),
    }
}

/// The selected pad's larger dimension spans about this share of the panel.
pub const ZOOM_SPAN: f64 = 0.25;
/// ... but never fewer than this many character columns.
pub const MIN_SPAN_COLS: f64 = 3.0;

/// Millimetres per character column (= per sub-pixel) for a pad of `pad_size`
/// mm on a panel `cols` columns wide.
///
/// The pad spans [`ZOOM_SPAN`] of the panel, clamped to at least
/// [`MIN_SPAN_COLS`] columns and at most the whole panel.
pub fn zoom_step(pad_size: f64, cols: usize) -> f64 {
    let cols = cols.max(1) as f64;
    let span = (cols * ZOOM_SPAN).clamp(MIN_SPAN_COLS.min(cols), cols);
    let size = if pad_size.is_finite() && pad_size > 1e-9 {
        pad_size
    } else {
        1.0
    };
    size / span
}

/// A bounding box that intersects nothing.
const NOWHERE: Bounds = (1.0, 1.0, -1.0, -1.0);

/// Board -> sheet for a bounding box (a mirror swaps its two x values).
pub fn sheet_bounds(b: Bounds, xf: &Transform) -> Bounds {
    let (x0, y0) = xf.apply(b.0, b.1);
    let (x1, y1) = xf.apply(b.2, b.3);
    (x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1))
}

/// Sheet -> board: the inverse of [`Transform::apply`].
fn unapply(xf: &Transform, x: f64, y: f64) -> (f64, f64) {
    let bx = x - xf.dx;
    ((if xf.mirror { -bx } else { bx }), y - xf.dy)
}

// --------------------------------------------------------------------------- //
// The geometry cache
// --------------------------------------------------------------------------- //

/// One geometry, ready to be asked about single points.
///
/// The interval tree turns point containment from a walk over every edge into
/// a lookup, which is what makes a big copper pour affordable: a frame samples
/// one sub-pixel centre at a time, tens of thousands of times over.
pub struct Shape {
    index: IntervalTreeMultiPolygon<f64>,
    /// bounding box, board coordinates ([`NOWHERE`] when the geometry is empty)
    pub bounds: Bounds,
    empty: bool,
}

impl Shape {
    pub fn new(geom: &Geom) -> Self {
        Self {
            index: IntervalTreeMultiPolygon::new(geom),
            bounds: geom_bounds(geom).unwrap_or(NOWHERE),
            empty: geom.0.is_empty(),
        }
    }
    /// Is `(x, y)` - board coordinates - inside?
    pub fn contains(&self, x: f64, y: f64) -> bool {
        !self.empty && self.index.contains(&Coord { x, y })
    }
}

impl std::fmt::Debug for Shape {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shape")
            .field("bounds", &self.bounds)
            .finish()
    }
}

/// Everything of one side the rasteriser needs, in board coordinates.
///
/// Built once per side and kept: the objects never change, only their classes
/// do (a pad state is read from the live `Pad`, never from here).
#[derive(Debug, Default)]
pub struct SideCache {
    /// copper objects
    pub copper: Vec<Shape>,
    /// paste objects, indexed like `Side::paste_objects`
    pub paste: Vec<Shape>,
    /// pad shapes, indexed like `Side::pads`
    pub pads: Vec<Shape>,
    /// board outline polylines (empty when the project has no outline layer)
    pub outline: Vec<Vec<(f64, f64)>>,
}

/// One [`SideCache`] per side, filled lazily.
pub type GeomCache = HashMap<SideId, SideCache>;

fn prepare(objects: &[GraphicObject]) -> Vec<Shape> {
    objects
        .iter()
        .map(|obj| Shape::new(&object_geometry(obj)))
        .collect()
}

/// Board outline polylines in *board* coordinates (empty without an outline).
fn outline_paths(projects: &[Project], id: SideId) -> Vec<Vec<(f64, f64)>> {
    let Some(outline) = projects[id.project].outline.as_ref() else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    for obj in &outline.objects {
        match obj {
            // a flash on an outline layer carries no shape information
            GraphicObject::Flash(_) => {}
            GraphicObject::Stroke(s) => {
                let pts = s.seg.points();
                if pts.len() >= 2 {
                    paths.push(pts);
                }
            }
            GraphicObject::Region(r) => {
                for contour in &r.contours {
                    let mut ring = contour_points(contour);
                    if ring.len() >= 3 {
                        if ring[0] != ring[ring.len() - 1] {
                            ring.push(ring[0]);
                        }
                        paths.push(ring);
                    }
                }
            }
        }
    }
    paths
}

fn build_cache(projects: &[Project], id: SideId) -> SideCache {
    let side: &Side = id.get(projects);
    SideCache {
        copper: prepare(side.copper_objects()),
        paste: prepare(side.paste_objects()),
        pads: side.pads.iter().map(|pad| Shape::new(&pad.geom)).collect(),
        outline: outline_paths(projects, id),
    }
}

/// The cache entry of `id`, built on first use.
pub fn side_entry<'a>(cache: &'a mut GeomCache, projects: &[Project], id: SideId) -> &'a SideCache {
    cache.entry(id).or_insert_with(|| build_cache(projects, id))
}

// --------------------------------------------------------------------------- //
// The raster
// --------------------------------------------------------------------------- //

/// A window of the sheet as square sub-pixels, two per character row.
#[derive(Clone, Debug)]
pub struct Raster {
    /// character columns = sub-pixel columns
    pub cols: usize,
    /// sub-pixel rows; always `2 * rows`
    pub subrows: usize,
    /// left edge of the window, sheet mm
    pub minx: f64,
    /// top edge of the window, sheet mm
    pub maxy: f64,
    /// millimetres per sub-pixel, the same across and down
    pub step: f64,
    /// the caption line above the picture
    pub caption: String,
    cells: Vec<Class>,
}

impl Raster {
    fn new(cols: usize, rows: usize, centre: (f64, f64), step: f64) -> Self {
        let subrows = rows * 2;
        Self {
            cols,
            subrows,
            minx: centre.0 - cols as f64 * step / 2.0,
            maxy: centre.1 + subrows as f64 * step / 2.0,
            step,
            caption: String::new(),
            cells: vec![Class::Empty; cols * subrows],
        }
    }

    /// Character rows.
    pub fn rows(&self) -> usize {
        self.subrows / 2
    }
    /// Window width, sheet mm.
    pub fn width_mm(&self) -> f64 {
        self.cols as f64 * self.step
    }
    /// Window height, sheet mm.
    pub fn height_mm(&self) -> f64 {
        self.subrows as f64 * self.step
    }
    /// Window centre, sheet mm.
    pub fn centre(&self) -> (f64, f64) {
        (
            self.minx + self.width_mm() / 2.0,
            self.maxy - self.height_mm() / 2.0,
        )
    }
    /// The class of one sub-pixel (`Empty` outside the window).
    pub fn at(&self, col: usize, subrow: usize) -> Class {
        if col >= self.cols || subrow >= self.subrows {
            return Class::Empty;
        }
        self.cells[subrow * self.cols + col]
    }
    /// The character and colours of one character cell.
    pub fn cell(&self, col: usize, row: usize) -> (&'static str, Color, Color) {
        half_block(self.at(col, row * 2), self.at(col, row * 2 + 1))
    }

    /// Sheet x of the centre of column `col`.
    pub fn x_of(&self, col: usize) -> f64 {
        self.minx + (col as f64 + 0.5) * self.step
    }
    /// Sheet y of the centre of sub-row `subrow`.
    pub fn y_of(&self, subrow: usize) -> f64 {
        self.maxy - (subrow as f64 + 0.5) * self.step
    }

    fn put(&mut self, col: usize, subrow: usize, class: Class) {
        if col < self.cols && subrow < self.subrows {
            self.cells[subrow * self.cols + col] = class;
        }
    }

    /// The `(col0, col1, row0, row1)` half-open sub-pixel range a sheet
    /// bounding box covers, or None when it is outside the window.
    fn clip(&self, b: Bounds) -> Option<(usize, usize, usize, usize)> {
        let (minx, miny, maxx, maxy) = b;
        if !(minx <= maxx && miny <= maxy) {
            return None; // empty or NaN
        }
        let (c0, c1) = span(
            (minx - self.minx) / self.step - 0.5,
            (maxx - self.minx) / self.step - 0.5,
            self.cols,
        )?;
        let (r0, r1) = span(
            (self.maxy - maxy) / self.step - 0.5,
            (self.maxy - miny) / self.step - 0.5,
            self.subrows,
        )?;
        Some((c0, c1, r0, r1))
    }

    /// Paint every sub-pixel whose centre is inside `shape` (board coordinates).
    fn fill_shape(&mut self, shape: &Shape, xf: &Transform, class: Class) {
        let Some((c0, c1, r0, r1)) = self.clip(sheet_bounds(shape.bounds, xf)) else {
            return;
        };
        for subrow in r0..r1 {
            let y = self.y_of(subrow);
            for col in c0..c1 {
                let (bx, by) = unapply(xf, self.x_of(col), y);
                if shape.contains(bx, by) {
                    self.put(col, subrow, class);
                }
            }
        }
    }

    /// Paint a filled circle, sheet coordinates.
    fn fill_circle(&mut self, cx: f64, cy: f64, r: f64, class: Class) {
        if r <= 0.0 || !r.is_finite() {
            return;
        }
        let Some((c0, c1, r0, r1)) = self.clip((cx - r, cy - r, cx + r, cy + r)) else {
            return;
        };
        let rr = r * r;
        for subrow in r0..r1 {
            let dy = self.y_of(subrow) - cy;
            for col in c0..c1 {
                let dx = self.x_of(col) - cx;
                if dx * dx + dy * dy <= rr {
                    self.put(col, subrow, class);
                }
            }
        }
    }

    /// Paint a segment buffered by `r` (round ends), sheet coordinates.
    fn fill_capsule(&mut self, a: (f64, f64), b: (f64, f64), r: f64, class: Class) {
        if r <= 0.0 || !r.is_finite() {
            return;
        }
        let bounds = (
            a.0.min(b.0) - r,
            a.1.min(b.1) - r,
            a.0.max(b.0) + r,
            a.1.max(b.1) + r,
        );
        let Some((c0, c1, r0, r1)) = self.clip(bounds) else {
            return;
        };
        let rr = r * r;
        for subrow in r0..r1 {
            let y = self.y_of(subrow);
            for col in c0..c1 {
                if segment_dist2(self.x_of(col), y, a, b) <= rr {
                    self.put(col, subrow, class);
                }
            }
        }
    }

    /// Paint an axis-aligned obround (stadium), sheet coordinates; `w` is the
    /// size along x, `h` along y - the same shape the writer flashes for a slot.
    fn fill_obround(&mut self, cx: f64, cy: f64, w: f64, h: f64, class: Class) {
        let r = w.min(h) / 2.0;
        let ex = (w / 2.0 - r).max(0.0);
        let ey = (h / 2.0 - r).max(0.0);
        self.fill_capsule((cx - ex, cy - ey), (cx + ex, cy + ey), r, class);
    }

    /// Paint one sub-pixel: the one whose centre is nearest `(x, y)`.
    fn plot(&mut self, x: f64, y: f64, class: Class) {
        let col = ((x - self.minx) / self.step - 0.5).round();
        let subrow = ((self.maxy - y) / self.step - 0.5).round();
        if col < 0.0 || subrow < 0.0 || !col.is_finite() || !subrow.is_finite() {
            return;
        }
        self.put(col as usize, subrow as usize, class);
    }

    /// Paint a one sub-pixel wide line, sheet coordinates.
    fn stroke_segment(&mut self, a: (f64, f64), b: (f64, f64), class: Class) {
        let pad = self.step;
        let bounds = (
            a.0.min(b.0) - pad,
            a.1.min(b.1) - pad,
            a.0.max(b.0) + pad,
            a.1.max(b.1) + pad,
        );
        if self.clip(bounds).is_none() {
            return; // wholly outside the window
        }
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let steps = (dx.hypot(dy) / (self.step / 2.0)).ceil();
        let steps = if steps.is_finite() {
            (steps as usize).clamp(1, 1 << 16)
        } else {
            1
        };
        for i in 0..=steps {
            let t = i as f64 / steps as f64;
            self.plot(a.0 + dx * t, a.1 + dy * t, class);
        }
    }

    /// Paint a polyline given in board coordinates.
    fn stroke_path(&mut self, path: &[(f64, f64)], xf: &Transform, class: Class) {
        for pair in path.windows(2) {
            let a = xf.apply(pair[0].0, pair[0].1);
            let b = xf.apply(pair[1].0, pair[1].1);
            self.stroke_segment(a, b, class);
        }
    }

    /// Paint the four edges of a sheet rectangle.
    fn stroke_rect(&mut self, rect: Bounds, class: Class) {
        let (x0, y0, x1, y1) = rect;
        for (a, b) in [
            ((x0, y0), (x1, y0)),
            ((x1, y0), (x1, y1)),
            ((x1, y1), (x0, y1)),
            ((x0, y1), (x0, y0)),
        ] {
            self.stroke_segment(a, b, class);
        }
    }
}

/// The half-open index range `[ceil(lo), floor(hi) + 1)` clamped to `0..n`.
fn span(lo: f64, hi: f64, n: usize) -> Option<(usize, usize)> {
    if !lo.is_finite() || !hi.is_finite() || hi < 0.0 {
        return None;
    }
    let first = lo.ceil().max(0.0);
    if first >= n as f64 {
        return None;
    }
    let last = (hi.floor() + 1.0).min(n as f64);
    if last <= first {
        return None;
    }
    Some((first as usize, last as usize))
}

/// Squared distance from `(px, py)` to the segment `a`-`b`.
fn segment_dist2(px: f64, py: f64, a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 {
        (((px - a.0) * dx + (py - a.1) * dy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (ex, ey) = (px - (a.0 + dx * t), py - (a.1 + dy * t));
    ex * ex + ey * ey
}

// --------------------------------------------------------------------------- //
// Rasterising one pad
// --------------------------------------------------------------------------- //

/// The caption above the picture.
pub fn caption(label: &str, project: &str, side: &str, raster: &Raster) -> String {
    format!(
        "{label} · {project} {side} · window {:.2} x {:.2} mm · 1 col = {:.2} mm",
        raster.width_mm(),
        raster.height_mm(),
        raster.step
    )
}

/// Rasterise the sheet around the pad `sel` into a `cols x rows` panel.
///
/// Returns None when the pad's side has no cell on the sheet (it is disabled,
/// or the layout is empty), when the pad is gone, or when the panel has no
/// room at all.
pub fn render_pad(
    projects: &[Project],
    layout: &Layout,
    sel: (SideId, usize),
    cache: &mut GeomCache,
    cols: usize,
    rows: usize,
) -> Option<Raster> {
    if cols == 0 || rows == 0 {
        return None;
    }
    let area: &Area = layout.areas.iter().find(|a| a.side == sel.0)?;
    let side: &Side = sel.0.get(projects);
    let pad = side.pads.get(sel.1)?;
    let entry = side_entry(cache, projects, sel.0);

    // 1. Where to look and how far to zoom in.
    let chosen = entry.pads.get(sel.1)?;
    let board = if chosen.bounds.0 <= chosen.bounds.2 {
        chosen.bounds
    } else {
        (pad.x, pad.y, pad.x, pad.y) // an empty pad shape: aim at the flash
    };
    let sheet = sheet_bounds(board, &area.transform);
    let centre = ((sheet.0 + sheet.2) / 2.0, (sheet.1 + sheet.3) / 2.0);
    let step = zoom_step((sheet.2 - sheet.0).max(sheet.3 - sheet.1), cols);
    let mut raster = Raster::new(cols, rows, centre, step);

    let xf = &area.transform;
    let params = &layout.params;

    // 2. Copper, then every pad on it (the PNG's steps 2 and 3).
    for shape in &entry.copper {
        raster.fill_shape(shape, xf, Class::Copper);
    }
    for shape in &entry.pads {
        raster.fill_shape(shape, xf, Class::Pad);
    }

    // 3. The board outline, or its bounding box when there is no outline layer.
    if entry.outline.is_empty() {
        raster.stroke_rect(area.board_rect, Class::Outline);
    } else {
        for path in &entry.outline {
            raster.stroke_path(path, xf, Class::Outline);
        }
    }

    // 4. What stays closed: ignored candidates and the openings the user removed.
    let closed = side.closed_paste_indices();
    for index in &closed {
        if let Some(shape) = entry.paste.get(*index) {
            raster.fill_shape(shape, xf, Class::Ignored);
        }
    }
    for (index, other) in side.pads.iter().enumerate() {
        if other.is_candidate() && other.state == STATE_IGNORE {
            if let Some(shape) = entry.pads.get(index) {
                raster.fill_shape(shape, xf, Class::Ignored);
            }
        }
    }

    // 5. Everything that becomes an opening.
    for (index, shape) in entry.paste.iter().enumerate() {
        if !closed.contains(&index) {
            raster.fill_shape(shape, xf, Class::Opening);
        }
    }
    for (index, other) in side.pads.iter().enumerate() {
        if other.is_candidate() && other.state == STATE_OPEN {
            if let Some(shape) = entry.pads.get(index) {
                raster.fill_shape(shape, xf, Class::Opening);
            }
        }
    }
    for &(dx, dy) in &layout.dots {
        raster.fill_circle(dx, dy, params.dot_dia / 2.0, Class::Opening);
    }
    for cell in &layout.areas {
        for &(hx, hy) in &cell.holes {
            raster.fill_circle(hx, hy, params.hole_dia / 2.0, Class::Opening);
        }
        for &(sx, sy, sw, sh) in &cell.slots {
            raster.fill_obround(sx, sy, sw, sh, Class::Opening);
        }
        if let Some(mark) = cell.marker {
            for (x0, y0, x1, y1) in marker_strokes(mark, params.marker_size) {
                raster.fill_capsule((x0, y0), (x1, y1), params.dot_dia / 2.0, Class::Opening);
            }
        }
    }

    // 6. Pads that still need a decision, and finally the one under the cursor.
    for (index, other) in side.pads.iter().enumerate() {
        if other.is_candidate() && other.state == STATE_UNDEFINED {
            if let Some(shape) = entry.pads.get(index) {
                raster.fill_shape(shape, xf, Class::Undefined);
            }
        }
    }
    raster.fill_shape(chosen, xf, Class::Selected);

    raster.caption = caption(&pad.label(), &side.project_name, &side.name, &raster);
    Some(raster)
}

#[cfg(test)]
mod tests {
    use super::*;

    use geo::{LineString, MultiPolygon, Polygon};

    fn square(cx: f64, cy: f64, half: f64) -> Geom {
        MultiPolygon::new(vec![Polygon::new(
            LineString::from(vec![
                (cx - half, cy - half),
                (cx + half, cy - half),
                (cx + half, cy + half),
                (cx - half, cy + half),
                (cx - half, cy - half),
            ]),
            vec![],
        )])
    }

    fn raster(cols: usize, rows: usize, step: f64) -> Raster {
        Raster::new(cols, rows, (0.0, 0.0), step)
    }

    #[test]
    fn the_zoom_gives_the_pad_a_quarter_of_the_panel() {
        // 1 mm across 40 columns: a quarter is 10 columns
        assert!((zoom_step(1.0, 40) - 0.1).abs() < 1e-12);
        // never fewer than three columns
        assert!((zoom_step(1.0, 8) - 1.0 / 3.0).abs() < 1e-12);
        // never more than the panel
        assert!((zoom_step(1.0, 2) - 0.5).abs() < 1e-12);
        // a degenerate pad still gives a usable window
        assert!(zoom_step(0.0, 40).is_finite() && zoom_step(0.0, 40) > 0.0);
    }

    #[test]
    fn the_window_is_centred_and_square() {
        let r = raster(10, 4, 0.5);
        assert_eq!(r.subrows, 8);
        assert!((r.width_mm() - 5.0).abs() < 1e-12);
        assert!((r.height_mm() - 4.0).abs() < 1e-12);
        let (cx, cy) = r.centre();
        assert!(cx.abs() < 1e-12 && cy.abs() < 1e-12);
        // the step is the same across and down
        assert!((r.x_of(1) - r.x_of(0) - r.step).abs() < 1e-12);
        assert!((r.y_of(0) - r.y_of(1) - r.step).abs() < 1e-12);
    }

    #[test]
    fn geometry_is_sampled_through_the_transform() {
        let mut r = raster(8, 4, 0.25);
        let xf = Transform {
            mirror: true,
            dx: 4.0,
            dy: 1.0,
        };
        // a board square at (4, -1) lands on the sheet at (0, 0)
        let shape = Shape::new(&square(4.0, -1.0, 0.3));
        assert_eq!(shape.bounds, (3.7, -1.3, 4.3, -0.7));
        assert!(shape.contains(4.0, -1.0) && !shape.contains(0.0, 0.0));
        r.fill_shape(&shape, &xf, Class::Selected);
        assert_eq!(r.at(4, 4), Class::Selected);
        assert_eq!(r.at(0, 0), Class::Empty);
    }

    #[test]
    fn circles_capsules_and_lines_land_where_they_should() {
        let mut r = raster(9, 4, 0.2);
        r.fill_circle(0.0, 0.0, 0.25, Class::Opening);
        assert_eq!(r.at(4, 4), Class::Opening);
        assert_eq!(r.at(0, 0), Class::Empty);

        let mut r = raster(9, 4, 0.2);
        r.fill_obround(0.0, 0.0, 1.8, 0.3, Class::Opening);
        assert_eq!(r.at(0, 4), Class::Opening); // long: reaches the left edge
        assert_eq!(r.at(4, 0), Class::Empty); // short: not the top row

        let mut r = raster(9, 4, 0.2);
        r.stroke_segment((-1.0, 0.0), (1.0, 0.0), Class::Outline);
        assert_eq!(r.at(0, 4), Class::Outline);
        assert_eq!(r.at(8, 4), Class::Outline);
        assert_eq!(r.at(4, 0), Class::Empty);
    }

    #[test]
    fn half_blocks_carry_two_sub_pixels() {
        assert_eq!(half_block(Class::Empty, Class::Empty).0, " ");
        assert_eq!(half_block(Class::Opening, Class::Opening).0, FULL_BLOCK);
        assert_eq!(half_block(Class::Opening, Class::Empty).0, UPPER_HALF);
        assert_eq!(half_block(Class::Empty, Class::Opening).0, LOWER_HALF);
        let (symbol, fg, bg) = half_block(Class::Selected, Class::Undefined);
        assert_eq!(symbol, UPPER_HALF);
        assert_eq!(fg, class_color(Class::Selected));
        assert_eq!(bg, class_color(Class::Undefined));
    }

    #[test]
    fn later_classes_cover_earlier_ones() {
        let mut r = raster(4, 2, 1.0);
        let shape = Shape::new(&square(0.0, 0.0, 10.0));
        let xf = Transform::default();
        r.fill_shape(&shape, &xf, Class::Copper);
        r.fill_shape(&shape, &xf, Class::Selected);
        assert_eq!(r.at(0, 0), Class::Selected);
        assert!(Class::Copper < Class::Pad && Class::Pad < Class::Selected);
    }

    #[test]
    fn a_window_that_misses_everything_stays_empty() {
        let mut r = raster(6, 3, 0.1);
        let far = Shape::new(&square(100.0, 100.0, 1.0));
        r.fill_shape(&far, &Transform::default(), Class::Pad);
        r.fill_circle(-50.0, 0.0, 1.0, Class::Opening);
        r.stroke_segment((20.0, 20.0), (30.0, 30.0), Class::Outline);
        assert!((0..r.cols).all(|c| (0..r.subrows).all(|s| r.at(c, s) == Class::Empty)));
    }
}
