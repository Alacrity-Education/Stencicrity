//! Cells, MaxRects packing, dotted borders, alignment features and the report.
//!
//! Every *enabled* side becomes one rectangular *cell* on the sheet: the board
//! bounding box padded by at least `gap/2` on all four sides, grown for the
//! datum (the slot raster, the hole grid). The cells are packed with a MaxRects
//! bin packer (three heuristics, the best result wins), the block of placed
//! cells is centred on the stencil by a whole number of raster pitches and the
//! cells that fit nowhere are lined up to the right of the sheet.
//!
//! Every cell owns one dotted *divider* line along each of its four edges,
//! running `dot_line_gap / 2` inside that edge; collinear pieces are merged and
//! the lines on the outer boundary of the block are dropped unless
//! `outer_border`. The dots along those lines are dropped where they would sit
//! in, or too close to, a datum opening: a dot goes when the neck of metal
//! between it and a slot, a dowel hole or the orientation marker would be
//! thinner than `dot_clearance`.

use std::collections::HashMap;

use crate::model::{
    all_sides, Area, Bounds, Config, Layout, LayoutParams, Project, SideId, Transform, DATUM_NONE,
    ORIENTATION_PORTRAIT, SIDE_TOP, SORT_NAME, STATE_IGNORE, STATE_OPEN, STATE_UNDEFINED,
};
use crate::util::natural_key;

const EPS: f64 = 1e-9;
const FIT_EPS: f64 = 1e-6;
/// Two coordinates closer than this are the same coordinate (divider merging).
const MERGE_EPS: f64 = 1e-6;
/// Touching cell edges are snapped together within this distance (see `aligned`).
const ALIGN_EPS: f64 = 1e-9;
/// Two holes closer than this are the same hole (neighbouring cells share them
/// when `hole_inset == -hole_dia / 2`, i.e. the holes sit on the dotted line).
const HOLE_EPS: f64 = 1e-6;
/// Slack of the pin raster arithmetic, in steps (see `snap_up` / `grid_size`).
const GRID_EPS: f64 = 1e-9;
/// Safety net of `edge_for_slots`: no cell edge is ever grown past this many
/// pitches looking for a slot position.
const MAX_PITCH_STEPS: i64 = 1000;

/// Placement heuristics, in preference order (the first one wins a tie).
pub const HEURISTIC_BL: &str = "bottom-left";
pub const HEURISTIC_BSSF: &str = "best short side";
pub const HEURISTIC_BAF: &str = "best area";
pub const HEURISTICS: [&str; 3] = [HEURISTIC_BL, HEURISTIC_BSSF, HEURISTIC_BAF];

const VERTICAL: u8 = 0;
const HORIZONTAL: u8 = 1;

// --------------------------------------------------------------------------- //
// Small numeric helpers
// --------------------------------------------------------------------------- //

/// Python's `round(value, 6)` (nearest, ties to even).
fn round6(value: f64) -> f64 {
    (value * 1e6).round_ties_even() / 1e6
}

fn lex(a: f64, b: f64) -> std::cmp::Ordering {
    a.total_cmp(&b)
}

// --------------------------------------------------------------------------- //
// Ordering
// --------------------------------------------------------------------------- //

fn side_rank(name: &str) -> u8 {
    if name == SIDE_TOP {
        0
    } else {
        1
    }
}

/// The enabled sides in the order they are handed to the packer.
pub fn ordered_sides(projects: &[Project], sides: &[SideId], params: &LayoutParams) -> Vec<SideId> {
    let mut out: Vec<SideId> = sides
        .iter()
        .copied()
        .filter(|id| id.get(projects).enabled)
        .collect();
    if params.sort == SORT_NAME {
        out.sort_by(|a, b| {
            let (sa, sb) = (a.get(projects), b.get(projects));
            natural_key(&sa.project_name)
                .cmp(&natural_key(&sb.project_name))
                .then(side_rank(&sa.name).cmp(&side_rank(&sb.name)))
        });
    } else {
        out.sort_by(|a, b| {
            let (sa, sb) = (a.get(projects), b.get(projects));
            lex(-round6(sa.board_height()), -round6(sb.board_height()))
                .then(lex(-round6(sa.board_width()), -round6(sb.board_width())))
                .then(natural_key(&sa.project_name).cmp(&natural_key(&sb.project_name)))
                .then(side_rank(&sa.name).cmp(&side_rank(&sb.name)))
        });
    }
    out
}

// --------------------------------------------------------------------------- //
// The jig pin grid
// --------------------------------------------------------------------------- //

/// The pin raster that actually applies to the layout's datum.
fn grid_of(params: &LayoutParams) -> f64 {
    if params.slots() {
        return params.slot_pitch.max(0.0);
    }
    if params.holes() {
        return params.hole_grid.max(0.0);
    }
    0.0
}

/// `size` grown until `size - 2 * ho` is a whole number of `grid`.
fn grid_size(size: f64, ho: f64, grid: f64) -> f64 {
    let steps = ((size - 2.0 * ho) / grid - GRID_EPS).ceil().max(0.0);
    steps * grid + 2.0 * ho
}

/// Cell size of a `board + gap` sized cell, grown to fit the hole grid.
fn cell_size(width: f64, height: f64, ho: f64, grid: f64) -> (f64, f64) {
    if grid <= 0.0 {
        return (width, height);
    }
    (
        width.max(grid_size(width, ho, grid)),
        height.max(grid_size(height, ho, grid)),
    )
}

/// The smallest cell corner `>= value` whose pins land on the grid.
fn snap_up(value: f64, ho: f64, grid: f64) -> f64 {
    if grid <= 0.0 {
        return value;
    }
    let steps = (value + ho) / grid;
    let nearest = steps.round_ties_even();
    let steps = if (steps - nearest).abs() * grid <= ALIGN_EPS {
        nearest
    } else {
        steps.ceil()
    };
    steps * grid - ho
}

/// `value` (>= 0) floored onto a multiple of `grid`; keeps pins aligned.
fn snap_down(value: f64, grid: f64) -> f64 {
    if grid <= 0.0 {
        return value;
    }
    let steps = value / grid;
    let nearest = steps.round_ties_even();
    let steps = if (steps - nearest).abs() * grid <= ALIGN_EPS {
        nearest
    } else {
        steps.floor()
    };
    (steps * grid).max(0.0)
}

/// `value` rounded up to a whole (at least one) multiple of `pitch`.
fn ceil_pitch(value: f64, pitch: f64) -> f64 {
    let steps = (value / pitch - GRID_EPS).ceil().max(1.0);
    steps * pitch
}

// --------------------------------------------------------------------------- //
// Cells and their datum features
// --------------------------------------------------------------------------- //

/// Cell size around one board: `board + gap`, grown for the datum.
fn cell_of(board_w: f64, board_h: f64, params: &LayoutParams, grid: f64) -> (f64, f64) {
    let (mut cw, mut ch) = (board_w + params.gap, board_h + params.gap);
    if params.slots() {
        let pad = params.pad().max(params.min_pad_for_slots());
        cw = board_w + 2.0 * pad;
        ch = board_h + 2.0 * pad;
        let pitch = params.slot_pitch.max(0.0);
        if pitch <= 0.0 {
            return (cw, ch); // no raster: padding only
        }
        cw = ceil_pitch(cw, pitch);
        ch = ceil_pitch(ch, pitch);
        let two = edge_for_slots(2, params);
        let one = edge_for_slots(1, params);
        if cw >= ch {
            // two slots along the longer edge
            return (cw.max(two), ch.max(one));
        }
        return (cw.max(one), ch.max(two));
    }
    if params.holes() {
        return cell_size(cw, ch, params.hole_offset(), grid);
    }
    (cw, ch)
}

/// Range a slot centre may take along one cell edge of `size`.
fn slot_span(start: f64, size: f64, params: &LayoutParams) -> (f64, f64) {
    let half = params.slot_length / 2.0;
    (
        start + params.slot_offset + half,
        start + size - params.slot_offset - half,
    )
}

/// The raster steps `first..=last` carrying a slot on an edge of `size`.
///
/// The cell corner sits at `k * slot_pitch - pin_offset`, so the raster
/// positions seen from the corner are `pin_offset + k * slot_pitch`; every one
/// of them that leaves the slot `slot_offset` clear of both ends of the edge
/// carries a slot (with the default numbers an edge of `n` pitches gets
/// `n - 1`). All of them are opened, so a modular jig may use any.
fn slot_steps(size: f64, params: &LayoutParams) -> (f64, i64, i64) {
    let pitch = params.slot_pitch.max(0.0);
    let none = (pitch, 0, -1);
    if pitch <= 0.0 {
        return none;
    }
    let (lo, hi) = slot_span(0.0, size, params);
    if hi < lo {
        return none; // the cell is shorter than a slot
    }
    let offset = params.pin_offset();
    let first = ((lo - offset) / pitch - GRID_EPS).ceil();
    let last = ((hi - offset) / pitch + GRID_EPS).floor();
    if !first.is_finite() || !last.is_finite() || last < first {
        return none;
    }
    (pitch, first as i64, last as i64)
}

/// Slot centres along one cell edge of `size`, measured from its corner.
fn slot_offsets(size: f64, params: &LayoutParams) -> Vec<f64> {
    let (pitch, first, last) = slot_steps(size, params);
    let offset = params.pin_offset();
    (first..=last).map(|k| offset + k as f64 * pitch).collect()
}

/// How many slot positions an edge of `size` carries (no allocation).
fn slot_count(size: f64, params: &LayoutParams) -> usize {
    let (_, first, last) = slot_steps(size, params);
    (last - first + 1).max(0) as usize
}

/// Shortest whole number of pitches whose edge carries `count` slots.
fn edge_for_slots(count: usize, params: &LayoutParams) -> f64 {
    let pitch = params.slot_pitch.max(0.0);
    if pitch <= 0.0 {
        return 0.0;
    }
    let mut steps: i64 = 1;
    while slot_count(steps as f64 * pitch, params) < count && steps < MAX_PITCH_STEPS {
        steps += 1;
    }
    steps as f64 * pitch
}

/// The two crossed strokes of an X marker, as `(x0, y0, x1, y1)`.
///
/// Two segments of length `size` at +45 and -45 degrees through `center`:
/// together they are an X whose bounding square is `size * sqrt(2) / 2` wide.
pub fn marker_strokes(center: (f64, f64), size: f64) -> [(f64, f64, f64, f64); 2] {
    let (cx, cy) = center;
    let half = size.max(0.0) * std::f64::consts::SQRT_2 / 4.0; // half the X's square
    [
        (cx - half, cy - half, cx + half, cy + half),
        (cx - half, cy + half, cx + half, cy - half),
    ]
}

/// Half the side of the square an X marker cuts out of the foil.
pub fn marker_half(params: &LayoutParams) -> f64 {
    params.marker_size.max(0.0) * std::f64::consts::SQRT_2 / 4.0 + params.dot_dia.max(0.0) / 2.0
}

/// Centre of the X orientation marker of the cell at `(x, y)`, or `None`.
fn marker_of(x: f64, y: f64, params: &LayoutParams) -> Option<(f64, f64)> {
    if !(params.slots() && params.marker) {
        return None;
    }
    Some((x + params.pin_offset(), y + params.pin_offset()))
}

type Slot = (f64, f64, f64, f64);
type Point = (f64, f64);

/// The alignment features `(holes, slots, pins)` of the cell at `(x, y, w, h)`.
fn datum_of(
    rect: (f64, f64, f64, f64),
    params: &LayoutParams,
    seen: &mut Dedupe,
) -> (Vec<Point>, Vec<Slot>, Vec<Point>) {
    let (x, y, w, h) = rect;
    if params.slots() {
        let (width, length) = (params.slot_width, params.slot_length);
        let across = params.slot_offset + width / 2.0; // cell edge to the slot centre
        let pin = params.pin_offset(); // == slot_inner - pin_dia / 2
        let mut slots: Vec<Slot> = Vec::new();
        let mut pins: Vec<Point> = Vec::new();
        for offset in slot_offsets(w, params) {
            // the bottom edge
            slots.push((x + offset, y + across, length, width));
            pins.push((x + offset, y + pin));
        }
        for offset in slot_offsets(h, params) {
            // the left edge
            slots.push((x + across, y + offset, width, length));
            pins.push((x + pin, y + offset));
        }
        return (Vec::new(), slots, pins);
    }
    if params.holes() {
        let holes = holes_of(rect, params, seen);
        return (holes.clone(), Vec::new(), holes);
    }
    (Vec::new(), Vec::new(), Vec::new())
}

/// Centres of the four dowel holes of the cell at `(x, y, w, h)`.
fn holes_of(rect: (f64, f64, f64, f64), params: &LayoutParams, seen: &mut Dedupe) -> Vec<Point> {
    if !params.holes() {
        return Vec::new();
    }
    let (x, y, w, h) = rect;
    let off = params.hole_offset();
    [
        (x + off, y + off),
        (x + w - off, y + off),
        (x + w - off, y + h - off),
        (x + off, y + h - off),
    ]
    .into_iter()
    .filter(|p| seen.add(p.0, p.1))
    .collect()
}

// --------------------------------------------------------------------------- //
// MaxRects bin packing
// --------------------------------------------------------------------------- //

type Rect = (f64, f64, f64, f64);

/// Bottom-Left: lowest top edge, then leftmost.
fn score_bottom_left(x: f64, y: f64, _aw: f64, _ah: f64, _cw: f64, ch: f64) -> (f64, f64) {
    (y + ch, x)
}

/// Best Short Side Fit: smallest leftover on the tighter axis.
fn score_short_side(_x: f64, _y: f64, aw: f64, ah: f64, cw: f64, ch: f64) -> (f64, f64) {
    let (dw, dh) = (aw - cw, ah - ch);
    (dw.min(dh), dw.max(dh))
}

/// Best Area Fit: smallest leftover area, then the tighter axis.
fn score_area(_x: f64, _y: f64, aw: f64, ah: f64, cw: f64, ch: f64) -> (f64, f64) {
    let (dw, dh) = (aw - cw, ah - ch);
    (aw * ah - cw * ch, dw.min(dh))
}

fn score(heuristic: &str, x: f64, y: f64, aw: f64, ah: f64, cw: f64, ch: f64) -> (f64, f64) {
    match heuristic {
        HEURISTIC_BSSF => score_short_side(x, y, aw, ah, cw, ch),
        HEURISTIC_BAF => score_area(x, y, aw, ah, cw, ch),
        _ => score_bottom_left(x, y, aw, ah, cw, ch),
    }
}

/// Split one free rectangle by a placed cell into its maximal remainders.
fn split(free: Rect, placed: Rect, out: &mut Vec<Rect>) {
    let (fx, fy, fw, fh) = free;
    let (px, py, pw, ph) = placed;
    if px >= fx + fw - EPS || px + pw <= fx + EPS || py >= fy + fh - EPS || py + ph <= fy + EPS {
        out.push(free); // no overlap
        return;
    }
    if px > fx + EPS {
        out.push((fx, fy, px - fx, fh)); // left
    }
    if px + pw < fx + fw - EPS {
        out.push((px + pw, fy, fx + fw - px - pw, fh)); // right
    }
    if py > fy + EPS {
        out.push((fx, fy, fw, py - fy)); // bottom
    }
    if py + ph < fy + fh - EPS {
        out.push((fx, py + ph, fw, fy + fh - py - ph)); // top
    }
}

fn contains(outer: Rect, inner: Rect) -> bool {
    let (ox, oy, ow, oh) = outer;
    let (ix, iy, iw, ih) = inner;
    ox <= ix + FIT_EPS
        && oy <= iy + FIT_EPS
        && ix + iw <= ox + ow + FIT_EPS
        && iy + ih <= oy + oh + FIT_EPS
}

/// Drop every free rectangle that another one already covers.
fn prune(rects: Vec<Rect>) -> Vec<Rect> {
    let mut dead = vec![false; rects.len()];
    for i in 0..rects.len() {
        if dead[i] {
            continue;
        }
        for j in 0..rects.len() {
            if i == j || dead[j] {
                continue;
            }
            if contains(rects[j], rects[i]) {
                dead[i] = true;
                break;
            }
        }
    }
    rects
        .into_iter()
        .zip(dead)
        .filter(|(_, d)| !*d)
        .map(|(r, _)| r)
        .collect()
}

/// Place `cells` (in order) into `(0, 0, sheet_w, sheet_h)`.
///
/// Returns one bottom-left corner per cell, `None` for a cell that did not fit
/// into any free rectangle.
fn maxrects(
    cells: &[(f64, f64)],
    sheet: (f64, f64),
    heuristic: &str,
    ho: f64,
    grid: f64,
) -> Vec<Option<Point>> {
    let (sheet_w, sheet_h) = sheet;
    let mut free: Vec<Rect> = Vec::new();
    if sheet_w > 0.0 && sheet_h > 0.0 {
        free.push((0.0, 0.0, sheet_w, sheet_h));
    }
    let mut out: Vec<Option<Point>> = Vec::with_capacity(cells.len());
    for &(cw, ch) in cells {
        let mut best: Option<(f64, f64, f64, f64)> = None; // (s0, s1, x, y)
        for &(fx, fy, fw, fh) in &free {
            let (x, y) = (snap_up(fx, ho, grid), snap_up(fy, ho, grid));
            let (aw, ah) = (fw - (x - fx), fh - (y - fy));
            if aw + FIT_EPS < cw || ah + FIT_EPS < ch {
                continue;
            }
            // Ties are broken by the placement itself (x, then y).
            let (s0, s1) = score(heuristic, x, y, aw, ah, cw, ch);
            let key = (s0, s1, x, y);
            let better = match best {
                None => true,
                Some(b) => lex(key.0, b.0)
                    .then(lex(key.1, b.1))
                    .then(lex(key.2, b.2))
                    .then(lex(key.3, b.3))
                    .is_lt(),
            };
            if better {
                best = Some(key);
            }
        }
        let Some((_, _, x, y)) = best else {
            out.push(None);
            continue;
        };
        out.push(Some((x, y)));
        let placed = (x, y, cw, ch);
        let mut pieces: Vec<Rect> = Vec::with_capacity(free.len() * 2);
        for &rect in &free {
            split(rect, placed, &mut pieces);
        }
        free = prune(pieces);
    }
    out
}

/// Run every heuristic and keep the best result: fewest unplaced cells, then
/// the smallest bounding box of the placed cells, then the earliest heuristic.
fn best_packing(
    cells: &[(f64, f64)],
    sheet: (f64, f64),
    ho: f64,
    grid: f64,
) -> (Vec<Option<Point>>, String) {
    let mut best: Option<(usize, f64, usize)> = None;
    let mut best_out: Vec<Option<Point>> = Vec::new();
    let mut best_name = HEURISTICS[0];
    for (rank, name) in HEURISTICS.iter().enumerate() {
        let out = maxrects(cells, sheet, name, ho, grid);
        let unplaced = out.iter().filter(|p| p.is_none()).count();
        let mut bw: f64 = 0.0;
        let mut bh: f64 = 0.0;
        for (i, spot) in out.iter().enumerate() {
            if let Some((x, y)) = *spot {
                bw = bw.max(x + cells[i].0);
                bh = bh.max(y + cells[i].1);
            }
        }
        let key = (unplaced, bw * bh, rank);
        let better = match best {
            None => true,
            Some(b) => key
                .0
                .cmp(&b.0)
                .then(lex(key.1, b.1))
                .then(key.2.cmp(&b.2))
                .is_lt(),
        };
        if better {
            best = Some(key);
            best_out = out;
            best_name = name;
        }
    }
    (best_out, best_name.to_string())
}

// --------------------------------------------------------------------------- //
// Packing
// --------------------------------------------------------------------------- //

/// Translate the packed coordinates by `offset` along one axis.
///
/// Floating point addition is not associative, so `(start + offset) + size` can
/// miss `(start + size) + offset` by one ULP and two cells that touch in the
/// bin would overlap by ~1e-13 mm on the sheet. Every translated coordinate is
/// therefore snapped onto the far edge of an already translated cell when the
/// two are within `ALIGN_EPS`.
fn aligned(starts: &[f64], sizes: &[f64], offset: f64) -> Vec<f64> {
    let mut out = vec![0.0; starts.len()];
    let mut edges: Vec<f64> = Vec::new();
    let mut order: Vec<usize> = (0..starts.len()).collect();
    order.sort_by(|&a, &b| {
        lex(starts[a], starts[b])
            .then(lex(sizes[a], sizes[b]))
            .then(a.cmp(&b))
    });
    for i in order {
        let mut value = starts[i] + offset;
        for &edge in &edges {
            if (edge - value).abs() <= ALIGN_EPS {
                value = edge;
                break;
            }
        }
        out[i] = value;
        let far = value + sizes[i];
        if edges.iter().all(|&edge| (edge - far).abs() > ALIGN_EPS) {
            edges.push(far);
        }
    }
    out
}

fn block_of(areas: &[Area]) -> Bounds {
    if areas.is_empty() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let mut b = (
        f64::INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::NEG_INFINITY,
    );
    for a in areas {
        b.0 = b.0.min(a.x);
        b.1 = b.1.min(a.y);
        b.2 = b.2.max(a.x + a.w);
        b.3 = b.3.max(a.y + a.h);
    }
    b
}

/// Pack the enabled sides (ordered by config.layout.sort) onto the sheet.
///
/// The packing is deterministic: the sides are ordered by `config.layout.sort`
/// and their cells (`board + gap`, grown for the datum) are packed with
/// MaxRects; the block of placed cells is then centred on the stencil (by a
/// whole number of raster pitches, so the jig pins stay on the raster). Cells
/// that fit nowhere are lined up to the right of the sheet and make
/// `Layout.fits` false.
pub fn pack(projects: &[Project], sides: &[SideId], config: &Config) -> Layout {
    let params = &config.layout;
    let (sheet_w, sheet_h) = config.sheet_size();
    let grid = grid_of(params); // slot_pitch (slots) or hole_grid (holes)
    let ho = params.pin_offset(); // cell edge to pin centre, for both datums

    let order = ordered_sides(projects, sides, params);
    let cells: Vec<(f64, f64)> = order
        .iter()
        .map(|id| {
            let side = id.get(projects);
            // gap/2 per side, then grown for the datum (slot raster / hole grid).
            cell_of(side.board_width(), side.board_height(), params, grid)
        })
        .collect();

    // 1. Pack, then line the cells that fit nowhere up right of the sheet.
    let (mut spots, heuristic) = best_packing(&cells, (sheet_w, sheet_h), ho, grid);
    let spilled: Vec<bool> = spots.iter().map(|s| s.is_none()).collect();
    let overflow = spilled.iter().filter(|s| **s).count();
    let mut x = snap_up(sheet_w, ho, grid);
    let y = snap_up(0.0, ho, grid);
    for i in 0..spots.len() {
        if spilled[i] {
            spots[i] = Some((x, y));
            x = snap_up(x + cells[i].0, ho, grid);
        }
    }

    // 2. Centre the block of *placed* cells on the stencil (overflow moves too).
    //    The offset is a whole number of raster pitches, so every pin stays on
    //    the raster measured from the sheet origin.
    let mut block_w: f64 = f64::NEG_INFINITY;
    let mut block_h: f64 = f64::NEG_INFINITY;
    for (i, spot) in spots.iter().enumerate() {
        if spilled[i] {
            continue;
        }
        let (sx, sy) = spot.unwrap();
        block_w = block_w.max(sx + cells[i].0);
        block_h = block_h.max(sy + cells[i].1);
    }
    let (ox, oy) = if block_w.is_finite() {
        (
            snap_down(((sheet_w - block_w) / 2.0).max(0.0), grid),
            snap_down(((sheet_h - block_h) / 2.0).max(0.0), grid),
        )
    } else {
        (0.0, 0.0) // every cell overflowed (or there are none)
    };

    // 3. Translate every cell onto the sheet, keeping touching edges identical.
    let sx: Vec<f64> = spots.iter().map(|s| s.unwrap().0).collect();
    let sy: Vec<f64> = spots.iter().map(|s| s.unwrap().1).collect();
    let cw_all: Vec<f64> = cells.iter().map(|c| c.0).collect();
    let ch_all: Vec<f64> = cells.iter().map(|c| c.1).collect();
    let xs = aligned(&sx, &cw_all, ox);
    let ys = aligned(&sy, &ch_all, oy);

    // 4. The cells themselves.
    let mut areas: Vec<Area> = Vec::with_capacity(order.len());
    let mut seen = Dedupe::new(HOLE_EPS);
    for (index, id) in order.iter().enumerate() {
        let side = id.get(projects);
        let (cw, ch) = cells[index];
        let (cx, cy) = (xs[index], ys[index]);
        let (minx, miny, maxx, maxy) = side.board_bbox;
        let (board_w, board_h) = (maxx - minx, maxy - miny);
        // The board is centred inside its cell: gap/2 on every side, more when
        // the cell was grown for the datum.
        let bx = cx + (cw - board_w) / 2.0;
        let by = cy + (ch - board_h) / 2.0;
        // Board -> sheet. A mirrored board occupies [-maxx, -minx] in x.
        let dx = bx - if side.mirror { -maxx } else { minx };
        let dy = by - miny;
        let (cell_holes, slots, pins) = datum_of((cx, cy, cw, ch), params, &mut seen);
        areas.push(Area {
            side: *id,
            x: cx,
            y: cy,
            w: cw,
            h: ch,
            board_rect: (bx, by, bx + board_w, by + board_h),
            transform: Transform {
                mirror: side.mirror,
                dx,
                dy,
            },
            row: index, // informational: the packing order
            // Cells that fit nowhere are drawn outside the stencil.
            overflow: spilled[index],
            holes: cell_holes,
            slots,
            pins,
            // The corner the piece is pushed into (a mirrored side has it at
            // the board's physical bottom-right).
            datum_corner: (cx, cy),
            // The X that says which corner that is (slots datum, marker on).
            marker: marker_of(cx, cy, params),
        });
    }

    let block = block_of(&areas); // the dividers use the same boundary
    let fits = overflow == 0
        && block.0 >= -FIT_EPS
        && block.1 >= -FIT_EPS
        && block.2 <= sheet_w + FIT_EPS
        && block.3 <= sheet_h + FIT_EPS;

    let dividers = dividers_of(&areas, params.outer_border, params.dot_line_gap);
    let cover = Cover::new(&areas, params);
    // `Layout` has nowhere to keep the number of dots the clearance rule
    // removed, so the report recomputes it with `dropped_dots`.
    let (dots, _dropped) = dots_of(&dividers, params, &cover);
    Layout {
        params: params.clone(),
        areas,
        width: sheet_w,
        height: sheet_h,
        block,
        fits,
        dots,
        dividers,
        heuristic,
        overflow,
    }
}

/// Convenience: pack every side of every project.
pub fn pack_all(projects: &[Project], config: &Config) -> Layout {
    pack(projects, &all_sides(projects), config)
}

// --------------------------------------------------------------------------- //
// Dividers and dots
// --------------------------------------------------------------------------- //

/// Maps coordinates that are within `tol` of each other onto one value.
struct Snap {
    tol: f64,
    values: Vec<f64>,
}

impl Snap {
    fn new(tol: f64) -> Self {
        Self {
            tol,
            values: Vec::new(),
        }
    }
    fn snap(&mut self, value: f64) -> f64 {
        for &known in &self.values {
            if (known - value).abs() <= self.tol {
                return known;
            }
        }
        self.values.push(value);
        value
    }
}

/// Merge overlapping or touching 1-D intervals.
fn merge(mut intervals: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    intervals.sort_by(|a, b| lex(a.0, b.0).then(lex(a.1, b.1)));
    let mut out: Vec<(f64, f64)> = Vec::new();
    for (lo, hi) in intervals {
        if hi - lo <= MERGE_EPS {
            continue;
        }
        match out.last_mut() {
            Some(last) if lo <= last.1 + MERGE_EPS => {
                if hi > last.1 {
                    last.1 = hi;
                }
            }
            _ => out.push((lo, hi)),
        }
    }
    out
}

/// One divider line being collected: `(orientation, coordinate, its pieces)`.
type Line = (u8, f64, Vec<(f64, f64)>);

/// The dotted lines, as `(x0, y0, x1, y1)`.
///
/// Every cell owns one dotted line along each of its four edges, running
/// `line_gap / 2` *inside* that edge; the extent of a line is the extent of its
/// edge shortened by the same `line_gap / 2` at both ends, so the four lines of
/// a cell are exactly the cell rectangle inset by `line_gap / 2`. Collinear
/// pieces on one line are merged. Lines whose *edge* lies on the outer boundary
/// of the block are dropped unless `outer`.
fn dividers_of(areas: &[Area], outer: bool, line_gap: f64) -> Vec<(f64, f64, f64, f64)> {
    let (minx, miny, maxx, maxy) = block_of(areas);
    let boundary = |orientation: u8| {
        if orientation == VERTICAL {
            (minx, maxx)
        } else {
            (miny, maxy)
        }
    };

    let half = line_gap.max(0.0) / 2.0;
    let mut snap_v = Snap::new(MERGE_EPS);
    let mut snap_h = Snap::new(MERGE_EPS);
    // (orientation, snapped line coordinate) -> the pieces of that line
    let mut lines: Vec<Line> = Vec::new();
    for area in areas {
        let (x0, y0, x1, y1) = area.rect();
        // This cell's own rectangle, inset by line_gap/2 (clamped so it never
        // turns inside out): the four lines run along its sides.
        let ix = half.min((x1 - x0) / 2.0);
        let iy = half.min((y1 - y0) / 2.0);
        let (lox, hix, loy, hiy) = (x0 + ix, x1 - ix, y0 + iy, y1 - iy);
        // (orientation, the cell edge, its line, the extent of that line)
        let raw = [
            (VERTICAL, x0, lox, loy, hiy),
            (VERTICAL, x1, hix, loy, hiy),
            (HORIZONTAL, y0, loy, lox, hix),
            (HORIZONTAL, y1, hiy, lox, hix),
        ];
        for (orientation, edge, line, lo, hi) in raw {
            if hi - lo <= MERGE_EPS {
                continue; // no room left for this line
            }
            let (b0, b1) = boundary(orientation);
            if !outer && ((edge - b0).abs() <= MERGE_EPS || (edge - b1).abs() <= MERGE_EPS) {
                continue; // edge on the outer boundary of the block
            }
            let key = if orientation == VERTICAL {
                snap_v.snap(line)
            } else {
                snap_h.snap(line)
            };
            match lines
                .iter_mut()
                .find(|(o, l, _)| *o == orientation && *l == key)
            {
                Some((_, _, parts)) => parts.push((lo, hi)),
                None => lines.push((orientation, key, vec![(lo, hi)])),
            }
        }
    }

    lines.sort_by(|a, b| a.0.cmp(&b.0).then(lex(a.1, b.1)));
    let mut out: Vec<(f64, f64, f64, f64)> = Vec::new();
    for (orientation, line, parts) in lines {
        for (lo, hi) in merge(parts) {
            out.push(if orientation == VERTICAL {
                (line, lo, line, hi)
            } else {
                (lo, line, hi, line)
            });
        }
    }
    out
}

/// Dot centres along the dividers, centred on each segment and deduplicated.
///
/// Returns `(dots, dropped)`: a dot too close to a datum opening (`cover`) is
/// dropped and counted.
fn dots_of(
    dividers: &[(f64, f64, f64, f64)],
    params: &LayoutParams,
    cover: &Cover,
) -> (Vec<Point>, usize) {
    let pitch = params.dot_pitch;
    let mut dots: Vec<Point> = Vec::new();
    let mut dropped = 0usize;
    let push = |x: f64, y: f64, dots: &mut Vec<Point>, dropped: &mut usize| {
        if cover.near(x, y) {
            *dropped += 1;
        } else {
            dots.push((x, y));
        }
    };
    for &(x0, y0, x1, y1) in dividers {
        let length = (x1 - x0).hypot(y1 - y0);
        if length <= 0.0 {
            push(x0, y0, &mut dots, &mut dropped);
            continue;
        }
        let (ux, uy) = ((x1 - x0) / length, (y1 - y0) / length);
        let n = if pitch > 0.0 {
            (length / pitch).floor() as i64
        } else {
            0
        };
        let start = (length - n as f64 * pitch) / 2.0; // n == 0 -> one dot in the middle
        for i in 0..=n {
            let d = start + i as f64 * pitch;
            push(x0 + ux * d, y0 + uy * d, &mut dots, &mut dropped);
        }
    }
    let mut keep = Dedupe::new(params.dot_dia);
    let kept = dots.into_iter().filter(|p| keep.add(p.0, p.1)).collect();
    (kept, dropped)
}

/// How many divider dots the layout dropped for the datum clearance.
///
/// Recomputed from the layout (the count is not stored in `Layout`).
pub fn dropped_dots(layout: &Layout) -> usize {
    let cover = Cover::new(&layout.areas, &layout.params);
    dots_of(&layout.dividers, &layout.params, &cover).1
}

/// Distance from `(px, py)` to the segment `a-b`.
fn segment_distance(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    let (dx, dy) = (bx - ax, by - ay);
    let span = dx * dx + dy * dy;
    let t = if span <= 0.0 {
        0.0
    } else {
        (((px - ax) * dx + (py - ay) * dy) / span).clamp(0.0, 1.0)
    };
    (px - (ax + t * dx)).hypot(py - (ay + t * dy))
}

/// One datum feature as a capsule: the segment `a-b` grown by `r`.
type Shape = (f64, f64, f64, f64, f64);

/// The datum features of a layout, hashed so a clearance test is O(1).
///
/// Every feature is stored as a *capsule* - a segment plus a radius, the set of
/// points within that radius of the segment - which makes the distance from a
/// point to it `segment_distance(point, segment) - radius`:
///
/// * an obround slot is its centre segment (length `slot_length - slot_width`,
///   along the slot's axis) grown by `slot_width / 2`;
/// * a round dowel hole is its centre grown by `hole_dia / 2`;
/// * an X marker is its two `dot_dia` wide strokes, so two capsules of radius
///   `dot_dia / 2` ([`marker_strokes`]), and the distance to the X is the
///   smaller of the two.
///
/// [`Cover::near`] answers the question [`dots_of`] asks: would a dot of
/// `dot_dia` centred there leave less than `dot_clearance` of metal to any
/// feature? That is `distance < dot_dia / 2 + dot_clearance`, so with
/// `dot_clearance = 0` only a dot that actually overlaps a feature goes.
/// Capsules are bucketed by their bounding box grown by that same margin, so a
/// dot only has to be measured against the features in its own bucket.
struct Cover {
    /// Metal a dot must leave: its own radius plus the clearance.
    margin: f64,
    cell: f64,
    buckets: HashMap<(i64, i64), Vec<Shape>>,
}

impl Cover {
    fn new(areas: &[Area], params: &LayoutParams) -> Self {
        let margin = params.dot_dia.max(0.0) / 2.0 + params.dot_clearance.max(0.0);
        let stroke = params.dot_dia.max(0.0) / 2.0; // the marker strokes' half width
        let mut shapes: Vec<Shape> = Vec::new();
        for area in areas {
            for &(cx, cy, w, h) in &area.slots {
                // The obround is the segment between its two cap centres,
                // grown by half the short axis.
                let r = w.min(h) / 2.0;
                let ex = (w / 2.0 - r).max(0.0);
                let ey = (h / 2.0 - r).max(0.0);
                shapes.push((cx - ex, cy - ey, cx + ex, cy + ey, r));
            }
            if params.holes() {
                let r = params.hole_dia / 2.0;
                for &(hx, hy) in &area.holes {
                    shapes.push((hx, hy, hx, hy, r));
                }
            }
            if let Some(centre) = area.marker {
                for (x0, y0, x1, y1) in marker_strokes(centre, params.marker_size) {
                    shapes.push((x0, y0, x1, y1, stroke));
                }
            }
        }
        let biggest = shapes
            .iter()
            .map(|&(ax, ay, bx, by, r)| (bx - ax).abs().max((by - ay).abs()) + 2.0 * r)
            .fold(0.0f64, f64::max);
        let cell = (biggest + 2.0 * margin).max(1.0);
        let mut buckets: HashMap<(i64, i64), Vec<Shape>> = HashMap::new();
        for shape in shapes {
            let (ax, ay, bx, by, r) = shape;
            let grown = r + margin;
            let (x0, y0) = (ax.min(bx) - grown, ay.min(by) - grown);
            let (x1, y1) = (ax.max(bx) + grown, ay.max(by) + grown);
            for i in (x0 / cell).floor() as i64..=(x1 / cell).floor() as i64 {
                for j in (y0 / cell).floor() as i64..=(y1 / cell).floor() as i64 {
                    buckets.entry((i, j)).or_default().push(shape);
                }
            }
        }
        Self {
            margin,
            cell,
            buckets,
        }
    }

    /// Metal between `(x, y)` and the nearest feature (`inf`: none near).
    ///
    /// Negative inside a feature. Only the features filed under the point's own
    /// bucket are measured, which is every one that could be within `margin`.
    fn distance(&self, x: f64, y: f64) -> f64 {
        if self.buckets.is_empty() {
            return f64::INFINITY;
        }
        let key = (
            (x / self.cell).floor() as i64,
            (y / self.cell).floor() as i64,
        );
        match self.buckets.get(&key) {
            None => f64::INFINITY,
            Some(shapes) => shapes
                .iter()
                .map(|&(ax, ay, bx, by, r)| segment_distance(x, y, ax, ay, bx, by) - r)
                .fold(f64::INFINITY, f64::min),
        }
    }

    /// Would a dot centred at `(x, y)` come closer than the clearance?
    fn near(&self, x: f64, y: f64) -> bool {
        self.distance(x, y) < self.margin
    }
}

/// Spatial hash that rejects points closer than `min_dist` to a kept one.
struct Dedupe {
    min_dist: f64,
    limit: f64,
    cells: HashMap<(i64, i64), Vec<Point>>,
}

impl Dedupe {
    fn new(min_dist: f64) -> Self {
        Self {
            min_dist,
            limit: min_dist * min_dist,
            cells: HashMap::new(),
        }
    }

    /// Remember `(x, y)` and return `true` when it is a new point.
    fn add(&mut self, x: f64, y: f64) -> bool {
        if self.min_dist <= 0.0 {
            return true;
        }
        let cx = (x / self.min_dist).floor() as i64;
        let cy = (y / self.min_dist).floor() as i64;
        for i in cx - 1..=cx + 1 {
            for j in cy - 1..=cy + 1 {
                if let Some(points) = self.cells.get(&(i, j)) {
                    for &(px, py) in points {
                        if (px - x).powi(2) + (py - y).powi(2) < self.limit {
                            return false;
                        }
                    }
                }
            }
        }
        self.cells.entry((cx, cy)).or_default().push((x, y));
        true
    }
}

// --------------------------------------------------------------------------- //
// Number formatting (Python's format specs)
// --------------------------------------------------------------------------- //

/// Python's `f"{v:g}"` (C `%g` with precision 6).
fn fmt_g(v: f64) -> String {
    if v.is_nan() {
        return "nan".to_string();
    }
    if v.is_infinite() {
        return if v < 0.0 { "-inf" } else { "inf" }.to_string();
    }
    const P: i32 = 6;
    let sci = format!("{:.*e}", (P - 1) as usize, v);
    let exp: i32 = sci
        .rfind('e')
        .and_then(|i| sci[i + 1..].parse().ok())
        .unwrap_or(0);
    if (-4..P).contains(&exp) {
        let decimals = (P - 1 - exp).max(0) as usize;
        let s = format!("{v:.decimals$}");
        return strip_zeros(&s).to_string();
    }
    let (mantissa, _) = sci.split_at(sci.rfind('e').unwrap());
    format!(
        "{}e{}{:02}",
        strip_zeros(mantissa),
        if exp < 0 { '-' } else { '+' },
        exp.abs()
    )
}

fn strip_zeros(s: &str) -> &str {
    if !s.contains('.') {
        return s;
    }
    s.trim_end_matches('0').trim_end_matches('.')
}

/// Python's `f"{v:,.1f}"`: one decimal, comma thousands separators.
fn fmt_thousands_1(v: f64) -> String {
    let text = format!("{v:.1}");
    let (sign, rest) = match text.strip_prefix('-') {
        Some(r) => ("-", r),
        None => ("", text.as_str()),
    };
    let (int_part, frac) = rest.split_once('.').unwrap_or((rest, ""));
    let mut grouped = String::new();
    for (i, ch) in int_part.chars().enumerate() {
        if i > 0 && (int_part.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("{sign}{grouped}.{frac}")
}

fn fmt_point(p: Point) -> String {
    format!("({:.2}, {:.2})", p.0, p.1)
}

// --------------------------------------------------------------------------- //
// Report
// --------------------------------------------------------------------------- //

/// Why the slots of one cell would run into each other, or `""`.
fn crowded(params: &LayoutParams) -> String {
    if params.slot_pitch <= 0.0 {
        return "slot_pitch must be positive: no slots are cut at all".to_string();
    }
    if params.slot_length > params.slot_pitch + FIT_EPS {
        return format!(
            "slot_length {} mm is longer than the {} mm raster: two slots on one edge overlap",
            fmt_g(params.slot_length),
            fmt_g(params.slot_pitch)
        );
    }
    let first = slot_offsets(1000.0 * params.slot_pitch.max(1.0), params);
    if let Some(&first) = first.first() {
        if first < params.slot_inner() + params.slot_length / 2.0 - FIT_EPS {
            return "the first raster position is so close to the corner that the first bottom \
                    slot and the first left slot overlap there"
                .to_string();
        }
    }
    String::new()
}

/// Why the X marker does not fit where it is cut, or `""`.
fn marker_trouble(layout: &Layout) -> String {
    let params = &layout.params;
    let half = marker_half(params);
    if half > params.pin_offset() + FIT_EPS {
        return format!(
            "the {} mm X reaches {:.2} mm from its centre but is cut only {:.2} mm inside the \
             cell edge: it crosses the edge",
            fmt_g(params.marker_size),
            half,
            params.pin_offset()
        );
    }
    for area in &layout.areas {
        let Some((mx, my)) = area.marker else {
            continue;
        };
        for &(sx, sy, sw, sh) in &area.slots {
            if (sx - mx).abs() < (sw + 2.0 * half) / 2.0 - FIT_EPS
                && (sy - my).abs() < (sh + 2.0 * half) / 2.0 - FIT_EPS
            {
                return format!(
                    "the {} mm X at {} reaches the slot at {}",
                    fmt_g(params.marker_size),
                    fmt_point((mx, my)),
                    fmt_point((sx, sy))
                );
            }
        }
    }
    String::new()
}

/// How many slots `area` carries on its bottom edge and on its left edge.
fn edge_counts(area: &Area, params: &LayoutParams) -> (usize, usize) {
    (slot_count(area.w, params), slot_count(area.h, params))
}

/// Is `value` a whole number of `grid`, measured from the sheet origin?
fn on_grid(value: f64, grid: f64) -> bool {
    if grid <= 0.0 {
        return true;
    }
    let steps = value / grid;
    (steps - steps.round_ties_even()).abs() * grid <= FIT_EPS
}

/// Is every jig pin centre of the sheet on the datum's raster?
fn pins_on_grid(layout: &Layout) -> bool {
    let grid = grid_of(&layout.params);
    if grid <= 0.0 {
        return true;
    }
    layout
        .areas
        .iter()
        .flat_map(|a| a.pins.iter())
        .all(|&(px, py)| on_grid(px, grid) && on_grid(py, grid))
}

/// Extra cell area (mm2) the datum costs, against plain `board + gap`.
fn grid_waste(projects: &[Project], layout: &Layout) -> f64 {
    let gap = layout.params.gap;
    // Summed left to right from 0.0, like the original's `waste += ...` (so an
    // empty layout gives +0.0, not the -0.0 of `Iterator::sum`).
    layout.areas.iter().fold(0.0, |waste, area| {
        let side = area.side.get(projects);
        waste + (area.w * area.h - (side.board_width() + gap) * (side.board_height() + gap))
    })
}

/// Human readable report of the layout (stencil, block, datum, per cell).
pub fn layout_report(projects: &[Project], layout: &Layout, config: &Config) -> String {
    let params = &layout.params;
    let mut lines: Vec<String> = Vec::new();
    let orientation = if config.orientation == ORIENTATION_PORTRAIT {
        "portrait"
    } else {
        "landscape"
    };
    lines.push(format!(
        "Stencil: {:.1} x {:.1} mm ({} {})",
        layout.width,
        layout.height,
        config.size_label(),
        orientation
    ));
    let (bx0, by0, bx1, by1) = layout.block;
    lines.push(format!(
        "Block:   {:.2} x {:.2} mm at {}..{}   {}",
        layout.block_width(),
        layout.block_height(),
        fmt_point((bx0, by0)),
        fmt_point((bx1, by1)),
        if layout.fits { "FITS" } else { "DOES NOT FIT" }
    ));
    let spilled = layout.overflow;
    let heuristic = if layout.heuristic.is_empty() {
        "unknown"
    } else {
        &layout.heuristic
    };
    // Cells grow for the slot raster and for the hole grid, so gap/2 is then
    // only the *minimum* padding.
    let floor_pad = if params.slots() {
        params.pad().max(params.min_pad_for_slots())
    } else {
        params.pad()
    };
    let exact_pad = grid_of(params) <= 0.0 && !params.slots();
    let padding = if exact_pad {
        format!("padding {floor_pad:.1} mm")
    } else {
        format!("padding ≥ {floor_pad:.2} mm")
    };
    lines.push(format!(
        "Cells:   {} placed with MaxRects ({}), {} overflow, gap {:.1} mm ({}), sort by {}",
        layout.areas.len() - spilled,
        heuristic,
        spilled,
        params.gap,
        padding,
        params.sort
    ));
    if params.slots() {
        let mut counts: Vec<(usize, usize)> = layout
            .areas
            .iter()
            .map(|area| edge_counts(area, params))
            .collect();
        counts.sort_unstable();
        counts.dedup();
        let spread = if counts.is_empty() {
            "none".to_string()
        } else {
            counts
                .iter()
                .map(|(b, l)| format!("{b}+{l}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        lines.push(format!(
            "Datum:   slots at a {} mm raster along the bottom and left edges, outer wall {} mm \
             inside the cell edge, inner wall {:.1} mm, pins ⌀{} mm tangent to the inner walls on \
             the same raster, push toward the bottom-left corner",
            fmt_g(params.slot_pitch),
            fmt_g(params.slot_offset),
            params.slot_inner(),
            fmt_g(params.pin_dia)
        ));
        lines.push(format!(
            "         {} x {} mm obround; every raster position that fits is opened, so a modular \
             jig may use any of them (bottom+left per cell: {})",
            fmt_g(params.slot_width),
            fmt_g(params.slot_length),
            spread
        ));
        lines.push(format!(
            "         cells are whole multiples of the raster, at least two slots on the longer \
             edge and one on the shorter; every slot lies inside its own cell, ≥ {} mm of foil to \
             the board",
            fmt_g(params.slot_web)
        ));
        let crowd = crowded(params);
        if !crowd.is_empty() {
            lines.push(format!("         WARNING: {crowd}"));
        }
        let band = (params.dot_line_gap / 2.0 + params.dot_dia / 2.0).max(params.dot_dia);
        if params.slot_offset < band {
            lines.push(format!(
                "         the slots clip this cell's own dotted line ({:.2} mm inside the edge) \
                 and stop {} mm short of the neighbouring cell: intended, it frees the foil \
                 beside the board for the squeegee",
                params.dot_line_gap / 2.0,
                fmt_g(params.slot_offset)
            ));
        }
        if params.marker {
            lines.push(format!(
                "Marker:  X {} mm at the raster point {} mm inside the datum corner, where the \
                 left pin column meets the bottom pin row (the one raster point that never \
                 carries a slot)",
                fmt_g(params.marker_size),
                fmt_g(params.pin_offset())
            ));
            lines.push(format!(
                "         two crossed strokes at ±45°, {} mm wide, reaching {:.2} mm from the \
                 centre: the orientation of a cut-out piece can be read at a glance",
                fmt_g(params.dot_dia),
                marker_half(params)
            ));
            let trouble = marker_trouble(layout);
            if !trouble.is_empty() {
                lines.push(format!("         WARNING: {trouble}"));
            }
        } else {
            lines.push("Marker:  off (no orientation X is cut)".to_string());
        }
    } else if params.holes() {
        lines.push(format!(
            "Datum:   holes: ⌀{:.1} mm, inset {:.1} mm (centres {:.2} mm inside the cell edge)",
            params.hole_dia,
            params.hole_inset,
            params.hole_offset()
        ));
    } else {
        lines.push("Datum:   none".to_string());
    }
    let raster = grid_of(params);
    if raster > 0.0 {
        let count: usize = layout.areas.iter().map(|a| a.pins.len()).sum();
        let verdict = if pins_on_grid(layout) { "yes" } else { "NO" };
        let source = if params.slots() {
            "slot_pitch"
        } else {
            "hole_grid"
        };
        lines.push(format!(
            "Grid:    pin centres on the {raster:.1} mm {source} raster: {verdict} ({count} \
             pin(s), measured from the sheet origin)"
        ));
        lines.push(format!(
            "         cells grown for the raster: {} mm2 more than board + gap (the least this \
             raster allows)",
            fmt_thousands_1(grid_waste(projects, layout))
        ));
        if params.slots() && params.hole_grid > 0.0 {
            lines.push(format!(
                "         hole_grid ({} mm) is set but applies to the holes datum only",
                fmt_g(params.hole_grid)
            ));
        }
    } else if params.datum == DATUM_NONE {
        lines.push("Grid:    off (datum none: no pins to align)".to_string());
    } else {
        lines.push("Grid:    off (cells are exactly board + gap)".to_string());
    }

    for (index, area) in layout.areas.iter().enumerate() {
        let side = area.side.get(projects);
        let label = format!(
            "{}{}",
            side.label(),
            if side.mirror { " (mirrored)" } else { "" }
        );
        let (bx0, by0, bx1, by1) = area.board_rect;
        let spill = if area.overflow { " OVERFLOW" } else { "" };
        lines.push(String::new());
        lines.push(format!(
            "{}. {}   [cell {}{}]",
            index + 1,
            label,
            area.row,
            spill
        ));
        lines.push(format!(
            "   cell   x={:.2} y={:.2} w={:.2} h={:.2} mm",
            area.x, area.y, area.w, area.h
        ));
        lines.push(format!(
            "   board  x {:.2}..{:.2}  y {:.2}..{:.2} mm ({:.2} x {:.2} mm)",
            bx0,
            bx1,
            by0,
            by1,
            bx1 - bx0,
            by1 - by0
        ));
        if params.slots() {
            let (dcx, dcy) = area.datum_corner;
            let note = if side.mirror {
                "  (mirrored: the board's physical bottom-right)"
            } else {
                ""
            };
            lines.push(format!(
                "   datum corner {}   rel {}{}",
                fmt_point((dcx, dcy)),
                fmt_point((dcx - bx0, dcy - by0)),
                note
            ));
            if let Some((mx, my)) = area.marker {
                lines.push(format!(
                    "   marker X ({} mm strokes) {}   rel {}",
                    fmt_g(params.marker_size),
                    fmt_point((mx, my)),
                    fmt_point((mx - bx0, my - by0))
                ));
            }
            let (bottom, left) = edge_counts(area, params);
            lines.push(format!(
                "   slots ({} x {} mm obround), {} on the bottom edge + {} on the left edge, \
                 sheet / relative to board corner {}:",
                fmt_g(params.slot_width),
                fmt_g(params.slot_length),
                bottom,
                left,
                fmt_point((bx0, by0))
            ));
            for &(sx, sy, sw, sh) in &area.slots {
                lines.push(format!(
                    "     {} {:.2} x {:.2} mm   rel {}",
                    fmt_point((sx, sy)),
                    sw,
                    sh,
                    fmt_point((sx - bx0, sy - by0))
                ));
            }
        } else if !area.holes.is_empty() {
            lines.push(format!(
                "   dowel holes (⌀{:.1} mm), sheet / relative to board corner {}:",
                params.hole_dia,
                fmt_point((bx0, by0))
            ));
            for &(hx, hy) in &area.holes {
                lines.push(format!(
                    "     {}   rel {}",
                    fmt_point((hx, hy)),
                    fmt_point((hx - bx0, hy - by0))
                ));
            }
        } else if params.holes() {
            lines.push("   dowel holes: none (shared with a neighbouring cell)".to_string());
        } else {
            lines.push("   datum features: none".to_string());
        }
        if !area.pins.is_empty() {
            let pin_dia = if params.slots() {
                params.pin_dia
            } else {
                params.hole_dia
            };
            lines.push(format!(
                "   jig pins (⌀{} mm), sheet / relative to board corner {}:",
                fmt_g(pin_dia),
                fmt_point((bx0, by0))
            ));
            let grid = grid_of(params);
            for &(px, py) in &area.pins {
                let off = if grid <= 0.0 || (on_grid(px, grid) && on_grid(py, grid)) {
                    ""
                } else {
                    "   OFF THE RASTER (the cell is too small for it)"
                };
                lines.push(format!(
                    "     {}   rel {}{}",
                    fmt_point((px, py)),
                    fmt_point((px - bx0, py - by0)),
                    off
                ));
            }
        }
        if params.slots() {
            let board = if side.mirror {
                "+x and -y in board coordinates, the board's physical bottom-right"
            } else {
                "-x and -y in board coordinates"
            };
            lines.push(format!(
                "   nesting: push toward the datum corner, sheet -x and -y ({board}); first -y \
                 onto the bottom pins, then -x onto the left one(s)"
            ));
        }

        let mut open = 0usize;
        let mut ignore = 0usize;
        let mut undefined = 0usize;
        let mut candidates = 0usize;
        for pad in side.candidates() {
            candidates += 1;
            match pad.state.as_str() {
                STATE_OPEN => open += 1,
                STATE_IGNORE => ignore += 1,
                STATE_UNDEFINED => undefined += 1,
                _ => {}
            }
        }
        lines.push(format!("   paste openings: {}", side.paste_objects().len()));
        lines.push(format!(
            "   candidate pads: {candidates} (open {open}, ignore {ignore}, undefined {undefined})"
        ));
        lines.push(format!(
            "   closed openings: {}",
            side.closed_pads().count()
        ));
    }

    lines.push(String::new());
    let inset = if params.dot_line_gap > 0.0 {
        format!("{:.2} mm inside the edge", params.dot_line_gap / 2.0)
    } else {
        "on the edge itself".to_string()
    };
    lines.push(format!(
        "Divider dots: {} (⌀{:.2} mm, pitch {:.2} mm, {} dotted line(s), one per cell edge, {}{}; \
         {} dropped within {} mm of a slot, hole or marker)",
        layout.dots.len(),
        params.dot_dia,
        params.dot_pitch,
        layout.dividers.len(),
        inset,
        if params.outer_border {
            ", block boundary included"
        } else {
            ""
        },
        dropped_dots(layout),
        fmt_g(params.dot_clearance)
    ));
    lines.join("\n")
}

// --------------------------------------------------------------------------- //
// Tests
// --------------------------------------------------------------------------- //

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        all_sides, Side, DATUM_HOLES, DATUM_NONE, SIDE_BOTTOM, SORT_HEIGHT, STENCIL_SIZES,
    };

    const EPS_T: f64 = 1e-9;

    fn side_of(project: &str, name: &str, bbox: Bounds, mirror: bool) -> Side {
        Side {
            project_name: project.to_string(),
            board_bbox: bbox,
            name: name.to_string(),
            copper: None,
            paste: None,
            mirror,
            enabled: true,
            pads: Vec::new(),
        }
    }

    /// One project per `(name, width, height)`, with a top and a bottom side.
    /// The board box is offset so a mirrored side is not a special case of 0.
    fn projects_of(dims: &[(&str, f64, f64)]) -> Vec<Project> {
        dims.iter()
            .enumerate()
            .map(|(i, &(name, w, h))| {
                let x0 = 10.0 + 3.0 * i as f64;
                let y0 = -40.0 - 2.0 * i as f64;
                let bbox = (x0, y0, x0 + w, y0 + h);
                Project {
                    name: name.to_string(),
                    source: String::new(),
                    outline: None,
                    bbox,
                    sides: vec![
                        side_of(name, SIDE_TOP, bbox, false),
                        side_of(name, SIDE_BOTTOM, bbox, true),
                    ],
                }
            })
            .collect()
    }

    fn config_of(size: (u32, u32), params: LayoutParams) -> Config {
        Config {
            size,
            layout: params,
            ..Config::default()
        }
    }

    fn slots_params() -> LayoutParams {
        LayoutParams::default()
    }

    fn holes_params() -> LayoutParams {
        LayoutParams {
            datum: DATUM_HOLES.to_string(),
            ..LayoutParams::default()
        }
    }

    fn none_params() -> LayoutParams {
        LayoutParams {
            datum: DATUM_NONE.to_string(),
            ..LayoutParams::default()
        }
    }

    /// A handful of boards of different shapes (13 cells when both sides count).
    fn sample() -> Vec<Project> {
        projects_of(&[
            ("alpha", 66.5, 14.0),
            ("Bravo", 100.0, 78.0),
            ("charlie10", 20.0, 36.2),
            ("charlie2", 88.4, 14.0),
            ("delta", 13.4, 11.6),
            ("echo", 50.8, 20.1),
            ("Foxtrot", 85.6, 54.0),
        ])
    }

    fn packed(projects: &[Project], size: (u32, u32), params: LayoutParams) -> (Layout, Config) {
        let config = config_of(size, params);
        let layout = pack(projects, &all_sides(projects), &config);
        (layout, config)
    }

    // -- cells ---------------------------------------------------------------

    #[test]
    fn cells_never_overlap() {
        for params in [slots_params(), holes_params(), none_params()] {
            let projects = sample();
            let (layout, _) = packed(&projects, (380, 280), params.clone());
            for (i, a) in layout.areas.iter().enumerate() {
                for b in layout.areas.iter().skip(i + 1) {
                    let dx = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
                    let dy = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
                    assert!(
                        dx <= 1e-9 || dy <= 1e-9,
                        "{}: cells {:?} and {:?} overlap by {dx} x {dy}",
                        params.datum,
                        a.rect(),
                        b.rect()
                    );
                }
            }
        }
    }

    #[test]
    fn board_is_centred_with_at_least_half_the_gap() {
        for params in [slots_params(), holes_params(), none_params()] {
            let projects = sample();
            let floor = if params.slots() {
                params.pad().max(params.min_pad_for_slots())
            } else {
                params.pad()
            };
            let (layout, _) = packed(&projects, (600, 600), params.clone());
            for area in &layout.areas {
                let (bx0, by0, bx1, by1) = area.board_rect;
                // centred
                assert!((bx0 - area.x - (area.x + area.w - bx1)).abs() < 1e-9);
                assert!((by0 - area.y - (area.y + area.h - by1)).abs() < 1e-9);
                // padded
                for pad in [
                    bx0 - area.x,
                    area.x + area.w - bx1,
                    by0 - area.y,
                    area.y + area.h - by1,
                ] {
                    assert!(
                        pad >= floor - 1e-9,
                        "{}: padding {pad} < {floor}",
                        params.datum
                    );
                }
            }
        }
    }

    #[test]
    fn slot_cells_are_whole_pitches_with_two_slots_on_the_longer_edge() {
        for pitch in [20.0, 30.0, 12.5] {
            let params = LayoutParams {
                slot_pitch: pitch,
                ..slots_params()
            };
            let projects = sample();
            let (layout, _) = packed(&projects, (700, 600), params.clone());
            assert!(!layout.areas.is_empty());
            for area in &layout.areas {
                for size in [area.w, area.h] {
                    let steps = size / pitch;
                    assert!(
                        (steps - steps.round()).abs() * pitch < 1e-9,
                        "cell {size} is not a multiple of {pitch}"
                    );
                }
                let (bottom, left) = edge_counts(area, &params);
                assert_eq!(bottom, area.slots.iter().take(bottom).count());
                assert!(
                    bottom.max(left) >= 2 && bottom.min(left) >= 1,
                    "cell {}x{} carries {bottom}+{left} slots",
                    area.w,
                    area.h
                );
                // the longer edge is the one with two of them
                if area.w > area.h + 1e-9 {
                    assert!(
                        bottom >= 2,
                        "{bottom}+{left} on a {}x{} cell",
                        area.w,
                        area.h
                    );
                } else if area.h > area.w + 1e-9 {
                    assert!(left >= 2, "{bottom}+{left} on a {}x{} cell", area.w, area.h);
                }
            }
        }
    }

    #[test]
    fn slots_lie_inside_their_cell_and_keep_the_web_to_the_board() {
        let params = slots_params();
        let projects = sample();
        let (layout, _) = packed(&projects, (700, 600), params.clone());
        for area in &layout.areas {
            let (x0, y0, x1, y1) = area.rect();
            let (bx0, by0, _, _) = area.board_rect;
            let (bottom, _) = edge_counts(area, &params);
            for (i, &(sx, sy, sw, sh)) in area.slots.iter().enumerate() {
                assert!(
                    sx - sw / 2.0 > x0 + EPS_T
                        && sx + sw / 2.0 < x1 - EPS_T
                        && sy - sh / 2.0 > y0 + EPS_T
                        && sy + sh / 2.0 < y1 - EPS_T,
                    "slot {i} ({sx}, {sy}) {sw}x{sh} is not strictly inside {:?}",
                    area.rect()
                );
                // The outer wall is slot_offset inside the cell edge.
                if i < bottom {
                    assert!((sy - sh / 2.0 - (y0 + params.slot_offset)).abs() < 1e-9);
                    assert!(
                        by0 - (sy + sh / 2.0) >= params.slot_web - 1e-9,
                        "bottom slot {i}: only {} mm of web",
                        by0 - (sy + sh / 2.0)
                    );
                } else {
                    assert!((sx - sw / 2.0 - (x0 + params.slot_offset)).abs() < 1e-9);
                    assert!(
                        bx0 - (sx + sw / 2.0) >= params.slot_web - 1e-9,
                        "left slot {i}: only {} mm of web",
                        bx0 - (sx + sw / 2.0)
                    );
                }
            }
        }
    }

    #[test]
    fn pins_sit_on_the_raster_tangent_to_the_inner_wall() {
        let params = slots_params();
        let projects = sample();
        let (layout, _) = packed(&projects, (700, 600), params.clone());
        let pitch = params.slot_pitch;
        let mut seen = 0;
        for area in &layout.areas {
            let (bottom, _) = edge_counts(area, &params);
            assert_eq!(area.pins.len(), area.slots.len());
            for (i, (&(px, py), &(sx, sy, _, sh))) in area.pins.iter().zip(&area.slots).enumerate()
            {
                seen += 1;
                for value in [px, py] {
                    let steps = value / pitch;
                    assert!(
                        (steps - steps.round()).abs() * pitch < 1e-9,
                        "pin coordinate {value} is off the {pitch} mm raster"
                    );
                }
                if i < bottom {
                    assert!((px - sx).abs() < 1e-9);
                    // the wall nearest the board, touched from below
                    assert!((py + params.pin_dia / 2.0 - (sy + sh / 2.0)).abs() < 1e-9);
                } else {
                    assert!((py - sy).abs() < 1e-9);
                    assert!(
                        (px + params.pin_dia / 2.0 - (sx + params.slot_width / 2.0)).abs() < 1e-9
                    );
                }
            }
        }
        assert!(seen > 0);
        assert!(pins_on_grid(&layout));
    }

    #[test]
    fn marker_is_on_the_raster_and_clear_of_slots_and_board() {
        let params = slots_params();
        let projects = sample();
        let (layout, _) = packed(&projects, (700, 600), params.clone());
        let half = marker_half(&params);
        for area in &layout.areas {
            let marker = area.marker.expect("the slots datum marks every cell");
            assert!((marker.0 - (area.x + params.pin_offset())).abs() < 1e-12);
            assert!((marker.1 - (area.y + params.pin_offset())).abs() < 1e-12);
            // inside its own cell
            assert!(marker.0 - half > area.x && marker.1 - half > area.y);
            // clear of the board
            let (bx0, by0, _, _) = area.board_rect;
            assert!(marker.0 + half < bx0 && marker.1 + half < by0);
            // clear of every slot
            for &(sx, sy, sw, sh) in &area.slots {
                assert!(
                    (sx - marker.0).abs() >= (sw + 2.0 * half) / 2.0 - 1e-9
                        || (sy - marker.1).abs() >= (sh + 2.0 * half) / 2.0 - 1e-9,
                    "the X at {marker:?} reaches the slot at ({sx}, {sy})"
                );
            }
        }
        assert_eq!(marker_trouble(&layout), "");
        // no marker without the slots datum, none when it is switched off
        let (holes, _) = packed(&projects, (700, 600), holes_params());
        assert!(holes.areas.iter().all(|a| a.marker.is_none()));
        let off = LayoutParams {
            marker: false,
            ..slots_params()
        };
        let (plain, _) = packed(&projects, (700, 600), off);
        assert!(plain.areas.iter().all(|a| a.marker.is_none()));
    }

    #[test]
    fn holes_sit_near_the_corners_and_are_shared() {
        let params = holes_params();
        let projects = sample();
        let (layout, _) = packed(&projects, (700, 600), params.clone());
        let off = params.hole_offset();
        for area in &layout.areas {
            assert!(area.slots.is_empty());
            assert_eq!(area.pins, area.holes);
            assert!(area.holes.len() <= 4);
            for &(hx, hy) in &area.holes {
                let near_x = (hx - (area.x + off)).abs() < 1e-9
                    || (hx - (area.x + area.w - off)).abs() < 1e-9;
                let near_y = (hy - (area.y + off)).abs() < 1e-9
                    || (hy - (area.y + area.h - off)).abs() < 1e-9;
                assert!(
                    near_x && near_y,
                    "hole ({hx}, {hy}) is not at a corner of {:?}",
                    area.rect()
                );
            }
        }
        // hole_grid makes the hole spacing a whole number of steps
        for area in &layout.areas {
            for span in [area.w - 2.0 * off, area.h - 2.0 * off] {
                let steps = span / params.hole_grid;
                assert!((steps - steps.round()).abs() * params.hole_grid < 1e-9);
            }
        }
    }

    #[test]
    fn no_datum_features_without_a_datum() {
        let projects = sample();
        let (layout, _) = packed(&projects, (700, 600), none_params());
        for area in &layout.areas {
            assert!(area.holes.is_empty() && area.slots.is_empty() && area.pins.is_empty());
            assert!(area.marker.is_none());
            // exactly board + gap
            assert!((area.w - (area.board_rect.2 - area.board_rect.0) - 30.0).abs() < 1e-9);
            assert!((area.h - (area.board_rect.3 - area.board_rect.1) - 30.0).abs() < 1e-9);
        }
    }

    // -- dividers ------------------------------------------------------------

    fn vertical_covering(layout: &Layout, line: f64, lo: f64, hi: f64) -> bool {
        layout.dividers.iter().any(|&(x0, y0, x1, y1)| {
            (x0 - x1).abs() < 1e-9 && (x0 - line).abs() < 1e-6 && y0 <= lo + 1e-6 && y1 >= hi - 1e-6
        })
    }

    fn horizontal_covering(layout: &Layout, line: f64, lo: f64, hi: f64) -> bool {
        layout.dividers.iter().any(|&(x0, y0, x1, y1)| {
            (y0 - y1).abs() < 1e-9 && (y0 - line).abs() < 1e-6 && x0 <= lo + 1e-6 && x1 >= hi - 1e-6
        })
    }

    #[test]
    fn dividers_run_inside_a_cell_on_a_line_that_cell_owns() {
        for outer in [false, true] {
            let params = LayoutParams {
                outer_border: outer,
                ..slots_params()
            };
            let projects = sample();
            let (layout, _) = packed(&projects, (700, 600), params.clone());
            let half = params.dot_line_gap / 2.0;
            let (bx0, by0, bx1, by1) = layout.block;
            for &(x0, y0, x1, y1) in &layout.dividers {
                let vertical = (x0 - x1).abs() < 1e-12;
                let line = if vertical { x0 } else { y0 };
                // every point of the segment lies in some cell
                for t in [0.0, 0.17, 0.5, 0.83, 1.0] {
                    let px = x0 + (x1 - x0) * t;
                    let py = y0 + (y1 - y0) * t;
                    assert!(
                        layout.areas.iter().any(|a| {
                            px >= a.x - 1e-6
                                && px <= a.x + a.w + 1e-6
                                && py >= a.y - 1e-6
                                && py <= a.y + a.h + 1e-6
                        }),
                        "divider point ({px}, {py}) is outside every cell"
                    );
                }
                // and the line belongs to a cell edge, half the gap away
                let owners: Vec<f64> = layout
                    .areas
                    .iter()
                    .flat_map(|a| {
                        let (ax0, ay0, ax1, ay1) = a.rect();
                        if vertical {
                            vec![ax0, ax1]
                        } else {
                            vec![ay0, ay1]
                        }
                    })
                    .filter(|&edge| {
                        (edge + half - line).abs() < 1e-6 || (edge - half - line).abs() < 1e-6
                    })
                    .collect();
                assert!(!owners.is_empty(), "no cell owns the line at {line}");
                if !outer {
                    let (b0, b1) = if vertical { (bx0, bx1) } else { (by0, by1) };
                    assert!(
                        owners
                            .iter()
                            .any(|&e| (e - b0).abs() > 1e-6 && (e - b1).abs() > 1e-6),
                        "the line at {line} only sits on the block boundary"
                    );
                }
            }
        }
    }

    #[test]
    fn a_lone_cell_is_a_closed_rectangle_of_four_lines() {
        let params = LayoutParams {
            outer_border: true,
            ..slots_params()
        };
        let projects = projects_of(&[("solo", 40.0, 30.0)]);
        let config = config_of((380, 280), params.clone());
        let sides = vec![SideId {
            project: 0,
            side: 0,
        }];
        let layout = pack(&projects, &sides, &config);
        let area = &layout.areas[0];
        let (x0, y0, x1, y1) = area.rect();
        let half = params.dot_line_gap / 2.0;
        assert_eq!(layout.dividers.len(), 4, "{:?}", layout.dividers);
        // the four lines are exactly the cell rectangle inset by half the gap
        assert!(vertical_covering(&layout, x0 + half, y0 + half, y1 - half));
        assert!(vertical_covering(&layout, x1 - half, y0 + half, y1 - half));
        assert!(horizontal_covering(
            &layout,
            y0 + half,
            x0 + half,
            x1 - half
        ));
        assert!(horizontal_covering(
            &layout,
            y1 - half,
            x0 + half,
            x1 - half
        ));
        // the corners are closed: the extents match exactly
        for &(a, b, c, d) in &layout.dividers {
            let len = (c - a).abs() + (d - b).abs();
            let expect = if (a - c).abs() < 1e-12 {
                y1 - y0 - 2.0 * half
            } else {
                x1 - x0 - 2.0 * half
            };
            assert!((len - expect).abs() < 1e-9);
        }
        // A lone cell is all block boundary, so without outer_border it keeps
        // no line at all - there is nothing to cut apart.
        let plain = pack(&projects, &sides, &config_of((380, 280), slots_params()));
        assert!(plain.dividers.is_empty(), "{:?}", plain.dividers);
        assert!(plain.dots.is_empty());
    }

    #[test]
    fn touching_cells_show_two_lines_one_dot_line_gap_apart() {
        let params = slots_params();
        let projects = sample();
        let (layout, _) = packed(&projects, (700, 600), params.clone());
        let half = params.dot_line_gap / 2.0;
        let mut pairs = 0;
        for a in &layout.areas {
            for b in &layout.areas {
                // b immediately right of a, with a shared span
                if (a.x + a.w - b.x).abs() < 1e-9 {
                    let lo = a.y.max(b.y);
                    let hi = (a.y + a.h).min(b.y + b.h);
                    if hi - lo <= params.dot_line_gap {
                        continue;
                    }
                    pairs += 1;
                    assert!(
                        vertical_covering(&layout, a.x + a.w - half, lo + half, hi - half),
                        "no line inside the right edge of {:?}",
                        a.rect()
                    );
                    assert!(
                        vertical_covering(&layout, b.x + half, lo + half, hi - half),
                        "no line inside the left edge of {:?}",
                        b.rect()
                    );
                    // and they are exactly dot_line_gap apart
                    assert!(((b.x + half) - (a.x + a.w - half) - params.dot_line_gap).abs() < 1e-9);
                }
            }
        }
        assert!(pairs > 0, "the sample never puts two cells side by side");
    }

    #[test]
    fn a_line_gap_of_zero_puts_the_lines_on_the_edges() {
        let params = LayoutParams {
            dot_line_gap: 0.0,
            outer_border: true,
            ..slots_params()
        };
        let projects = projects_of(&[("solo", 40.0, 30.0)]);
        let config = config_of((380, 280), params);
        let sides = vec![SideId {
            project: 0,
            side: 0,
        }];
        let layout = pack(&projects, &sides, &config);
        let area = &layout.areas[0];
        let (x0, y0, x1, y1) = area.rect();
        assert!(vertical_covering(&layout, x0, y0, y1));
        assert!(vertical_covering(&layout, x1, y0, y1));
        assert!(horizontal_covering(&layout, y0, x0, x1));
        assert!(horizontal_covering(&layout, y1, x0, x1));
    }

    // -- dots ----------------------------------------------------------------

    #[test]
    fn dots_are_centred_on_their_divider_and_spaced_by_the_pitch() {
        let params = none_params(); // no datum features, so nothing is dropped
        let projects = sample();
        let (layout, _) = packed(&projects, (700, 600), params.clone());
        assert!(!layout.dividers.is_empty());
        let pitch = params.dot_pitch;
        for &(x0, y0, x1, y1) in &layout.dividers {
            let length = (x1 - x0).hypot(y1 - y0);
            let n = (length / pitch).floor() as i64;
            let start = (length - n as f64 * pitch) / 2.0;
            // the row is centred: the same margin at both ends
            assert!((start - (length - (start + n as f64 * pitch))).abs() < 1e-9);
            let (ux, uy) = ((x1 - x0) / length, (y1 - y0) / length);
            let mut on_line = 0;
            for i in 0..=n {
                let d = start + i as f64 * pitch;
                let want = (x0 + ux * d, y0 + uy * d);
                if layout
                    .dots
                    .iter()
                    .any(|p| (p.0 - want.0).abs() < 1e-9 && (p.1 - want.1).abs() < 1e-9)
                {
                    on_line += 1;
                }
            }
            // every position is there unless the dedupe already had that point
            assert!(on_line >= 1, "no dot on the divider at ({x0}, {y0})");
        }
        // every dot sits on some divider
        for &(px, py) in &layout.dots {
            assert!(
                layout
                    .dividers
                    .iter()
                    .any(|&(x0, y0, x1, y1)| { segment_distance(px, py, x0, y0, x1, y1) < 1e-9 }),
                "the dot at ({px}, {py}) is on no divider"
            );
        }
    }

    #[test]
    fn a_divider_shorter_than_the_pitch_gets_one_dot_in_the_middle() {
        let params = none_params();
        let cover = Cover::new(&[], &params);
        let (dots, dropped) = dots_of(&[(0.0, 0.0, 2.0, 0.0)], &params, &cover);
        assert_eq!(dropped, 0);
        assert_eq!(dots.len(), 1);
        assert!((dots[0].0 - 1.0).abs() < 1e-12 && dots[0].1.abs() < 1e-12);
        // and one exactly as long as the pitch gets two, at its ends
        let (dots, _) = dots_of(&[(0.0, 0.0, params.dot_pitch, 0.0)], &params, &cover);
        assert_eq!(dots.len(), 2);
        assert!(dots[0].0.abs() < 1e-12 && (dots[1].0 - params.dot_pitch).abs() < 1e-12);
    }

    #[test]
    fn dots_closer_than_their_diameter_are_deduplicated() {
        for params in [slots_params(), holes_params(), none_params()] {
            let projects = sample();
            let (layout, _) = packed(&projects, (700, 600), params.clone());
            let mut dedupe = Dedupe::new(params.dot_dia);
            for &(px, py) in &layout.dots {
                assert!(
                    dedupe.add(px, py),
                    "{}: two dots closer than {} mm around ({px}, {py})",
                    params.datum,
                    params.dot_dia
                );
            }
        }
    }

    // -- the clearance rule --------------------------------------------------

    #[test]
    fn a_dot_too_close_to_a_slot_is_dropped() {
        let params = slots_params(); // dot_dia 0.5, dot_clearance 0.5
        let projects = sample();
        let (layout, _) = packed(&projects, (700, 600), params.clone());
        let cover = Cover::new(&layout.areas, &params);
        let area = layout
            .areas
            .iter()
            .find(|a| !a.slots.is_empty())
            .expect("a cell with slots");
        let &(sx, sy, sw, sh) = &area.slots[0];
        // Straight down from the slot's centre: the surface is sh/2 away, so a
        // dot whose *edge* is `metal` from it has its centre that much further.
        for metal in [-0.2, 0.0, 0.1, 0.4, 0.49] {
            let y = sy - sh / 2.0 - params.dot_dia / 2.0 - metal;
            assert!(
                cover.near(sx, y),
                "a dot leaving {metal} mm of metal survived"
            );
        }
        for metal in [0.51, 0.6, 1.0, 5.0] {
            let y = sy - sh / 2.0 - params.dot_dia / 2.0 - metal;
            assert!(
                !cover.near(sx, y),
                "a dot leaving {metal} mm of metal was dropped"
            );
        }
        // the same along the slot's long axis (its rounded cap)
        for (metal, gone) in [(0.4, true), (0.6, false)] {
            let x = sx - sw / 2.0 - params.dot_dia / 2.0 - metal;
            assert_eq!(cover.near(x, sy), gone, "cap clearance at {metal} mm");
        }
        // and every dot that survived really does keep the clearance
        for &(px, py) in &layout.dots {
            assert!(
                cover.distance(px, py) >= cover.margin,
                "the dot at ({px}, {py}) leaves only {} mm",
                cover.distance(px, py)
            );
        }
    }

    #[test]
    fn a_dot_too_close_to_a_hole_or_the_marker_is_dropped() {
        let params = holes_params();
        let projects = sample();
        let (layout, _) = packed(&projects, (700, 600), params.clone());
        let cover = Cover::new(&layout.areas, &params);
        let area = layout
            .areas
            .iter()
            .find(|a| !a.holes.is_empty())
            .expect("a cell with holes");
        let (hx, hy) = area.holes[0];
        let r = params.hole_dia / 2.0 + params.dot_dia / 2.0;
        assert!(cover.near(hx + r + 0.4, hy));
        assert!(!cover.near(hx + r + 0.6, hy));

        // the marker: two dot_dia wide strokes, measured from a stroke's end
        let params = slots_params();
        let (layout, _) = packed(&projects, (700, 600), params.clone());
        let cover = Cover::new(&layout.areas, &params);
        let marker = layout.areas[0].marker.unwrap();
        let (x0, y0, _, _) = marker_strokes(marker, params.marker_size)[0];
        let reach = params.dot_dia; // the stroke's half width plus the dot's
        let dir = std::f64::consts::FRAC_1_SQRT_2;
        assert!(cover.near(x0 - dir * (reach + 0.4), y0 - dir * (reach + 0.4)));
        assert!(!cover.near(x0 - dir * (reach + 0.6), y0 - dir * (reach + 0.6)));
    }

    #[test]
    fn the_clearance_only_ever_removes_dots() {
        let projects = sample();
        let mut last = usize::MAX;
        for clearance in [0.0, 0.25, 0.5, 1.0, 2.0] {
            let params = LayoutParams {
                dot_clearance: clearance,
                ..slots_params()
            };
            let (layout, _) = packed(&projects, (700, 600), params);
            assert!(
                layout.dots.len() <= last,
                "clearance {clearance} kept more dots than the one before"
            );
            assert!(dropped_dots(&layout) > 0);
            last = layout.dots.len();
        }
        // a clearance of 0 still drops the dots that overlap a feature
        let params = LayoutParams {
            dot_clearance: 0.0,
            ..slots_params()
        };
        let (zero, _) = packed(&projects, (700, 600), params);
        assert!(dropped_dots(&zero) > 0);
        // no features, nothing to drop
        let (plain, _) = packed(&projects, (700, 600), none_params());
        assert_eq!(dropped_dots(&plain), 0);
    }

    // -- packing -------------------------------------------------------------

    #[test]
    fn a_sheet_that_is_too_small_overflows_to_its_right() {
        let projects = sample();
        let (layout, config) = packed(&projects, (270, 270), slots_params());
        assert!(layout.overflow > 0, "13 cells should not fit on 270x270");
        assert!(!layout.fits);
        let (sheet_w, _) = config.sheet_size();
        for area in &layout.areas {
            if area.overflow {
                assert!(area.x >= sheet_w - 1e-9, "overflow cell at x={}", area.x);
            }
        }
        assert_eq!(
            layout.overflow,
            layout.areas.iter().filter(|a| a.overflow).count()
        );
        // a sheet big enough takes them all
        let (roomy, _) = packed(&projects, (700, 600), slots_params());
        assert_eq!(roomy.overflow, 0);
        assert!(roomy.fits);
        assert!(roomy.areas.iter().all(|a| !a.overflow));
    }

    #[test]
    fn the_block_is_centred_on_the_sheet_and_clamped_at_zero() {
        let projects = projects_of(&[("solo", 40.0, 30.0)]);
        let sides = vec![SideId {
            project: 0,
            side: 0,
        }];
        // Without a raster the centring is exact: the same margin left and
        // right, top and bottom.
        let config = config_of((380, 280), none_params());
        let layout = pack(&projects, &sides, &config);
        let (sheet_w, sheet_h) = config.sheet_size();
        let (bx0, by0, bx1, by1) = layout.block;
        assert!(
            (bx0 - (sheet_w - bx1)).abs() < 1e-9,
            "{bx0} vs {}",
            sheet_w - bx1
        );
        assert!(
            (by0 - (sheet_h - by1)).abs() < 1e-9,
            "{by0} vs {}",
            sheet_h - by1
        );
        assert!(layout.fits);

        // With one, the block is shifted by a whole number of pitches, so it is
        // centred only as well as the raster allows - never past the middle,
        // never more than a pitch short of it, and never off the sheet.
        let params = slots_params();
        let pitch = params.slot_pitch;
        let config = config_of((380, 280), params);
        let layout = pack(&projects, &sides, &config);
        let (bx0, by0, bx1, by1) = layout.block;
        assert!(bx0 >= 0.0 && by0 >= 0.0 && bx1 <= sheet_w && by1 <= sheet_h);
        assert!(layout.fits && pins_on_grid(&layout));
        // the right margin is the left one give or take two pitches
        assert!(sheet_w - bx1 >= 0.0 && sheet_w - bx1 < bx0 + 2.0 * pitch);
        assert!(sheet_h - by1 >= 0.0 && sheet_h - by1 < by0 + 2.0 * pitch);

        // a block bigger than the sheet is not pushed negative
        let (big, _) = packed(&sample(), (270, 270), slots_params());
        assert!(big.block.0 >= 0.0 && big.block.1 >= 0.0);
    }

    #[test]
    fn fits_follows_the_block_and_the_overflow() {
        for size in STENCIL_SIZES {
            let projects = sample();
            let (layout, config) = packed(&projects, size, slots_params());
            let (sheet_w, sheet_h) = config.sheet_size();
            let want = layout.overflow == 0
                && layout.block.0 >= -1e-6
                && layout.block.1 >= -1e-6
                && layout.block.2 <= sheet_w + 1e-6
                && layout.block.3 <= sheet_h + 1e-6;
            assert_eq!(layout.fits, want, "{size:?}");
        }
    }

    #[test]
    fn an_empty_layout_is_empty_but_valid() {
        let mut projects = sample();
        for project in &mut projects {
            for side in &mut project.sides {
                side.enabled = false;
            }
        }
        let (layout, _) = packed(&projects, (380, 280), slots_params());
        assert!(layout.areas.is_empty());
        assert!(layout.dividers.is_empty() && layout.dots.is_empty());
        assert_eq!(layout.block, (0.0, 0.0, 0.0, 0.0));
        assert!(layout.fits);
        assert_eq!(layout.overflow, 0);
    }

    #[test]
    fn packing_is_deterministic() {
        for params in [slots_params(), holes_params(), none_params()] {
            let projects = sample();
            let (a, _) = packed(&projects, (460, 460), params.clone());
            let (b, _) = packed(&projects, (460, 460), params.clone());
            assert_eq!(a.heuristic, b.heuristic);
            assert_eq!(a.fits, b.fits);
            assert_eq!(a.block, b.block);
            assert_eq!(a.dots, b.dots);
            assert_eq!(a.dividers, b.dividers);
            assert_eq!(a.areas.len(), b.areas.len());
            for (x, y) in a.areas.iter().zip(&b.areas) {
                assert_eq!(x.side, y.side);
                assert_eq!((x.x, x.y, x.w, x.h), (y.x, y.y, y.w, y.h));
                assert_eq!(x.slots, y.slots);
                assert_eq!(x.pins, y.pins);
                assert_eq!(x.holes, y.holes);
                assert_eq!(x.marker, y.marker);
            }
        }
        // and the heuristic that won is one of the three
        let projects = sample();
        let (layout, _) = packed(&projects, (460, 460), slots_params());
        assert!(HEURISTICS.contains(&layout.heuristic.as_str()));
    }

    // -- ordering ------------------------------------------------------------

    #[test]
    fn height_sorts_tallest_first_then_widest_then_by_name_then_top_first() {
        let projects = projects_of(&[
            ("bravo", 10.0, 20.0),
            ("alpha10", 30.0, 20.0),
            ("alpha2", 30.0, 20.0),
            ("charlie", 5.0, 40.0),
        ]);
        let params = LayoutParams {
            sort: SORT_HEIGHT.to_string(),
            ..slots_params()
        };
        let order = ordered_sides(&projects, &all_sides(&projects), &params);
        let labels: Vec<String> = order.iter().map(|id| id.get(&projects).label()).collect();
        assert_eq!(
            labels,
            vec![
                "charlie top",
                "charlie bottom",
                "alpha2 top",
                "alpha2 bottom",
                "alpha10 top",
                "alpha10 bottom",
                "bravo top",
                "bravo bottom",
            ]
        );
    }

    #[test]
    fn name_sorts_naturally_with_top_before_bottom() {
        let projects = projects_of(&[("R10", 10.0, 20.0), ("r2", 30.0, 40.0), ("C1", 5.0, 5.0)]);
        let params = LayoutParams {
            sort: SORT_NAME.to_string(),
            ..slots_params()
        };
        let order = ordered_sides(&projects, &all_sides(&projects), &params);
        let labels: Vec<String> = order.iter().map(|id| id.get(&projects).label()).collect();
        assert_eq!(
            labels,
            vec![
                "C1 top",
                "C1 bottom",
                "r2 top",
                "r2 bottom",
                "R10 top",
                "R10 bottom",
            ]
        );
        // the packing order is the area order
        let config = config_of((700, 600), params);
        let layout = pack(&projects, &all_sides(&projects), &config);
        assert_eq!(
            layout.areas.iter().map(|a| a.row).collect::<Vec<_>>(),
            (0..layout.areas.len()).collect::<Vec<_>>()
        );
        for (area, id) in layout.areas.iter().zip(&order) {
            assert_eq!(area.side, *id);
        }
    }

    #[test]
    fn only_enabled_sides_get_a_cell() {
        let mut projects = sample();
        projects[0].sides[1].enabled = false;
        projects[2].sides[0].enabled = false;
        let (layout, _) = packed(&projects, (700, 600), slots_params());
        assert_eq!(layout.areas.len(), 12);
        for area in &layout.areas {
            assert!(area.side.get(&projects).enabled);
        }
    }

    // -- transforms ----------------------------------------------------------

    #[test]
    fn the_transform_puts_the_board_box_on_its_cell() {
        let projects = sample();
        let (layout, _) = packed(&projects, (700, 600), slots_params());
        for area in &layout.areas {
            let side = area.side.get(&projects);
            let (minx, miny, maxx, maxy) = side.board_bbox;
            let (a, b) = (
                area.transform.apply(minx, miny),
                area.transform.apply(maxx, maxy),
            );
            let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
            assert!((x0 - area.board_rect.0).abs() < 1e-9);
            assert!((x1 - area.board_rect.2).abs() < 1e-9);
            assert!((a.1 - area.board_rect.1).abs() < 1e-9);
            assert!((b.1 - area.board_rect.3).abs() < 1e-9);
            assert_eq!(area.transform.mirror, side.mirror);
            assert_eq!(area.datum_corner, (area.x, area.y));
        }
    }

    // -- the marker geometry -------------------------------------------------

    #[test]
    fn marker_strokes_cross_at_the_centre() {
        let [(ax0, ay0, ax1, ay1), (bx0, by0, bx1, by1)] = marker_strokes((10.0, 20.0), 4.0);
        let half = 4.0 * std::f64::consts::SQRT_2 / 4.0;
        assert!((ax0 - (10.0 - half)).abs() < 1e-12 && (ay0 - (20.0 - half)).abs() < 1e-12);
        assert!((ax1 - (10.0 + half)).abs() < 1e-12 && (ay1 - (20.0 + half)).abs() < 1e-12);
        assert!((bx0 - (10.0 - half)).abs() < 1e-12 && (by0 - (20.0 + half)).abs() < 1e-12);
        assert!((bx1 - (10.0 + half)).abs() < 1e-12 && (by1 - (20.0 - half)).abs() < 1e-12);
        // both strokes are `size` long
        for (x0, y0, x1, y1) in [(ax0, ay0, ax1, ay1), (bx0, by0, bx1, by1)] {
            assert!(((x1 - x0).hypot(y1 - y0) - 4.0).abs() < 1e-12);
        }
        // and the stroke width pushes the cut a little further out
        let params = LayoutParams {
            marker_size: 4.0,
            dot_dia: 0.5,
            ..slots_params()
        };
        assert!((marker_half(&params) - (half + 0.25)).abs() < 1e-12);
        assert_eq!(marker_strokes((0.0, 0.0), -1.0)[0], (0.0, 0.0, 0.0, 0.0));
    }

    // -- the report ----------------------------------------------------------

    #[test]
    fn the_report_names_every_cell_and_the_dot_summary() {
        let projects = sample();
        let (layout, config) = packed(&projects, (700, 600), slots_params());
        let report = layout_report(&projects, &layout, &config);
        assert!(report.starts_with("Stencil: 700.0 x 600.0 mm (700x600 landscape)\n"));
        assert!(report.contains("   FITS"));
        assert!(report.contains("Datum:   slots at a 20 mm raster"));
        assert!(report.contains("Marker:  X 4 mm at the raster point 3.5 mm"));
        assert!(report.contains("Grid:    pin centres on the 20.0 mm slot_pitch raster: yes"));
        for area in &layout.areas {
            assert!(report.contains(&area.side.get(&projects).label()));
        }
        let tail = report.lines().last().unwrap();
        assert!(
            tail.starts_with(&format!("Divider dots: {} (", layout.dots.len())),
            "{tail}"
        );
        assert!(
            tail.contains(&format!(
                "; {} dropped within 0.5 mm of a slot, hole or marker)",
                dropped_dots(&layout)
            )),
            "{tail}"
        );
        // the datum blocks of the other two modes
        let (holes, config) = packed(&projects, (700, 600), holes_params());
        let text = layout_report(&projects, &holes, &config);
        assert!(text.contains("Datum:   holes: ⌀5.0 mm, inset 2.0 mm (centres 4.50 mm inside"));
        assert!(text.contains("   dowel holes (⌀5.0 mm), sheet / relative to board corner"));
        let (plain, config) = packed(&projects, (700, 600), none_params());
        let text = layout_report(&projects, &plain, &config);
        assert!(text.contains("\nDatum:   none\n"));
        assert!(text.contains("Grid:    off (datum none: no pins to align)"));
        assert!(text.contains("   datum features: none"));
    }

    #[test]
    fn the_report_warns_about_hand_set_numbers() {
        let projects = projects_of(&[("solo", 40.0, 30.0)]);
        // a slot longer than the raster: two of them on one edge overlap
        let params = LayoutParams {
            slot_length: 25.0,
            ..slots_params()
        };
        let (layout, config) = packed(&projects, (700, 600), params);
        let text = layout_report(&projects, &layout, &config);
        assert!(
            text.contains("WARNING: slot_length 25 mm is longer than the 20 mm raster"),
            "{text}"
        );
        // an X that crosses the cell edge
        let params = LayoutParams {
            marker_size: 20.0,
            ..slots_params()
        };
        let (layout, config) = packed(&projects, (700, 600), params);
        let text = layout_report(&projects, &layout, &config);
        assert!(text.contains("WARNING: the 20 mm X reaches"), "{text}");
    }

    // -- the formatting helpers ---------------------------------------------

    #[test]
    fn g_and_thousands_match_python() {
        assert_eq!(fmt_g(20.0), "20");
        assert_eq!(fmt_g(4.5), "4.5");
        assert_eq!(fmt_g(0.5), "0.5");
        assert_eq!(fmt_g(0.0), "0");
        assert_eq!(fmt_g(3.0), "3");
        assert_eq!(fmt_g(1234567.0), "1.23457e+06");
        assert_eq!(fmt_g(0.000012345), "1.2345e-05");
        assert_eq!(fmt_g(0.0001), "0.0001");
        assert_eq!(fmt_thousands_1(0.0), "0.0");
        assert_eq!(fmt_thousands_1(28674.65), "28,674.7");
        assert_eq!(fmt_thousands_1(-1234.5), "-1,234.5");
        assert_eq!(fmt_thousands_1(1234567.0), "1,234,567.0");
        assert_eq!(fmt_point((1.005, -0.001)), "(1.00, -0.00)");
    }

    #[test]
    fn the_raster_helpers_round_the_way_the_original_does() {
        assert_eq!(ceil_pitch(0.0, 20.0), 20.0);
        assert_eq!(ceil_pitch(20.0, 20.0), 20.0);
        assert_eq!(ceil_pitch(20.000000001, 20.0), 20.0);
        assert_eq!(ceil_pitch(21.0, 20.0), 40.0);
        assert_eq!(snap_up(0.0, 3.5, 20.0), 16.5);
        assert_eq!(snap_up(16.5, 3.5, 20.0), 16.5);
        assert_eq!(snap_up(16.6, 3.5, 20.0), 36.5);
        assert_eq!(snap_up(5.0, 0.0, 0.0), 5.0);
        assert_eq!(snap_down(45.0, 20.0), 40.0);
        assert_eq!(snap_down(40.0, 20.0), 40.0);
        assert_eq!(snap_down(-5.0, 20.0), 0.0);
        // grown until size - 2*ho is a whole number of the grid
        assert_eq!(grid_size(10.0, 1.0, 8.0), 10.0);
        assert_eq!(grid_size(11.0, 1.0, 8.0), 18.0);
        assert_eq!(cell_size(11.0, 9.0, 1.0, 8.0), (18.0, 10.0));
        assert_eq!(cell_size(11.0, 9.0, 1.0, 0.0), (11.0, 9.0));
    }
}
