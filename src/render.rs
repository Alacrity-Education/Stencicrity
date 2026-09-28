//! High resolution PNG preview of a stencil layout.
//!
//! The preview shows the whole stencil sheet the way it will be cut:
//! everything that becomes an opening is red, copper is dim grey, pads that
//! still need a decision are yellow, and pads the user excluded - together
//! with the paste openings the user closed - are blue.  It is meant to be
//! looked at, not to be exact - it is rasterised with `tiny-skia` straight
//! from the geometry of the gerber objects.
//!
//! Cells can sit anywhere on the sheet (they are packed with MaxRects, see
//! [`crate::layout`]); the only cell markings drawn are a faint dashed guide
//! under every dotted line (one per cell edge, `dot_line_gap/2` inside the
//! cell) and, when the datum rides on a raster (`slot_pitch` for the slots,
//! `hole_grid` for the holes), a very faint grid of that pitch over the whole
//! sheet, so the jig pins can be seen sitting on its intersections.  Cells
//! that did not fit are drawn outside the stencil boundary - the image simply
//! grows to cover them.
//!
//! The alignment datum is drawn on top of the openings: the slots are
//! openings themselves (red obrounds), every jig pin is a dashed blue ghost
//! circle where the fixture pin comes up through the foil, and the `slots`
//! datum also marks the corner the piece is pushed into with an orange
//! bracket and a small green arrow pointing at it.  The X orientation marker
//! is an opening like any other, so it is drawn red, in the `dot_dia` stroke
//! width it is cut with, just inside the bracket.
//!
//! The image carries a millimetre ruler along its left and bottom edge
//! (origin at the bottom left corner of the stencil) and a legend band
//! underneath.  Sheet millimetres are mapped to pixels with `px_per_mm`
//! (reduced so the whole image, bands included, stays below `max_px`) and the
//! Y axis is flipped, because image rows grow downwards while sheet
//! coordinates grow upwards.
//!
//! Memory: one RGBA `Pixmap` plus a *single* 8-bit class mask that is zeroed
//! and reused for each colour class (and, only when a gerber file actually
//! uses clear polarity, a second mask of the same size to subtract with).
//! The buffers are 432 MB + 108 MB at the 12000 px cap and 185 MB + 46 MB at
//! the default 380 x 280 mm sheet; measured peak RSS for the eight example
//! projects is 863 MB and 378 MB, against 1099 MB and 515 MB for the Pillow
//! original, which keeps five masks alive at once.

use std::collections::BTreeMap;
use std::path::Path;

use ab_glyph::{point, Font, FontRef, GlyphId, PxScale, ScaleFont};
use tiny_skia::{
    FillRule, LineCap, LineJoin, Mask, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Stroke,
    StrokeDash, Transform as SkTransform,
};

use crate::gerber::{contour_points, geom_bounds, object_geometry, GraphicObject};
use crate::layout::marker_strokes;
use crate::model::{
    Area, Bounds, Geom, Layout, Pad, Project, SideId, Transform, STATE_IGNORE, STATE_OPEN,
    STATE_UNDEFINED,
};

type Rgb = [u8; 3];

const BACKGROUND: Rgb = [0x14, 0x14, 0x14];
const COLOR_SHEET: Rgb = [0x8a, 0x8a, 0x8a]; // the stencil boundary
const COLOR_GUIDE: Rgb = [0x33, 0x33, 0x33]; // faint dashed line under the divider dots
const COLOR_GRID: Rgb = [0x20, 0x20, 0x20]; // even fainter: the jig pin raster
const COLOR_COPPER: Rgb = [0x2e, 0x2e, 0x2e];
const COLOR_PAD: Rgb = [0x5a, 0x5a, 0x5a];
const COLOR_OUTLINE: Rgb = [0xc8, 0xc8, 0xc8];
const COLOR_IGNORE: Rgb = [0x3a, 0x5f, 0xcd];
const COLOR_OPEN: Rgb = [0xff, 0x2a, 0x2a];
const COLOR_UNDEFINED: Rgb = [0xff, 0xd0, 0x00];
const COLOR_SELECTED: Rgb = [0x00, 0xe5, 0xff];
const COLOR_PIN: Rgb = [0x4a, 0xa3, 0xff]; // ghost outline of a jig pin (dashed)
const COLOR_DATUM: Rgb = [0xff, 0x9a, 0x1f]; // bracket at the corner the piece is pushed into
const COLOR_NEST: Rgb = [0x39, 0xd9, 0x8a]; // arrow showing the nesting direction
const COLOR_LABEL: Rgb = [0xdd, 0xdd, 0xdd];
const COLOR_RULER_BG: Rgb = [0x1c, 0x1c, 0x1c];
const COLOR_RULER: Rgb = [0xb0, 0xb0, 0xb0];

const BAND_PX: i64 = 48; // smallest legend band height
const LEGEND_MM: f64 = 5.0; // nominal legend band height in millimetres
const REFERENCE_PPMM: f64 = 20.0; // scale the pixel-sized constants were chosen for
const RULER_MM: f64 = 6.0; // nominal ruler band thickness in millimetres
const RULER_MIN_PX: i64 = 28; // ... but never thinner than this
const TICK_MAJOR: f64 = 0.60; // tick lengths as a fraction of the nominal band
const TICK_MEDIUM: f64 = 0.40;
const TICK_MINOR: f64 = 0.20;
const LABEL_FRACTION: f64 = 0.55; // ruler label height as a fraction of the band
const MINOR_PPMM: f64 = 8.0; // 1 mm ticks only from this scale up
const LABEL_STEPS: [i64; 7] = [10, 20, 50, 100, 200, 500, 1000];

/// The fixed part of the legend line, appended after the layout numbers.
const LEGEND_KEY: &str = "   red = stencil opening   yellow = undefined pad   blue = ignored pad / closed opening   grey = copper   blue outline = jig pin";

const DATUM_LEG_MM: f64 = 4.0; // length of the two legs of the datum corner bracket
const NEST_TAIL_MM: f64 = 16.0; // the nesting arrow runs from here ...
const NEST_TIP_MM: f64 = 10.0; // ... to here, on the cell diagonal from the corner

/// DejaVu Sans, the face the Python preview picks up from the system.
/// See `assets/DejaVuSans-LICENSE.txt` for its (Bitstream Vera) licence.
const FONT_DATA: &[u8] = include_bytes!("../assets/DejaVuSans.ttf");

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
        Self {
            px_per_mm: 20.0,
            max_px: 12000,
            selected: None,
            title: String::new(),
        }
    }
}

// --------------------------------------------------------------------------- //
// Small numeric helpers
// --------------------------------------------------------------------------- //

/// `round()` with Python's banker's rounding, so the band and stroke widths
/// come out of the same arithmetic as in the original.
fn iround(v: f64) -> i64 {
    let r = v.round();
    if (v - v.trunc()).abs() == 0.5 && r % 2.0 != 0.0 {
        (r - v.signum()) as i64
    } else {
        r as i64
    }
}

fn ceil_i(v: f64) -> i64 {
    v.ceil() as i64
}

// --------------------------------------------------------------------------- //
// Fonts
// --------------------------------------------------------------------------- //

#[derive(Clone, Copy, PartialEq, Eq)]
enum HAnchor {
    Left,
    Middle,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum VAnchor {
    /// the ascender line sits on the given y
    Ascender,
    /// the midpoint of ascender and descender sits on the given y
    Middle,
    /// the descender line sits on the given y
    Descender,
}

/// The embedded face, scaled the way Pillow scales a TrueType font: the
/// nominal size is the em square in pixels.
struct Face {
    font: FontRef<'static>,
    upem: f32,
    height_unscaled: f32,
    ascent_unscaled: f32,
    descent_unscaled: f32,
}

impl Face {
    fn new() -> anyhow::Result<Self> {
        let font = FontRef::try_from_slice(FONT_DATA)
            .map_err(|e| anyhow::anyhow!("embedded font is invalid: {e}"))?;
        let upem = font.units_per_em().unwrap_or(1000.0);
        Ok(Self {
            height_unscaled: font.height_unscaled(),
            ascent_unscaled: font.ascent_unscaled(),
            descent_unscaled: font.descent_unscaled(),
            upem,
            font,
        })
    }

    fn px_scale(&self, size: i64) -> PxScale {
        let size = size.max(1) as f32;
        PxScale::from(size * self.height_unscaled / self.upem)
    }

    fn ascent(&self, size: i64) -> f64 {
        f64::from(self.ascent_unscaled) * size.max(1) as f64 / f64::from(self.upem)
    }

    /// Negative, like the font's own descender.
    fn descent(&self, size: i64) -> f64 {
        f64::from(self.descent_unscaled) * size.max(1) as f64 / f64::from(self.upem)
    }

    /// Advance width of `text` in pixels at `size` (Pillow's `getlength`).
    fn width(&self, size: i64, text: &str) -> f64 {
        let scaled = self.font.as_scaled(self.px_scale(size));
        let mut w = 0.0f32;
        let mut prev: Option<GlyphId> = None;
        for c in text.chars() {
            let id = scaled.glyph_id(c);
            if let Some(p) = prev {
                w += scaled.kern(p, id);
            }
            w += scaled.h_advance(id);
            prev = Some(id);
        }
        f64::from(w)
    }

    /// The largest size at or below `size` that keeps `text` under `available` px.
    fn fitted_size(&self, text: &str, size: i64, available: f64, minimum: i64) -> i64 {
        if available <= 0.0 || text.is_empty() {
            return size;
        }
        let length = self.width(size, text);
        if length <= available || length <= 0.0 {
            return size;
        }
        minimum.max((size as f64 * available / length) as i64)
    }

    /// Size and (possibly ellipsised) text that stay inside `available` px.
    fn fit_label(&self, text: &str, size: i64, available: f64) -> (i64, String) {
        let size = self.fitted_size(text, size, available, 11);
        if available <= 0.0 {
            return (size, String::new());
        }
        if self.width(size, text) <= available {
            return (size, text.to_string());
        }
        let mut short: Vec<char> = text.chars().collect();
        while !short.is_empty() {
            let mut candidate: String = short.iter().collect();
            candidate.push('…');
            if self.width(size, &candidate) <= available {
                break;
            }
            short.pop();
        }
        if short.is_empty() {
            (size, String::new())
        } else {
            let mut out: String = short.iter().collect();
            out.push('…');
            (size, out)
        }
    }

    fn draw(
        &self,
        pm: &mut Pixmap,
        pos: (f64, f64),
        anchor: (HAnchor, VAnchor),
        text: &str,
        size: i64,
        color: Rgb,
    ) {
        if text.is_empty() {
            return;
        }
        let size = size.max(1);
        let scale = self.px_scale(size);
        let x = match anchor.0 {
            HAnchor::Left => pos.0,
            HAnchor::Middle => pos.0 - self.width(size, text) / 2.0,
            HAnchor::Right => pos.0 - self.width(size, text),
        };
        let baseline = match anchor.1 {
            VAnchor::Ascender => pos.1 + self.ascent(size),
            VAnchor::Middle => pos.1 + (self.ascent(size) + self.descent(size)) / 2.0,
            VAnchor::Descender => pos.1 + self.descent(size),
        };
        let scaled = self.font.as_scaled(scale);
        let mut pen = x as f32;
        let base = baseline as f32;
        let mut prev: Option<GlyphId> = None;
        for c in text.chars() {
            let id = scaled.glyph_id(c);
            if let Some(p) = prev {
                pen += scaled.kern(p, id);
            }
            if let Some(outline) = self
                .font
                .outline_glyph(id.with_scale_and_position(scale, point(pen, base)))
            {
                let bounds = outline.px_bounds();
                let ox = bounds.min.x.floor() as i64;
                let oy = bounds.min.y.floor() as i64;
                outline.draw(|gx, gy, cov| {
                    let a = (cov.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                    blend(pm, ox + i64::from(gx), oy + i64::from(gy), color, a);
                });
            }
            pen += scaled.h_advance(id);
            prev = Some(id);
        }
    }
}

// --------------------------------------------------------------------------- //
// Direct pixel helpers (crisp, axis aligned, Pillow-like inclusive boxes)
// --------------------------------------------------------------------------- //

/// Source-over one colour with `a/255` coverage; the pixmap stays opaque.
fn blend(pm: &mut Pixmap, x: i64, y: i64, color: Rgb, a: u8) {
    if a == 0 {
        return;
    }
    let (w, h) = (i64::from(pm.width()), i64::from(pm.height()));
    if x < 0 || y < 0 || x >= w || y >= h {
        return;
    }
    let idx = (y * w + x) as usize;
    let px = &mut pm.pixels_mut()[idx];
    if a == 255 {
        *px = PremultipliedColorU8::from_rgba(color[0], color[1], color[2], 255).unwrap();
        return;
    }
    let inv = 255 - a as u32;
    let mix = |src: u8, dst: u8| -> u8 {
        let v = src as u32 * a as u32 + dst as u32 * inv + 127;
        ((v + (v >> 8)) >> 8) as u8
    };
    let out = PremultipliedColorU8::from_rgba(
        mix(color[0], px.red()),
        mix(color[1], px.green()),
        mix(color[2], px.blue()),
        255,
    )
    .unwrap_or(*px);
    *px = out;
}

/// Fill a Pillow-style *inclusive* integer box.
fn fill_box(pm: &mut Pixmap, x0: i64, y0: i64, x1: i64, y1: i64, color: Rgb) {
    let (w, h) = (i64::from(pm.width()), i64::from(pm.height()));
    let xa = x0.min(x1).max(0);
    let xb = x0.max(x1).min(w - 1);
    let ya = y0.min(y1).max(0);
    let yb = y0.max(y1).min(h - 1);
    if xa > xb || ya > yb {
        return;
    }
    let solid = PremultipliedColorU8::from_rgba(color[0], color[1], color[2], 255).unwrap();
    let pixels = pm.pixels_mut();
    for y in ya..=yb {
        let row = (y * w) as usize;
        pixels[row + xa as usize..=row + xb as usize].fill(solid);
    }
}

/// A crisp vertical line of `width` pixels centred on `x`.
fn vline(pm: &mut Pixmap, x: f64, y0: f64, y1: f64, width: i64, color: Rgb) {
    let x0 = (x - width as f64 / 2.0).round() as i64;
    fill_box(
        pm,
        x0,
        y0.round() as i64,
        x0 + width - 1,
        y1.round() as i64,
        color,
    );
}

/// A crisp horizontal line of `width` pixels centred on `y`.
fn hline(pm: &mut Pixmap, y: f64, x0: f64, x1: f64, width: i64, color: Rgb) {
    let y0 = (y - width as f64 / 2.0).round() as i64;
    fill_box(
        pm,
        x0.round() as i64,
        y0,
        x1.round() as i64,
        y0 + width - 1,
        color,
    );
}

/// Pillow's `rectangle(outline=..., width=w)`: the border grows *inwards*.
fn rect_outline(pm: &mut Pixmap, x0: f64, y0: f64, x1: f64, y1: f64, width: i64, color: Rgb) {
    let (xa, xb) = (x0.min(x1).round() as i64, x0.max(x1).round() as i64);
    let (ya, yb) = (y0.min(y1).round() as i64, y0.max(y1).round() as i64);
    let w = width.max(1);
    fill_box(pm, xa, ya, xb, ya + w - 1, color);
    fill_box(pm, xa, yb - w + 1, xb, yb, color);
    fill_box(pm, xa, ya, xa + w - 1, yb, color);
    fill_box(pm, xb - w + 1, ya, xb, yb, color);
}

fn solid_paint(color: Rgb) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color[0], color[1], color[2], 255);
    paint.anti_alias = true;
    paint
}

fn stroke_spec(width: f64) -> Stroke {
    Stroke {
        width: width as f32,
        line_cap: LineCap::Butt,
        line_join: LineJoin::Round,
        ..Stroke::default()
    }
}

/// Snap an axis-aligned line to pixel centres so thin guides stay crisp.
fn snap(v: f64, width: i64) -> f64 {
    if width % 2 == 1 {
        v.floor() + 0.5
    } else {
        v.round()
    }
}

fn stroke_polyline(pm: &mut Pixmap, pts: &[(f64, f64)], color: Rgb, stroke: &Stroke) {
    if pts.len() < 2 {
        return;
    }
    let mut pb = PathBuilder::new();
    pb.move_to(pts[0].0 as f32, pts[0].1 as f32);
    for p in &pts[1..] {
        pb.line_to(p.0 as f32, p.1 as f32);
    }
    if let Some(path) = pb.finish() {
        pm.stroke_path(
            &path,
            &solid_paint(color),
            stroke,
            SkTransform::identity(),
            None,
        );
    }
}

/// A dashed segment between two pixel points.
fn dashed_line(
    pm: &mut Pixmap,
    p0: (f64, f64),
    p1: (f64, f64),
    color: Rgb,
    width: i64,
    dash: (f64, f64),
) {
    let (mut a, mut b) = (p0, p1);
    if (a.0 - b.0).abs() < 0.5 {
        let x = snap((a.0 + b.0) / 2.0, width);
        a.0 = x;
        b.0 = x;
    }
    if (a.1 - b.1).abs() < 0.5 {
        let y = snap((a.1 + b.1) / 2.0, width);
        a.1 = y;
        b.1 = y;
    }
    if (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9 {
        return;
    }
    let mut stroke = stroke_spec(width as f64);
    stroke.dash = StrokeDash::new(vec![dash.0 as f32, dash.1 as f32], 0.0);
    stroke_polyline(pm, &[a, b], color, &stroke);
}

/// A dashed circle outline (a ghost jig pin) around a pixel centre.
fn dashed_circle(pm: &mut Pixmap, c: (f64, f64), r: f64, color: Rgb, width: i64) {
    if r <= 0.0 {
        return;
    }
    let mut pb = PathBuilder::new();
    pb.push_circle(c.0 as f32, c.1 as f32, r as f32);
    let Some(path) = pb.finish() else { return };
    let mut stroke = stroke_spec(width as f64);
    if r >= 3.0 {
        // 22 degrees of dash, 14 degrees of gap, like the Pillow original.
        let circumference = 2.0 * std::f64::consts::PI * r;
        stroke.dash = StrokeDash::new(
            vec![
                (circumference * 22.0 / 360.0) as f32,
                (circumference * 14.0 / 360.0) as f32,
            ],
            0.0,
        );
    }
    pm.stroke_path(
        &path,
        &solid_paint(color),
        &stroke,
        SkTransform::identity(),
        None,
    );
}

/// A thin arrow from `tail` to `tip` (pixel points) with a small head.
fn arrow(pm: &mut Pixmap, tail: (f64, f64), tip: (f64, f64), color: Rgb, width: i64, head: f64) {
    let (dx, dy) = (tip.0 - tail.0, tip.1 - tail.1);
    let length = dx.hypot(dy);
    if length <= 0.0 {
        return;
    }
    let (ux, uy) = (dx / length, dy / length);
    let stroke = stroke_spec(width as f64);
    stroke_polyline(pm, &[tail, tip], color, &stroke);
    for sign in [-1.0f64, 1.0] {
        // rotate the reversed direction by +/- 30 degrees for the two barbs
        let (ca, sa) = (30f64.to_radians().cos(), 30f64.to_radians().sin() * sign);
        let bx = -ux * ca + uy * sa;
        let by = -uy * ca - ux * sa;
        stroke_polyline(
            pm,
            &[tip, (tip.0 + bx * head, tip.1 + by * head)],
            color,
            &stroke,
        );
    }
}

// --------------------------------------------------------------------------- //
// Sheet-space shapes
// --------------------------------------------------------------------------- //

/// An axis-aligned obround (stadium) outline, sheet coordinates.
///
/// `w` is the size along x, `h` along y; the short sides are half circles (a
/// circle when the two are equal), exactly like the `O` gerber aperture the
/// writer flashes for a slot.
fn obround(cx: f64, cy: f64, w: f64, h: f64) -> Vec<(f64, f64)> {
    let r = w.min(h) / 2.0;
    let ex = (w / 2.0 - r).max(0.0);
    let ey = (h / 2.0 - r).max(0.0);
    capsule((cx - ex, cy - ey), (cx + ex, cy + ey), r)
}

/// A straight stroke of `width` with round ends, sheet coordinates: the shape
/// the gerber writer produces for a round aperture, so the preview shows the
/// X marker as it is cut.
fn stroke_shape(x0: f64, y0: f64, x1: f64, y1: f64, width: f64) -> Vec<(f64, f64)> {
    capsule((x0, y0), (x1, y1), width.max(0.0) / 2.0)
}

/// The outline of a segment buffered by `r` (round caps), 96 segments per turn.
fn capsule(a: (f64, f64), b: (f64, f64), r: f64) -> Vec<(f64, f64)> {
    const STEPS: usize = 48; // half a turn
    if r <= 0.0 {
        return Vec::new();
    }
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = dx.hypot(dy);
    let angle = if len > 0.0 { dy.atan2(dx) } else { 0.0 };
    let mut pts = Vec::with_capacity(2 * STEPS + 2);
    for (centre, base) in [(b, angle), (a, angle + std::f64::consts::PI)] {
        for i in 0..=STEPS {
            let t =
                base - std::f64::consts::FRAC_PI_2 + std::f64::consts::PI * i as f64 / STEPS as f64;
            pts.push((centre.0 + r * t.cos(), centre.1 + r * t.sin()));
        }
    }
    pts
}

// --------------------------------------------------------------------------- //
// Image geometry: rulers, legend band and scale
// --------------------------------------------------------------------------- //

/// Pixel geometry of one preview image.
struct Frame {
    ppmm: f64,
    content_w: f64,
    content_h: f64,
    legend: i64,
    major: f64,
    medium: f64,
    minor: f64,
    font_px: i64,
    label_w: f64,
    left: i64,
    bottom: i64,
    width_px: i64,
    height_px: i64,
    /// image row of the stencil origin (y = 0)
    origin_y: i64,
    longest: i64,
}

impl Frame {
    fn new(face: &Face, ppmm: f64, content_w: f64, content_h: f64, legend: i64) -> Self {
        let base = RULER_MIN_PX.max(iround(RULER_MM * ppmm));
        let major = base as f64 * TICK_MAJOR;
        let font_px = 9.max(iround(LABEL_FRACTION * base as f64));
        // Widest label that can occur on either ruler.
        let widest = format!("{}", ceil_i(content_w.max(content_h).max(1.0)));
        let label_w = face.width(font_px, &widest);
        // The bands are grown when the labels would not fit next to the ticks.
        let left = base.max(ceil_i(major + label_w + 0.30 * base as f64));
        let bottom = base.max(ceil_i(major + 1.25 * font_px as f64 + 4.0));
        let width_px = left + 1.max(ceil_i(content_w * ppmm));
        let height_px = 1.max(ceil_i(content_h * ppmm)) + bottom + legend;
        Self {
            ppmm,
            content_w,
            content_h,
            legend,
            major,
            medium: base as f64 * TICK_MEDIUM,
            minor: base as f64 * TICK_MINOR,
            font_px,
            label_w,
            left,
            bottom,
            width_px,
            height_px,
            origin_y: height_px - bottom - legend,
            longest: width_px.max(height_px),
        }
    }

    /// Sheet millimetres -> image pixels (Y flipped).
    fn to_px(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.left as f64 + x * self.ppmm,
            self.origin_y as f64 - y * self.ppmm,
        )
    }

    /// Millimetres between two labelled ticks so the labels never touch.
    fn label_step(&self, vertical: bool) -> i64 {
        let need = if vertical {
            1.8 * self.font_px as f64
        } else {
            self.label_w + 0.8 * self.font_px as f64
        };
        for step in LABEL_STEPS {
            if step as f64 * self.ppmm >= need {
                return step;
            }
        }
        LABEL_STEPS[LABEL_STEPS.len() - 1]
    }
}

/// Height of the legend band in pixels.
fn legend_band(ppmm: f64, max_px: i64) -> i64 {
    let band = BAND_PX.max(iround(LEGEND_MM * ppmm));
    1.max(band.min(1.max(max_px / 3)))
}

/// The largest frame at or below `px_per_mm` that stays within `max_px`.
///
/// The ruler and legend bands grow with the scale and the scale is limited by
/// the bands, so the fixed point is found by iteration instead of one shrink.
fn frame_for(face: &Face, content_w: f64, content_h: f64, px_per_mm: f64, max_px: u32) -> Frame {
    let max_px = 64.max(i64::from(max_px));
    let mut ppmm = px_per_mm.max(1e-9);
    let mut frame = Frame::new(face, ppmm, content_w, content_h, legend_band(ppmm, max_px));
    for _ in 0..12 {
        let bands_w = frame.left as f64;
        let bands_h = (frame.bottom + frame.legend) as f64;
        let limit_w = if content_w > 0.0 {
            (max_px as f64 - bands_w) / content_w
        } else {
            f64::INFINITY
        };
        let limit_h = if content_h > 0.0 {
            (max_px as f64 - bands_h) / content_h
        } else {
            f64::INFINITY
        };
        let new = px_per_mm.min(limit_w).min(limit_h).max(1e-3);
        if (new - ppmm).abs() < 1e-9 {
            break;
        }
        ppmm = new;
        frame = Frame::new(face, ppmm, content_w, content_h, legend_band(ppmm, max_px));
    }
    // Integer rounding (ceil) can still push the image one pixel over the cap.
    for _ in 0..24 {
        if frame.longest <= max_px {
            return frame;
        }
        ppmm *= 0.999;
        frame = Frame::new(face, ppmm, content_w, content_h, legend_band(ppmm, max_px));
    }
    frame
}

/// Millimetre rulers along the left and the bottom of the stencil area.
fn draw_rulers(pm: &mut Pixmap, face: &Face, frame: &Frame) {
    let width_px = frame.width_px;
    let ruler_bottom = frame.origin_y + frame.bottom;
    // Bands (drawn over the content so nothing spills into them); the row and
    // the column of the stencil boundary itself are left untouched.
    fill_box(
        pm,
        0,
        frame.origin_y + 1,
        width_px,
        ruler_bottom - 1,
        COLOR_RULER_BG,
    );
    fill_box(pm, 0, 0, frame.left - 1, ruler_bottom - 1, COLOR_RULER_BG);

    let thin = 1.max(iround(frame.ppmm / REFERENCE_PPMM));
    let minor = frame.ppmm >= MINOR_PPMM;
    let step_x = frame.label_step(false);
    let step_y = frame.label_step(true);

    let level = |mm: i64| -> Option<f64> {
        if mm % 10 == 0 {
            Some(frame.major)
        } else if mm % 5 == 0 {
            Some(frame.medium)
        } else if minor {
            Some(frame.minor)
        } else {
            None
        }
    };

    let gutter = 0.35 * frame.font_px as f64; // smallest gap between two labels
    let origin_y = frame.origin_y as f64;

    // Bottom ruler: X.
    let mut used = -1e9f64;
    for mm in 0..=(frame.content_w.floor() as i64) {
        let Some(length) = level(mm) else { continue };
        let px = frame.left as f64 + mm as f64 * frame.ppmm;
        vline(pm, px, origin_y, origin_y + length, thin, COLOR_RULER);
        if mm % step_x != 0 {
            continue;
        }
        let text = mm.to_string();
        let half = face.width(frame.font_px, &text) / 2.0;
        let tx = px.max(half + 2.0).min(width_px as f64 - half - 2.0);
        if tx - half < used + gutter {
            continue; // would touch the previous label
        }
        used = tx + half;
        face.draw(
            pm,
            (tx, origin_y + frame.major + 2.0),
            (HAnchor::Middle, VAnchor::Ascender),
            &text,
            frame.font_px,
            COLOR_RULER,
        );
    }

    // Left ruler: Y (numbers grow upwards).
    let mut used = 1e9f64;
    for mm in 0..=(frame.content_h.floor() as i64) {
        let Some(length) = level(mm) else { continue };
        let py = origin_y - mm as f64 * frame.ppmm;
        hline(
            pm,
            py,
            frame.left as f64 - length,
            frame.left as f64,
            thin,
            COLOR_RULER,
        );
        if mm % step_y != 0 {
            continue;
        }
        let half = frame.font_px as f64 * 0.6;
        let ty = py.max(half + 2.0).min(origin_y - half - 2.0);
        if ty + half > used - gutter {
            continue; // would touch the previous label
        }
        used = ty - half;
        face.draw(
            pm,
            (frame.left as f64 - frame.major - 3.0, ty),
            (HAnchor::Right, VAnchor::Middle),
            &mm.to_string(),
            frame.font_px,
            COLOR_RULER,
        );
    }
}

// --------------------------------------------------------------------------- //
// Class masks
// --------------------------------------------------------------------------- //

/// An 8-bit mask collecting every shape of one colour class.
///
/// Shapes are accumulated at full image size but the dirty region is tracked,
/// so compositing only touches the pixels that were actually drawn.  Clear
/// (negative) polarity objects are subtracted through a scratch mask that is
/// only allocated when a file actually uses them.
struct ClassMask {
    mask: Mask,
    scratch: Option<Mask>,
    w: i64,
    h: i64,
    bbox: Option<(i64, i64, i64, i64)>,
}

impl ClassMask {
    fn new(w: u32, h: u32) -> anyhow::Result<Self> {
        let mask =
            Mask::new(w, h).ok_or_else(|| anyhow::anyhow!("cannot allocate a {w}x{h} mask"))?;
        Ok(Self {
            mask,
            scratch: None,
            w: i64::from(w),
            h: i64::from(h),
            bbox: None,
        })
    }

    /// Extend the dirty region by a pixel-space box.
    fn touch(&mut self, x0: f64, y0: f64, x1: f64, y1: f64) {
        let b = (
            x0.floor() as i64 - 1,
            y0.floor() as i64 - 1,
            x1.ceil() as i64 + 1,
            y1.ceil() as i64 + 1,
        );
        self.bbox = Some(match self.bbox {
            None => b,
            Some(o) => (o.0.min(b.0), o.1.min(b.1), o.2.max(b.2), o.3.max(b.3)),
        });
    }

    /// The dirty region, clamped to the image.
    fn dirty(&self) -> Option<(i64, i64, i64, i64)> {
        let b = self.bbox?;
        let x0 = b.0.clamp(0, self.w);
        let y0 = b.1.clamp(0, self.h);
        let x1 = b.2.clamp(x0, self.w);
        let y1 = b.3.clamp(y0, self.h);
        if x1 <= x0 || y1 <= y0 {
            None
        } else {
            Some((x0, y0, x1, y1))
        }
    }

    /// Zero the dirty rows and forget them, ready for the next colour class.
    fn reset(&mut self) {
        if let Some((_, y0, _, y1)) = self.dirty() {
            let (w, h) = (self.w, self.h);
            let a = (y0 * w) as usize;
            let b = ((y1.min(h)) * w) as usize;
            self.mask.data_mut()[a..b].fill(0);
        }
        self.bbox = None;
    }

    fn set_pixel(&mut self, x: i64, y: i64, value: u8) {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return;
        }
        let idx = (y * self.w + x) as usize;
        self.mask.data_mut()[idx] = value;
    }

    /// Union `path` into the mask, or subtract it when `erase`.
    fn apply_path(&mut self, path: &tiny_skia::Path, erase: bool) {
        if !erase {
            self.mask
                .fill_path(path, FillRule::EvenOdd, true, SkTransform::identity());
            return;
        }
        let b = path.bounds();
        let x0 = (b.left().floor() as i64).clamp(0, self.w);
        let y0 = (b.top().floor() as i64).clamp(0, self.h);
        let x1 = (b.right().ceil() as i64 + 1).clamp(x0, self.w);
        let y1 = (b.bottom().ceil() as i64 + 1).clamp(y0, self.h);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let (w, _) = (self.w, self.h);
        if self.scratch.is_none() {
            self.scratch = Mask::new(self.w as u32, self.h as u32);
        }
        let Some(scratch) = self.scratch.as_mut() else {
            return;
        };
        scratch.fill_path(path, FillRule::EvenOdd, true, SkTransform::identity());
        let src = self.scratch.as_ref().expect("just filled").data().to_vec();
        {
            let dst = self.mask.data_mut();
            for y in y0..y1 {
                let row = (y * w) as usize;
                for x in x0..x1 {
                    let i = row + x as usize;
                    dst[i] = dst[i].saturating_sub(src[i]);
                }
            }
        }
        let scratch = self.scratch.as_mut().expect("just filled");
        let data = scratch.data_mut();
        for y in y0..y1 {
            let row = (y * w) as usize;
            data[row + x0 as usize..row + x1 as usize].fill(0);
        }
    }

    /// Rasterise one polygon (holes included), mapped board -> sheet -> pixels.
    fn fill_polygon(
        &mut self,
        poly: &geo::Polygon<f64>,
        xf: &Transform,
        frame: &Frame,
        erase: bool,
    ) {
        let ring = |ls: &geo::LineString<f64>| -> Vec<(f32, f32)> {
            ls.0.iter()
                .map(|c| {
                    let (sx, sy) = xf.apply(c.x, c.y);
                    let (px, py) = frame.to_px(sx, sy);
                    (px as f32, py as f32)
                })
                .collect()
        };
        let exterior = ring(poly.exterior());
        if exterior.len() < 3 {
            return;
        }
        let (mut minx, mut maxx) = (f32::INFINITY, f32::NEG_INFINITY);
        let (mut miny, mut maxy) = (f32::INFINITY, f32::NEG_INFINITY);
        for p in &exterior {
            minx = minx.min(p.0);
            maxx = maxx.max(p.0);
            miny = miny.min(p.1);
            maxy = maxy.max(p.1);
        }
        if !minx.is_finite() || !miny.is_finite() {
            return;
        }
        self.touch(
            f64::from(minx),
            f64::from(miny),
            f64::from(maxx),
            f64::from(maxy),
        );

        if maxx - minx < 1.0 && maxy - miny < 1.0 {
            // Sub-pixel shape: keep at least one pixel so it does not disappear.
            self.set_pixel(minx as i64, miny as i64, if erase { 0 } else { 255 });
            return;
        }

        let mut pb = PathBuilder::new();
        push_ring(&mut pb, &exterior);
        for hole in poly.interiors() {
            let pts = ring(hole);
            if pts.len() >= 3 {
                push_ring(&mut pb, &pts);
            }
        }
        if let Some(path) = pb.finish() {
            self.apply_path(&path, erase);
        }
    }

    fn fill_geom(&mut self, geom: &Geom, xf: &Transform, frame: &Frame, erase: bool) {
        for poly in &geom.0 {
            self.fill_polygon(poly, xf, frame, erase);
        }
    }

    /// Rasterise a sheet-coordinate ring (slots, markers, dots).
    fn fill_sheet_ring(&mut self, pts: &[(f64, f64)], frame: &Frame) {
        if pts.len() < 3 {
            return;
        }
        let poly = geo::Polygon::new(geo::LineString::from(pts.to_vec()), vec![]);
        self.fill_polygon(&poly, &Transform::default(), frame, false);
    }

    /// Rasterise gerber graphic objects, honouring clear (negative) polarity.
    fn fill_objects<'a, I>(&mut self, objects: I, xf: &Transform, frame: &Frame)
    where
        I: IntoIterator<Item = &'a GraphicObject>,
    {
        for obj in objects {
            let geom = object_geometry(obj);
            self.fill_geom(&geom, xf, frame, !obj.dark());
        }
    }

    fn ellipse(&mut self, cx: f64, cy: f64, r: f64) {
        if r <= 0.0 {
            return;
        }
        self.touch(cx - r, cy - r, cx + r, cy + r);
        let mut pb = PathBuilder::new();
        // Pillow's inclusive ellipse box covers 2r+1 pixels.
        pb.push_circle(cx as f32, cy as f32, (r + 0.5) as f32);
        if let Some(path) = pb.finish() {
            self.mask
                .fill_path(&path, FillRule::Winding, true, SkTransform::identity());
        }
    }

    /// Paste `color` onto `pm` wherever this mask is set.
    fn composite(&self, pm: &mut Pixmap, color: Rgb) {
        let Some((x0, y0, x1, y1)) = self.dirty() else {
            return;
        };
        let w = self.w;
        let data = self.mask.data();
        let solid = PremultipliedColorU8::from_rgba(color[0], color[1], color[2], 255).unwrap();
        let pixels = pm.pixels_mut();
        for y in y0..y1 {
            let row = (y * w) as usize;
            for x in x0..x1 {
                let i = row + x as usize;
                let a = data[i];
                if a == 0 {
                    continue;
                }
                if a == 255 {
                    pixels[i] = solid;
                    continue;
                }
                let inv = 255 - a as u32;
                let dst = pixels[i];
                let mix = |src: u8, d: u8| -> u8 {
                    let v = src as u32 * a as u32 + d as u32 * inv + 127;
                    ((v + (v >> 8)) >> 8) as u8
                };
                pixels[i] = PremultipliedColorU8::from_rgba(
                    mix(color[0], dst.red()),
                    mix(color[1], dst.green()),
                    mix(color[2], dst.blue()),
                    255,
                )
                .unwrap_or(dst);
            }
        }
    }
}

fn push_ring(pb: &mut PathBuilder, pts: &[(f32, f32)]) {
    pb.move_to(pts[0].0, pts[0].1);
    for p in &pts[1..] {
        pb.line_to(p.0, p.1);
    }
    pb.close();
}

// --------------------------------------------------------------------------- //
// Content helpers
// --------------------------------------------------------------------------- //

/// Undefined candidate pads of one area, grouped by component reference.
fn pad_groups(pads: &[Pad]) -> Vec<(&str, Vec<&Pad>)> {
    let mut order: Vec<&str> = Vec::new();
    let mut groups: BTreeMap<&str, Vec<&Pad>> = BTreeMap::new();
    for pad in pads {
        if pad.is_candidate() && pad.state == STATE_UNDEFINED {
            let entry = groups.entry(pad.ref_.as_str()).or_default();
            if entry.is_empty() {
                order.push(pad.ref_.as_str());
            }
            entry.push(pad);
        }
    }
    order
        .into_iter()
        .filter_map(|r| groups.remove(r).map(|g| (r, g)))
        .collect()
}

/// Sheet-coordinate bounding box of a group of pads.
fn group_bounds(pads: &[&Pad], xf: &Transform) -> Option<Bounds> {
    let mut acc: Option<Bounds> = None;
    for pad in pads {
        let Some(b) = geom_bounds(&pad.geom) else {
            continue;
        };
        let (ax, ay) = xf.apply(b.0, b.1);
        let (bx, by) = xf.apply(b.2, b.3);
        let one = (ax.min(bx), ay.min(by), ax.max(bx), ay.max(by));
        acc = Some(match acc {
            None => one,
            Some(o) => (
                o.0.min(one.0),
                o.1.min(one.1),
                o.2.max(one.2),
                o.3.max(one.3),
            ),
        });
    }
    acc
}

/// Board outline polylines in sheet coordinates (empty when there is none).
fn outline_paths(projects: &[Project], area: &Area) -> Vec<Vec<(f64, f64)>> {
    let Some(outline) = projects[area.side.project].outline.as_ref() else {
        return Vec::new();
    };
    let xf = &area.transform;
    let mut paths = Vec::new();
    // flashes on an outline layer carry no shape info
    for obj in &outline.objects {
        match obj {
            GraphicObject::Stroke(s) => {
                let pts = s.seg.points();
                if pts.len() >= 2 {
                    paths.push(pts.into_iter().map(|(x, y)| xf.apply(x, y)).collect());
                }
            }
            GraphicObject::Region(r) => {
                for contour in &r.contours {
                    let pts = contour_points(contour);
                    if pts.len() >= 3 {
                        let mut ring: Vec<(f64, f64)> =
                            pts.into_iter().map(|(x, y)| xf.apply(x, y)).collect();
                        if ring[0] != ring[ring.len() - 1] {
                            ring.push(ring[0]);
                        }
                        paths.push(ring);
                    }
                }
            }
            GraphicObject::Flash(_) => {}
        }
    }
    paths
}

/// Sheet area the image has to cover: the stencil plus any overflowing block.
fn content_size(layout: &Layout) -> (f64, f64) {
    (
        layout.width.max(layout.block.2).max(1.0),
        layout.height.max(layout.block.3).max(1.0),
    )
}

/// One hairline per raster pitch over the sheet (nothing when there is none).
///
/// The raster is `slot_pitch` for the slots datum and `hole_grid` for the
/// holes datum; it is measured from the sheet origin, exactly like the jig
/// pins, so every pin has to sit on a crossing.  It is skipped when the lines
/// would be closer than two pixels.
fn draw_pin_raster(pm: &mut Pixmap, layout: &Layout, frame: &Frame) {
    let params = &layout.params;
    let grid = if params.slots() {
        params.slot_pitch
    } else if params.holes() {
        params.hole_grid
    } else {
        0.0
    };
    if grid <= 0.0 || grid * frame.ppmm < 2.0 {
        return;
    }
    for i in 0..=((layout.width / grid).floor() as i64) {
        let x = i as f64 * grid;
        let a = frame.to_px(x, 0.0);
        let b = frame.to_px(x, layout.height);
        vline(pm, snap(a.0, 1), b.1, a.1, 1, COLOR_GRID);
    }
    for i in 0..=((layout.height / grid).floor() as i64) {
        let y = i as f64 * grid;
        let a = frame.to_px(0.0, y);
        let b = frame.to_px(layout.width, y);
        hline(pm, snap(a.1, 1), a.0, b.0, 1, COLOR_GRID);
    }
}

/// Jig pins (both datums) and the datum corner marks (slots datum).
///
/// Every pin is a dashed blue ghost circle where the fixture pin comes up
/// through the foil - through a slot, or through a dowel hole.  With the slots
/// datum the corner the piece is pushed into also gets an orange bracket and a
/// small green arrow pointing at it from inside the cell.
fn draw_datum(pm: &mut Pixmap, layout: &Layout, frame: &Frame, scale: f64) {
    let params = &layout.params;
    let pin_dia = if params.slots() {
        params.pin_dia
    } else {
        params.hole_dia
    };
    let pin_w = iround(scale).clamp(1, 2);
    let ppmm = frame.ppmm;
    for area in &layout.areas {
        for &(px, py) in &area.pins {
            dashed_circle(
                pm,
                frame.to_px(px, py),
                pin_dia / 2.0 * ppmm,
                COLOR_PIN,
                pin_w,
            );
        }
    }
    if !params.slots() {
        return;
    }
    let leg = 1.max(iround(2.0 * scale));
    for area in &layout.areas {
        let (dx, dy) = area.datum_corner;
        let span = DATUM_LEG_MM.min(area.w / 4.0).min(area.h / 4.0);
        let mut stroke = stroke_spec(leg as f64);
        stroke.line_cap = LineCap::Round;
        stroke_polyline(
            pm,
            &[
                frame.to_px(dx + span, dy),
                frame.to_px(dx, dy),
                frame.to_px(dx, dy + span),
            ],
            COLOR_DATUM,
            &stroke,
        );
        // The arrow runs along the diagonal inside the padding: it stops short
        // of the board and its tip stays clear of the two slots.
        let pad = (area.board_rect.0 - dx).min(area.board_rect.1 - dy);
        let tail = NEST_TAIL_MM.min(pad - 0.75);
        let tip = (params.slot_inner() + 1.5).max(tail - NEST_TAIL_MM + NEST_TIP_MM);
        if tail - tip >= 3.0 {
            arrow(
                pm,
                frame.to_px(dx + tail, dy + tail),
                frame.to_px(dx + tip, dy + tip),
                COLOR_NEST,
                1.max(iround(scale)),
                (2.0 * ppmm).max(4.0),
            );
        }
    }
}

// --------------------------------------------------------------------------- //
// Public API
// --------------------------------------------------------------------------- //

/// Write the preview PNG.
///
/// The image shows the whole stencil sheet with a millimetre ruler on its left
/// and bottom edge and a legend underneath.  `opts.px_per_mm` is reduced
/// automatically so the longer image side stays at or below `opts.max_px`,
/// `opts.selected` highlights a single pad in cyan and `opts.title` is
/// prepended to the legend line.  A layout that does not fit is drawn anyway:
/// the image grows to cover the overflowing cells and the legend says so.
pub fn render_preview(
    projects: &[Project],
    layout: &Layout,
    path: &Path,
    opts: &RenderOptions,
) -> anyhow::Result<()> {
    let face = Face::new()?;
    let (content_w, content_h) = content_size(layout);
    let frame = frame_for(&face, content_w, content_h, opts.px_per_mm, opts.max_px);
    let ppmm = frame.ppmm;
    let params = &layout.params;

    let (w, h) = (frame.width_px as u32, frame.height_px as u32);
    let mut pm =
        Pixmap::new(w, h).ok_or_else(|| anyhow::anyhow!("cannot allocate a {w}x{h} preview"))?;
    pm.fill(tiny_skia::Color::from_rgba8(
        BACKGROUND[0],
        BACKGROUND[1],
        BACKGROUND[2],
        255,
    ));

    let scale = ppmm / REFERENCE_PPMM;
    let thin = 1.max(iround(scale)); // 1 px at the reference scale
    let outline_w = 2.max(iround(2.0 * scale));
    let select_w = 3.max(iround(3.0 * scale));
    let dash = ((6.0 * scale).max(4.0), (5.0 * scale).max(4.0));

    // 0. The dowel hole grid, under everything else: every hole centre sits on
    //    one of its intersections.
    draw_pin_raster(&mut pm, layout, &frame);

    // 1. A very faint dashed guide under every dotted line (the dots are the
    //    real marking; there are two lines per cell edge); no rectangles.
    for &(x0, y0, x1, y1) in &layout.dividers {
        dashed_line(
            &mut pm,
            frame.to_px(x0, y0),
            frame.to_px(x1, y1),
            COLOR_GUIDE,
            thin,
            dash,
        );
    }

    let mut mask = ClassMask::new(w, h)?;

    // 2. Copper, then every pad.
    for area in &layout.areas {
        mask.fill_objects(
            area.side.get(projects).copper_objects(),
            &area.transform,
            &frame,
        );
    }
    mask.composite(&mut pm, COLOR_COPPER);
    mask.reset();

    for area in &layout.areas {
        for pad in &area.side.get(projects).pads {
            mask.fill_geom(&pad.geom, &area.transform, &frame, false);
        }
    }
    mask.composite(&mut pm, COLOR_PAD);
    mask.reset();

    // 3. The stencil boundary, then the board outlines.
    let (sx0, sy0) = frame.to_px(0.0, layout.height);
    let (sx1, sy1) = frame.to_px(layout.width, 0.0);
    rect_outline(&mut pm, sx0, sy0, sx1, sy1, outline_w, COLOR_SHEET);

    for area in &layout.areas {
        let paths = outline_paths(projects, area);
        if paths.is_empty() {
            let (bx0, by0, bx1, by1) = area.board_rect;
            let p0 = frame.to_px(bx0, by1);
            let p1 = frame.to_px(bx1, by0);
            rect_outline(&mut pm, p0.0, p0.1, p1.0, p1.1, thin, COLOR_OUTLINE);
        } else {
            let stroke = stroke_spec(outline_w as f64);
            for pts in &paths {
                let px: Vec<(f64, f64)> = pts.iter().map(|&(x, y)| frame.to_px(x, y)).collect();
                stroke_polyline(&mut pm, &px, COLOR_OUTLINE, &stroke);
            }
        }
    }

    // 4. Ignored pads and the paste openings the user closed: the ones that
    //    survive are drawn red below, so the change stays visible.
    for area in &layout.areas {
        let side = area.side.get(projects);
        mask.fill_objects(side.closed_paste_objects(), &area.transform, &frame);
        for pad in side.candidates() {
            if pad.state == STATE_IGNORE {
                mask.fill_geom(&pad.geom, &area.transform, &frame, false);
            }
        }
    }
    mask.composite(&mut pm, COLOR_IGNORE);
    mask.reset();

    // 5. Everything that becomes an opening.
    for area in &layout.areas {
        let side = area.side.get(projects);
        mask.fill_objects(side.active_paste_objects(), &area.transform, &frame);
        for pad in side.candidates() {
            if pad.state == STATE_OPEN {
                mask.fill_geom(&pad.geom, &area.transform, &frame, false);
            }
        }
    }
    // Dots and the datum openings are already in sheet coordinates.
    for &(cx, cy) in &layout.dots {
        let (px, py) = frame.to_px(cx, cy);
        mask.ellipse(px, py, (params.dot_dia / 2.0 * ppmm).max(1.0));
    }
    for area in &layout.areas {
        for &(hx, hy) in &area.holes {
            let (px, py) = frame.to_px(hx, hy);
            mask.ellipse(px, py, (params.hole_dia / 2.0 * ppmm).max(1.0));
        }
        for &(sx, sy, sw, sh) in &area.slots {
            mask.fill_sheet_ring(&obround(sx, sy, sw, sh), &frame);
        }
        // The orientation X: two dot_dia wide strokes, an opening like the rest.
        if let Some(centre) = area.marker {
            for (mx0, my0, mx1, my1) in marker_strokes(centre, params.marker_size) {
                mask.fill_sheet_ring(&stroke_shape(mx0, my0, mx1, my1, params.dot_dia), &frame);
            }
        }
    }
    mask.composite(&mut pm, COLOR_OPEN);
    mask.reset();

    // 6. Pads that still need a decision.
    for area in &layout.areas {
        for pad in area.side.get(projects).candidates() {
            if pad.state == STATE_UNDEFINED {
                mask.fill_geom(&pad.geom, &area.transform, &frame, false);
            }
        }
    }
    mask.composite(&mut pm, COLOR_UNDEFINED);
    drop(mask);

    // 6a. The jig: a ghost circle per pin, the datum corner of every cell.
    draw_datum(&mut pm, layout, &frame, scale);

    // 6b. One labelled box per component that still has undefined pads.
    let ref_font = 11.max(iround(1.1 * ppmm));
    let grow = 0.4;
    for area in &layout.areas {
        for (name, group) in pad_groups(&area.side.get(projects).pads) {
            let Some((minx, miny, maxx, maxy)) = group_bounds(&group, &area.transform) else {
                continue;
            };
            let p0 = frame.to_px(minx - grow, maxy + grow);
            let p1 = frame.to_px(maxx + grow, miny - grow);
            rect_outline(&mut pm, p0.0, p0.1, p1.0, p1.1, thin, COLOR_UNDEFINED);
            let label = if name.is_empty() { "?" } else { name };
            face.draw(
                &mut pm,
                (p0.0, p0.1 - 2.0 * thin as f64),
                (HAnchor::Left, VAnchor::Descender),
                label,
                ref_font,
                COLOR_UNDEFINED,
            );
        }
    }

    // 7. Selected pad.
    if let Some((sid, index)) = opts.selected {
        if let Some(area) = layout.areas.iter().find(|a| a.side == sid) {
            let side = sid.get(projects);
            if let Some(pad) = side.pads.get(index) {
                if let Some((minx, miny, maxx, maxy)) = group_bounds(&[pad], &area.transform) {
                    let p0 = frame.to_px(minx - 0.6, maxy + 0.6);
                    let p1 = frame.to_px(maxx + 0.6, miny - 0.6);
                    rect_outline(&mut pm, p0.0, p0.1, p1.0, p1.1, select_w, COLOR_SELECTED);
                    face.draw(
                        &mut pm,
                        (p1.0 + 4.0 * thin as f64, (p0.1 + p1.1) / 2.0),
                        (HAnchor::Left, VAnchor::Middle),
                        &pad.label(),
                        ref_font,
                        COLOR_SELECTED,
                    );
                }
            }
        }
    }

    // 8. Cell labels, inside the cell, right of the top left datum feature.
    let label_size = 14.max(iround(2.5 * ppmm));
    let (left_span, right_span) = if params.holes() {
        let s = (params.hole_offset() + params.hole_dia / 2.0).max(0.0);
        (s, s)
    } else if params.slots() {
        (params.slot_inner(), 0.0) // slots are low or at mid height
    } else {
        (0.0, 0.0)
    };
    for area in &layout.areas {
        let side = area.side.get(projects);
        let mut text = format!("{} · {}", side.project_name, side.name);
        if side.mirror {
            text.push_str(" (mirrored)");
        }
        let lx = area.x + left_span + 1.0;
        let pos = frame.to_px(lx, area.y + area.h - 1.0);
        let available = ((area.x + area.w - right_span - 1.0) - lx).max(0.0) * ppmm;
        let (size, text) = face.fit_label(&text, label_size, available);
        face.draw(
            &mut pm,
            pos,
            (HAnchor::Left, VAnchor::Ascender),
            &text,
            size,
            COLOR_LABEL,
        );
    }

    // 9. Rulers over the bands, then the legend.
    draw_rulers(&mut pm, &face, &frame);

    let prefix = if opts.title.is_empty() {
        String::new()
    } else {
        format!("{}:  ", opts.title)
    };
    let legend = format!(
        "{prefix}stencil {:.0} x {:.0} mm   block {:.1} x {:.1} mm   {} cell(s){}",
        layout.width,
        layout.height,
        layout.block_width(),
        layout.block_height(),
        layout.areas.len(),
        LEGEND_KEY,
    );
    let warning = if layout.fits {
        ""
    } else {
        "   —  DOES NOT FIT"
    };
    let left = 6.max(iround(6.0 * scale));
    let legend_size = face.fitted_size(
        &format!("{legend}{warning}"),
        12.max(iround(frame.legend as f64 * 0.42)),
        (frame.width_px - 2 * left) as f64,
        10,
    );
    let base_y = frame.height_px as f64 - frame.legend as f64 / 2.0;
    face.draw(
        &mut pm,
        (left as f64, base_y),
        (HAnchor::Left, VAnchor::Middle),
        &legend,
        legend_size,
        COLOR_LABEL,
    );
    if !warning.is_empty() {
        let x = left as f64 + face.width(legend_size, &legend);
        face.draw(
            &mut pm,
            (x, base_y),
            (HAnchor::Left, VAnchor::Middle),
            warning,
            legend_size,
            COLOR_OPEN,
        );
    }

    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    pm.save_png(path)?;
    Ok(())
}

/// Open a file with xdg-open (detached). Returns false when it could not be launched.
pub fn open_file(path: &Path) -> bool {
    std::process::Command::new("xdg-open")
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}

// --------------------------------------------------------------------------- //
// Tests
// --------------------------------------------------------------------------- //

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::rc::Rc;

    use crate::gerber::{Aperture, Flash};
    use crate::model::{Area, LayoutParams, Project, Side, DATUM_SLOTS};

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Geom {
        geo::MultiPolygon::new(vec![geo::Polygon::new(
            geo::LineString::from(vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)]),
            vec![],
        )])
    }

    /// A pad whose copper is a 1 x 1 mm square with its lower left at (x, y).
    fn pad(
        project: &str,
        side: &str,
        ref_: &str,
        x: f64,
        y: f64,
        has_paste: bool,
        state: &str,
    ) -> Pad {
        let aperture = Rc::new(Aperture::new(10, "C", vec![0.6], BTreeMap::new(), None));
        Pad {
            key: format!("{project}/{side}/{ref_}.1@{x:.3},{y:.3}"),
            project: project.to_string(),
            side: side.to_string(),
            ref_: ref_.to_string(),
            pin: "1".to_string(),
            x,
            y,
            function: "SMDPad".to_string(),
            shape: "R 1.00x1.00".to_string(),
            // The geometry is built directly: the test never needs the aperture shape.
            flash: Flash {
                x,
                y,
                aperture,
                attrs: BTreeMap::new(),
                dark: true,
            },
            geom: rect(x, y, x + 1.0, y + 1.0),
            has_paste,
            state: state.to_string(),
            paste_indices: Vec::new(),
        }
    }

    fn side(project: &str, name: &str, mirror: bool, bbox: Bounds, pads: Vec<Pad>) -> Side {
        Side {
            project_name: project.to_string(),
            board_bbox: bbox,
            name: name.to_string(),
            copper: None,
            paste: None,
            mirror,
            enabled: true,
            pads,
        }
    }

    /// Two cells on a 120 x 90 sheet, with one pad of every state.
    ///
    /// * `alpha/top` sits at x 5..55, unmirrored, and owns the slot, the X
    ///   marker and the selected pad `C1.1`.
    /// * `beta/bottom` sits at x 60..110, mirrored about x = 85, and owns a
    ///   legacy dowel hole.
    fn synthetic() -> (Vec<Project>, Layout) {
        let alpha = Project {
            name: "alpha".to_string(),
            source: "alpha.zip".to_string(),
            outline: None,
            bbox: (15.0, 15.0, 45.0, 35.0),
            sides: vec![side(
                "alpha",
                "top",
                false,
                (15.0, 15.0, 45.0, 35.0),
                vec![
                    pad("alpha", "top", "U1", 20.0, 20.0, false, STATE_UNDEFINED),
                    pad("alpha", "top", "U1", 23.0, 20.0, false, STATE_UNDEFINED),
                    pad("alpha", "top", "C1", 30.0, 25.0, true, STATE_OPEN),
                    pad("alpha", "top", "R1", 35.0, 20.0, false, STATE_OPEN),
                    pad("alpha", "top", "R2", 38.0, 20.0, false, STATE_IGNORE),
                ],
            )],
        };
        // Board x is mirrored to sheet x: 80 -> -80 + 170 = 90.
        let beta = Project {
            name: "beta".to_string(),
            source: "beta.zip".to_string(),
            outline: None,
            bbox: (70.0, 15.0, 100.0, 35.0),
            sides: vec![side(
                "beta",
                "bottom",
                true,
                (70.0, 15.0, 100.0, 35.0),
                vec![pad(
                    "beta",
                    "bottom",
                    "J1",
                    80.0,
                    20.0,
                    false,
                    STATE_UNDEFINED,
                )],
            )],
        };

        let params = LayoutParams {
            datum: DATUM_SLOTS.to_string(),
            ..LayoutParams::default()
        };
        let area_a = Area {
            side: SideId {
                project: 0,
                side: 0,
            },
            x: 5.0,
            y: 5.0,
            w: 50.0,
            h: 40.0,
            board_rect: (15.0, 15.0, 45.0, 35.0),
            transform: Transform::default(),
            row: 0,
            overflow: false,
            holes: Vec::new(),
            slots: vec![(25.0, 6.0, 8.0, 4.5)],
            pins: vec![(25.0, 8.0)],
            datum_corner: (5.0, 5.0),
            marker: Some((12.0, 12.0)),
        };
        let area_b = Area {
            side: SideId {
                project: 1,
                side: 0,
            },
            x: 60.0,
            y: 5.0,
            w: 50.0,
            h: 40.0,
            board_rect: (70.0, 15.0, 100.0, 35.0),
            transform: Transform {
                mirror: true,
                dx: 170.0,
                dy: 0.0,
            },
            row: 0,
            overflow: false,
            holes: vec![(95.0, 40.0)],
            slots: Vec::new(),
            pins: vec![(80.0, 8.0)],
            datum_corner: (60.0, 5.0),
            marker: None,
        };
        let layout = Layout {
            params,
            areas: vec![area_a, area_b],
            width: 120.0,
            height: 90.0,
            block: (5.0, 5.0, 110.0, 45.0),
            fits: true,
            dots: vec![(20.0, 50.0), (20.0, 2.0)],
            dividers: vec![(5.0, 5.0, 55.0, 5.0), (5.0, 5.0, 5.0, 45.0)],
            heuristic: "bottom-left".to_string(),
            overflow: 0,
            dots_dropped: 0,
        };
        (vec![alpha, beta], layout)
    }

    fn render(projects: &[Project], layout: &Layout, opts: &RenderOptions) -> (Pixmap, Frame) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("preview.png");
        render_preview(projects, layout, &path, opts).expect("render");
        let pm = Pixmap::load_png(&path).expect("read back the png");
        let face = Face::new().expect("font");
        let (cw, ch) = content_size(layout);
        let frame = frame_for(&face, cw, ch, opts.px_per_mm, opts.max_px);
        assert_eq!(pm.width(), frame.width_px as u32);
        assert_eq!(pm.height(), frame.height_px as u32);
        (pm, frame)
    }

    fn at(pm: &Pixmap, x: f64, y: f64) -> Rgb {
        let (w, h) = (pm.width() as i64, pm.height() as i64);
        let (px, py) = (x.round() as i64, y.round() as i64);
        assert!(
            px >= 0 && py >= 0 && px < w && py < h,
            "({px}, {py}) outside {w}x{h}"
        );
        let p = pm.pixels()[(py * w + px) as usize];
        [p.red(), p.green(), p.blue()]
    }

    /// Is `color` present anywhere in the box (inclusive pixel coordinates)?
    fn has_color(pm: &Pixmap, x0: f64, y0: f64, x1: f64, y1: f64, color: Rgb) -> bool {
        let (w, h) = (pm.width() as i64, pm.height() as i64);
        let xa = (x0.floor() as i64).max(0);
        let xb = (x1.ceil() as i64).min(w - 1);
        let ya = (y0.floor() as i64).max(0);
        let yb = (y1.ceil() as i64).min(h - 1);
        for y in ya..=yb {
            for x in xa..=xb {
                let p = pm.pixels()[(y * w + x) as usize];
                if [p.red(), p.green(), p.blue()] == color {
                    return true;
                }
            }
        }
        false
    }

    /// Is there a pixel in the box that is `color` blended at least `min_t`
    /// over the plain background?  Anti-aliased strokes and glyphs rarely
    /// reach full coverage, so an exact match would be too strict.
    fn has_blend(pm: &Pixmap, x0: f64, y0: f64, x1: f64, y1: f64, color: Rgb, min_t: f64) -> bool {
        let lead = (0..3)
            .max_by_key(|&k| (i32::from(color[k]) - i32::from(BACKGROUND[k])).abs())
            .expect("three channels");
        let span = f64::from(color[lead]) - f64::from(BACKGROUND[lead]);
        if span.abs() < 1e-9 {
            return false;
        }
        let (w, h) = (pm.width() as i64, pm.height() as i64);
        let xa = (x0.floor() as i64).max(0);
        let xb = (x1.ceil() as i64).min(w - 1);
        let ya = (y0.floor() as i64).max(0);
        let yb = (y1.ceil() as i64).min(h - 1);
        for y in ya..=yb {
            for x in xa..=xb {
                let p = pm.pixels()[(y * w + x) as usize];
                let got = [p.red(), p.green(), p.blue()];
                let t = (f64::from(got[lead]) - f64::from(BACKGROUND[lead])) / span;
                if !(min_t..=1.05).contains(&t) {
                    continue;
                }
                if (0..3).all(|k| {
                    let want = f64::from(BACKGROUND[k])
                        + t * (f64::from(color[k]) - f64::from(BACKGROUND[k]));
                    (f64::from(got[k]) - want).abs() <= 12.0
                }) {
                    return true;
                }
            }
        }
        false
    }

    #[test]
    fn iround_rounds_half_to_even_like_python() {
        assert_eq!(iround(2.5), 2);
        assert_eq!(iround(3.5), 4);
        assert_eq!(iround(-2.5), -2);
        assert_eq!(iround(2.4), 2);
        assert_eq!(iround(2.6), 3);
        assert_eq!(iround(120.0), 120);
    }

    #[test]
    fn obround_is_a_stadium_of_the_right_size() {
        let pts = obround(10.0, 5.0, 8.0, 4.0);
        let (mut minx, mut maxx) = (f64::INFINITY, f64::NEG_INFINITY);
        let (mut miny, mut maxy) = (f64::INFINITY, f64::NEG_INFINITY);
        for &(x, y) in &pts {
            minx = minx.min(x);
            maxx = maxx.max(x);
            miny = miny.min(y);
            maxy = maxy.max(y);
        }
        assert!((maxx - minx - 8.0).abs() < 1e-6, "width {}", maxx - minx);
        assert!((maxy - miny - 4.0).abs() < 1e-6, "height {}", maxy - miny);
        assert!((minx + maxx - 20.0).abs() < 1e-6);
        // A square obround is a circle.
        let circle = obround(0.0, 0.0, 6.0, 6.0);
        for &(x, y) in &circle {
            assert!((x.hypot(y) - 3.0).abs() < 1e-6);
        }
    }

    #[test]
    fn the_scale_is_clamped_to_max_px() {
        let face = Face::new().unwrap();
        for &(ppmm, max_px) in &[
            (20.0, 12000u32),
            (20.0, 2000),
            (20.0, 800),
            (20.0, 300),
            (50.0, 1500),
            (4.0, 12000),
            (1.0, 64),
        ] {
            let frame = frame_for(&face, 380.0, 280.0, ppmm, max_px);
            let longest = frame.width_px.max(frame.height_px);
            assert!(
                longest <= i64::from(max_px),
                "{ppmm} px/mm, cap {max_px}: got {longest}"
            );
            assert!(frame.ppmm <= ppmm + 1e-9);
            // When the cap binds, the image really does fill it.
            if frame.ppmm < ppmm - 1e-9 {
                assert!(
                    longest as f64 >= 0.97 * f64::from(max_px),
                    "{ppmm} px/mm, cap {max_px}: only {longest}"
                );
            }
        }
    }

    #[test]
    fn png_size_follows_the_scale_and_the_cap() {
        let (projects, layout) = synthetic();
        let mut sizes = Vec::new();
        for &(ppmm, max_px) in &[(10.0, 4000u32), (6.0, 4000), (10.0, 600), (10.0, 400)] {
            let opts = RenderOptions {
                px_per_mm: ppmm,
                max_px,
                ..RenderOptions::default()
            };
            let (pm, frame) = render(&projects, &layout, &opts);
            let longest = pm.width().max(pm.height());
            assert!(
                longest <= max_px,
                "{ppmm} px/mm, cap {max_px}: {longest} px"
            );
            // left band + content, bottom band + legend + content.
            assert_eq!(pm.width() as i64, frame.left + ceil_i(120.0 * frame.ppmm));
            assert_eq!(
                pm.height() as i64,
                ceil_i(90.0 * frame.ppmm) + frame.bottom + frame.legend
            );
            sizes.push((pm.width(), pm.height()));
        }
        assert!(
            sizes[0].0 > sizes[1].0,
            "a bigger scale must give a bigger image"
        );
        assert!(sizes[2].0 < sizes[1].0, "the cap must shrink the image");
    }

    #[test]
    fn the_y_axis_is_flipped_and_the_origin_sits_at_the_bottom() {
        let (projects, layout) = synthetic();
        let opts = RenderOptions {
            px_per_mm: 10.0,
            max_px: 4000,
            ..RenderOptions::default()
        };
        let (pm, frame) = render(&projects, &layout, &opts);

        // Sheet y = 0 is the last content row, above the bottom ruler band.
        let (_, y0) = frame.to_px(0.0, 0.0);
        let (_, ytop) = frame.to_px(0.0, layout.height);
        assert!(y0 > ytop, "y must grow upwards: {y0} vs {ytop}");
        assert_eq!(y0 as i64, frame.origin_y);
        assert_eq!(
            frame.origin_y,
            frame.height_px - frame.bottom - frame.legend
        );
        assert!(
            frame.origin_y > frame.height_px / 2,
            "the origin must sit in the lower half"
        );

        // The two red divider dots: the high one is drawn above the low one.
        let high = frame.to_px(20.0, 50.0);
        let low = frame.to_px(20.0, 2.0);
        assert!(high.1 < low.1);
        assert_eq!(at(&pm, high.0, high.1), COLOR_OPEN);
        assert_eq!(at(&pm, low.0, low.1), COLOR_OPEN);

        // The stencil boundary frames exactly the sheet.
        let (bx, by) = frame.to_px(0.0, 0.0);
        assert!(has_color(
            &pm,
            bx - 2.0,
            by - 2.0,
            bx + 2.0,
            by,
            COLOR_SHEET
        ));
    }

    #[test]
    fn the_bands_and_the_legend_are_there() {
        let (projects, layout) = synthetic();
        let opts = RenderOptions {
            px_per_mm: 10.0,
            max_px: 4000,
            title: "unit test".to_string(),
            ..RenderOptions::default()
        };
        let (pm, frame) = render(&projects, &layout, &opts);

        let right = pm.width() as f64 - 2.0;
        // The bottom ruler band ...
        let band_y = frame.origin_y as f64 + frame.bottom as f64 / 2.0;
        assert_eq!(at(&pm, right, band_y), COLOR_RULER_BG);
        // ... the left ruler band ...
        assert_eq!(at(&pm, frame.left as f64 / 2.0, 2.0), COLOR_RULER_BG);
        // ... and the legend band underneath, which keeps the plain background.
        let legend_y = pm.height() as f64 - frame.legend as f64 / 2.0;
        assert_eq!(at(&pm, right, legend_y), BACKGROUND);
        // The legend line itself is drawn on the left of that band.
        assert!(
            has_blend(
                &pm,
                4.0,
                legend_y - frame.legend as f64 / 3.0,
                400.0,
                legend_y + frame.legend as f64 / 3.0,
                COLOR_LABEL,
                0.5
            ),
            "no legend text"
        );
        // Ruler ticks and labels live in the bands.
        assert!(has_color(
            &pm,
            0.0,
            frame.origin_y as f64,
            pm.width() as f64,
            band_y,
            COLOR_RULER
        ));
        assert!(has_color(
            &pm,
            0.0,
            0.0,
            frame.left as f64,
            frame.origin_y as f64,
            COLOR_RULER
        ));
    }

    #[test]
    fn openings_are_red_undefined_pads_yellow_and_ignored_ones_blue() {
        let (projects, layout) = synthetic();
        let opts = RenderOptions {
            px_per_mm: 10.0,
            max_px: 4000,
            ..RenderOptions::default()
        };
        let (pm, frame) = render(&projects, &layout, &opts);

        // A divider dot, an obround slot, a legacy dowel hole and the X marker.
        let dot = frame.to_px(20.0, 50.0);
        assert_eq!(at(&pm, dot.0, dot.1), COLOR_OPEN, "divider dot");
        let slot = frame.to_px(25.0, 6.0);
        assert_eq!(at(&pm, slot.0, slot.1), COLOR_OPEN, "slot");
        let hole = frame.to_px(95.0, 40.0);
        assert_eq!(at(&pm, hole.0, hole.1), COLOR_OPEN, "dowel hole");
        let marker = frame.to_px(12.0, 12.0);
        assert_eq!(at(&pm, marker.0, marker.1), COLOR_OPEN, "X marker");
        // An open candidate pad.
        let open = frame.to_px(35.5, 20.5);
        assert_eq!(at(&pm, open.0, open.1), COLOR_OPEN, "open pad");

        // Undefined pads are yellow, on both the plain and the mirrored cell.
        let undef = frame.to_px(20.5, 20.5);
        assert_eq!(at(&pm, undef.0, undef.1), COLOR_UNDEFINED, "undefined pad");
        let mirrored = frame.to_px(89.5, 20.5);
        assert_eq!(
            at(&pm, mirrored.0, mirrored.1),
            COLOR_UNDEFINED,
            "mirrored undefined pad"
        );

        // Ignored candidate pads are blue.
        let ignored = frame.to_px(38.5, 20.5);
        assert_eq!(at(&pm, ignored.0, ignored.1), COLOR_IGNORE, "ignored pad");

        // A pad with paste that nobody touched is plain copper grey.
        let pasted = frame.to_px(30.5, 25.5);
        assert_eq!(at(&pm, pasted.0, pasted.1), COLOR_PAD, "pasted pad");

        // The yellow group box sits 0.4 mm around the two U1 pads.
        let box_tl = frame.to_px(20.0 - 0.4, 21.0 + 0.4);
        assert_eq!(
            at(&pm, box_tl.0, box_tl.1),
            COLOR_UNDEFINED,
            "undefined group box"
        );

        // The board outline falls back to the board rectangle.
        let corner = frame.to_px(15.0, 35.0);
        assert_eq!(
            at(&pm, corner.0, corner.1),
            COLOR_OUTLINE,
            "board rect fallback"
        );
    }

    #[test]
    fn the_selected_pad_gets_a_cyan_box_and_a_label() {
        let (projects, layout) = synthetic();
        let selected = Some((
            SideId {
                project: 0,
                side: 0,
            },
            2,
        )); // C1.1 at (30, 25)
        let opts = RenderOptions {
            px_per_mm: 10.0,
            max_px: 4000,
            selected,
            ..RenderOptions::default()
        };
        let (pm, frame) = render(&projects, &layout, &opts);

        let p0 = frame.to_px(30.0 - 0.6, 26.0 + 0.6);
        let p1 = frame.to_px(31.0 + 0.6, 25.0 - 0.6);
        assert_eq!(at(&pm, p0.0, p0.1), COLOR_SELECTED, "cyan box corner");
        assert!(has_color(&pm, p0.0, p0.1, p1.0, p1.1, COLOR_SELECTED));
        // The label is drawn to the right of the box.
        assert!(
            has_blend(
                &pm,
                p1.0 + 1.0,
                p0.1 - 20.0,
                p1.0 + 200.0,
                p1.1 + 20.0,
                COLOR_SELECTED,
                0.5
            ),
            "no pad label"
        );
        // Without a selection nothing is cyan.
        let plain = RenderOptions {
            px_per_mm: 10.0,
            max_px: 4000,
            ..RenderOptions::default()
        };
        let (pm2, _) = render(&projects, &layout, &plain);
        assert!(!has_color(
            &pm2,
            0.0,
            0.0,
            pm2.width() as f64,
            pm2.height() as f64,
            COLOR_SELECTED
        ));
    }

    #[test]
    fn the_jig_and_the_datum_corner_are_drawn() {
        let (projects, layout) = synthetic();
        let opts = RenderOptions {
            px_per_mm: 10.0,
            max_px: 4000,
            ..RenderOptions::default()
        };
        let (pm, frame) = render(&projects, &layout, &opts);

        // Dashed blue ghost circle around the jig pin at (25, 8), r = 1.5 mm.
        let pin = frame.to_px(25.0, 8.0);
        let r = 1.5 * frame.ppmm;
        assert!(has_blend(
            &pm,
            pin.0 - r - 3.0,
            pin.1 - r - 3.0,
            pin.0 + r + 3.0,
            pin.1 + r + 3.0,
            COLOR_PIN,
            0.3
        ));
        // Orange bracket at the datum corner of both cells.
        let corner = frame.to_px(5.0, 5.0);
        assert!(has_blend(
            &pm,
            corner.0 - 3.0,
            corner.1 - 3.0,
            corner.0 + 40.0,
            corner.1 + 3.0,
            COLOR_DATUM,
            0.3
        ));
        // The faint pin raster hairlines and the dashed cell guides.
        let raster = frame.to_px(20.0, 70.0);
        assert!(has_color(
            &pm,
            raster.0 - 2.0,
            raster.1 - 2.0,
            raster.0 + 2.0,
            raster.1 + 2.0,
            COLOR_GRID
        ));
        let guide = frame.to_px(20.0, 5.0);
        assert!(has_color(
            &pm,
            guide.0 - 30.0,
            guide.1 - 2.0,
            guide.0 + 30.0,
            guide.1 + 2.0,
            COLOR_GUIDE
        ));
    }

    #[test]
    fn a_layout_that_does_not_fit_grows_the_image_and_warns() {
        let (projects, mut layout) = synthetic();
        layout.fits = false;
        layout.block = (5.0, 5.0, 150.0, 45.0);
        layout.areas[1].x = 100.0;
        layout.areas[1].overflow = true;
        let opts = RenderOptions {
            px_per_mm: 10.0,
            max_px: 4000,
            ..RenderOptions::default()
        };
        let (pm, frame) = render(&projects, &layout, &opts);
        // The image covers the overflowing block, not just the 120 mm sheet.
        assert_eq!(frame.content_w, 150.0);
        assert_eq!(pm.width() as i64, frame.left + ceil_i(150.0 * frame.ppmm));
        // "DOES NOT FIT" is written in the opening red.
        let legend_y = pm.height() as f64 - frame.legend as f64 / 2.0;
        let band = frame.legend as f64 / 3.0;
        assert!(has_blend(
            &pm,
            0.0,
            legend_y - band,
            pm.width() as f64,
            legend_y + band,
            COLOR_OPEN,
            0.5
        ));
    }

    #[test]
    fn clear_polarity_objects_erase_inside_their_class() {
        use crate::gerber::{GerberFile, GraphicObject, Region, Segment};

        let contour = |x0: f64, y0: f64, x1: f64, y1: f64| {
            vec![
                Segment::line(x0, y0, x1, y0),
                Segment::line(x1, y0, x1, y1),
                Segment::line(x1, y1, x0, y1),
                Segment::line(x0, y1, x0, y0),
            ]
        };
        let region = |x0, y0, x1, y1, dark| {
            GraphicObject::Region(Region {
                contours: vec![contour(x0, y0, x1, y1)],
                attrs: BTreeMap::new(),
                dark,
            })
        };

        let (mut projects, layout) = synthetic();
        // A copper plane with a clear window punched out of it.
        projects[0].sides[0].copper = Some(GerberFile {
            name: "alpha-copper".to_string(),
            objects: vec![
                region(16.0, 27.0, 44.0, 34.0, true),
                region(24.0, 29.0, 36.0, 32.0, false),
            ],
            ..GerberFile::default()
        });
        let opts = RenderOptions {
            px_per_mm: 10.0,
            max_px: 4000,
            ..RenderOptions::default()
        };
        let (pm, frame) = render(&projects, &layout, &opts);

        let plane = frame.to_px(19.0, 30.5);
        assert_eq!(at(&pm, plane.0, plane.1), COLOR_COPPER, "the copper plane");
        let window = frame.to_px(30.0, 30.5);
        assert_eq!(at(&pm, window.0, window.1), BACKGROUND, "the clear window");
    }

    #[test]
    fn cell_labels_are_written_and_shrink_to_fit() {
        let (projects, layout) = synthetic();
        let face = Face::new().unwrap();
        // The fitting helper ellipsises rather than overflowing.
        let (size, text) = face.fit_label("alpha · top", 40, 30.0);
        assert!(text.ends_with('…'), "{text:?}");
        assert!(face.width(size, &text) <= 30.0);
        let (_, full) = face.fit_label("alpha · top", 14, 400.0);
        assert_eq!(full, "alpha · top");
        let (_, gone) = face.fit_label("alpha · top", 14, 0.0);
        assert_eq!(gone, "");

        // The labels are painted inside the cells, past the datum features.
        let opts = RenderOptions {
            px_per_mm: 10.0,
            max_px: 4000,
            ..RenderOptions::default()
        };
        let (pm, frame) = render(&projects, &layout, &opts);
        let anchor = frame.to_px(5.0 + layout.params.slot_inner() + 1.0, 5.0 + 40.0 - 1.0);
        assert!(
            has_blend(
                &pm,
                anchor.0,
                anchor.1,
                anchor.0 + 300.0,
                anchor.1 + 3.0 * frame.ppmm,
                COLOR_LABEL,
                0.5
            ),
            "no cell label"
        );
    }
}
