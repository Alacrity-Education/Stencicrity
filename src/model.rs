//! Shared data model: projects, sides, pads, configuration and layout.
//!
//! Coordinate conventions
//! ----------------------
//! * "board coordinates": the coordinates found in a project's gerber files
//!   (mm). Top and bottom layers of one project share them.
//! * "sheet coordinates": the output stencil sheet, origin (0, 0) at the
//!   bottom-left corner of the *stencil* (its ordered size), X right, Y up, mm.
//! * A bottom side is placed mirrored about the Y axis (x -> -x) so that the
//!   stencil matches the board when it is flipped over.
//!
//! Layout model
//! ------------
//! Every enabled side gets a *cell*: the board bounding box padded by at
//! least `gap/2` on all four sides. Cells are placed on the sheet with a
//! MaxRects bin packer. Every cell owns one sparse dotted line along each of
//! its edges, `dot_line_gap/2` inside the edge, so two touching cells show two
//! lines `dot_line_gap` apart and the scissors cut between them; the lines on
//! the outer boundary of the whole block are left out unless `outer_border`.
//!
//! Alignment features (`datum`): with `slots` the cells are sized to
//! multiples of `slot_pitch` (the raster of a modular pin jig) and every cell
//! carries an obround slot at every raster position along its bottom and left
//! edge, `slot_offset` inside the edge. A slot lies completely inside its own
//! cell (it may clip the cell's dotted line, never a neighbour) and keeps
//! `slot_web` of foil to the board. Jig pins pass through the slots; the pin
//! centre (`pin_offset` inside the edge) lies on the same raster, so two slots
//! on one edge plus one on the other give an exact three-contact location when
//! the piece is pushed toward the bottom-left corner; cells are sized so that
//! this is always possible. An X marker is cut at the raster point
//! `pin_offset` inside the datum corner. Cell corners sit on the raster with
//! phase `-pin_offset`, so touching cells share the raster. With `holes` every
//! cell gets four round holes near its corners (legacy); `hole_grid` snaps
//! those. The block of all cells is centred on the stencil (keeping the raster
//! alignment).

use std::collections::BTreeMap;

use geo::MultiPolygon;

use crate::gerber::{Flash, GerberFile, GraphicObject};

/// Planar geometry in millimetres (board or sheet coordinates).
pub type Geom = MultiPolygon<f64>;

/// (minx, miny, maxx, maxy)
pub type Bounds = (f64, f64, f64, f64);

pub const SIDE_TOP: &str = "top";
pub const SIDE_BOTTOM: &str = "bottom";

pub const STATE_UNDEFINED: &str = "undefined";
pub const STATE_OPEN: &str = "open";
pub const STATE_IGNORE: &str = "ignore";
pub const STATES: [&str; 3] = [STATE_UNDEFINED, STATE_OPEN, STATE_IGNORE];

/// Orderable stencil sheet sizes (mm), long side first, smallest first.
pub const STENCIL_SIZES: [(u32, u32); 8] = [
    (270, 270),
    (380, 280),
    (420, 320),
    (450, 350),
    (460, 460),
    (520, 420),
    (600, 600),
    (700, 600),
];
pub const DEFAULT_STENCIL_SIZE: (u32, u32) = (380, 280);
pub const ORIENTATION_LANDSCAPE: &str = "landscape"; // long side horizontal
pub const ORIENTATION_PORTRAIT: &str = "portrait"; // long side vertical
pub const ORIENTATIONS: [&str; 2] = [ORIENTATION_LANDSCAPE, ORIENTATION_PORTRAIT];
pub const SORT_HEIGHT: &str = "height";
pub const SORT_NAME: &str = "name";
pub const SORT_ORDERS: [&str; 2] = [SORT_HEIGHT, SORT_NAME];

/// Reference prefixes whose pads without paste default to "ignore".
pub const DEFAULT_IGNORE_PREFIXES: [&str; 2] = ["NT", "TP"];

/// Alignment datum cut into every cell for the pin jig.
pub const DATUM_SLOTS: &str = "slots"; // obround slots on the slot_pitch raster along the bottom and left edges
pub const DATUM_HOLES: &str = "holes"; // four round holes near the cell corners (legacy)
pub const DATUM_NONE: &str = "none";
pub const DATUM_MODES: [&str; 3] = [DATUM_SLOTS, DATUM_HOLES, DATUM_NONE];

pub fn size_label(size: (u32, u32)) -> String {
    format!("{}x{}", size.0, size.1)
}

/// `"380x280"` -> `(380, 280)` (long side first). Errors on anything else.
pub fn parse_size(text: &str) -> Result<(u32, u32), String> {
    let t = text.trim().to_lowercase().replace('×', "x");
    let parts: Vec<&str> = t.split('x').collect();
    if parts.len() != 2 {
        return Err(format!("bad stencil size {text:?}"));
    }
    let mut dims = Vec::new();
    for p in parts {
        let v: f64 = p
            .trim()
            .parse()
            .map_err(|_| format!("bad stencil size {text:?}"))?;
        dims.push(v.round() as u32);
    }
    Ok((dims[0].max(dims[1]), dims[0].min(dims[1])))
}

/// Key of one side in the config file: `<project>/<side>`.
pub fn side_key(project_name: &str, side_name: &str) -> String {
    format!("{project_name}/{side_name}")
}

/// Board -> sheet mapping: x' = (-x if mirror else x) + dx ; y' = y + dy.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Transform {
    pub mirror: bool,
    pub dx: f64,
    pub dy: f64,
}

impl Transform {
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        ((if self.mirror { -x } else { x }) + self.dx, y + self.dy)
    }

    /// Apply to a geometry (mirror about x = 0, then translate).
    pub fn apply_geom(&self, g: &Geom) -> Geom {
        use geo::MapCoords;
        g.map_coords(|c| {
            let (x, y) = self.apply(c.x, c.y);
            geo::Coord { x, y }
        })
    }
}

/// A pad flash found on a copper layer.
#[derive(Clone, Debug)]
pub struct Pad {
    /// `<project>/<side>/<ref>.<pin>@<x>,<y>` (board coords, 3 decimals)
    pub key: String,
    pub project: String,
    pub side: String,
    pub ref_: String,
    pub pin: String,
    /// flash position, board coordinates (unmirrored)
    pub x: f64,
    pub y: f64,
    /// first token of AperFunction ("SMDPad", ...), may be empty
    pub function: String,
    /// human readable aperture description, e.g. "R 0.28x0.52"
    pub shape: String,
    /// the copper flash object
    pub flash: Flash,
    /// pad copper shape, board coordinates
    pub geom: Geom,
    /// the paste layer already has an opening on this pad
    pub has_paste: bool,
    /// candidates: undefined/open/ignore; pasted pads: open (default) or ignore
    pub state: String,
    /// indices into the side's paste objects covering this pad
    pub paste_indices: Vec<usize>,
}

impl Pad {
    /// A pad without a paste opening: the user decides whether to open it.
    pub fn is_candidate(&self) -> bool {
        !self.has_paste
    }
    /// A pad WITH paste whose opening the user removed from the stencil.
    pub fn is_closed(&self) -> bool {
        self.has_paste && self.state == STATE_IGNORE
    }
    pub fn label(&self) -> String {
        format!("{}.{}", self.ref_, self.pin)
    }
    pub fn side_key(&self) -> String {
        side_key(&self.project, &self.side)
    }
}

/// One side (top or bottom) of one project = one stencil cell when enabled.
#[derive(Debug)]
pub struct Side {
    pub project_name: String,
    /// board bounding box, board coords (copied from the project)
    pub board_bbox: Bounds,
    pub name: String,
    pub copper: Option<GerberFile>,
    pub paste: Option<GerberFile>,
    /// place mirrored on the sheet (set for bottom sides)
    pub mirror: bool,
    /// user switch from the config file / TUI sides page
    pub enabled: bool,
    /// every pad flash on copper, incl. those with paste
    pub pads: Vec<Pad>,
}

impl Side {
    pub fn key(&self) -> String {
        side_key(&self.project_name, &self.name)
    }
    pub fn label(&self) -> String {
        format!("{} {}", self.project_name, self.name)
    }
    pub fn board_width(&self) -> f64 {
        self.board_bbox.2 - self.board_bbox.0
    }
    pub fn board_height(&self) -> f64 {
        self.board_bbox.3 - self.board_bbox.1
    }
    pub fn paste_objects(&self) -> &[GraphicObject] {
        self.paste
            .as_ref()
            .map(|g| g.objects.as_slice())
            .unwrap_or(&[])
    }
    pub fn copper_objects(&self) -> &[GraphicObject] {
        self.copper
            .as_ref()
            .map(|g| g.objects.as_slice())
            .unwrap_or(&[])
    }
    pub fn candidates(&self) -> impl Iterator<Item = &Pad> {
        self.pads.iter().filter(|p| p.is_candidate())
    }
    pub fn pasted_pads(&self) -> impl Iterator<Item = &Pad> {
        self.pads.iter().filter(|p| p.has_paste)
    }
    pub fn open_pads(&self) -> impl Iterator<Item = &Pad> {
        self.pads
            .iter()
            .filter(|p| p.is_candidate() && p.state == STATE_OPEN)
    }
    pub fn closed_pads(&self) -> impl Iterator<Item = &Pad> {
        self.pads.iter().filter(|p| p.is_closed())
    }
    pub fn closed_paste_indices(&self) -> std::collections::BTreeSet<usize> {
        self.closed_pads()
            .flat_map(|p| p.paste_indices.iter().copied())
            .collect()
    }
    /// Paste objects that end up on the stencil (closed pads removed).
    pub fn active_paste_objects(&self) -> Vec<&GraphicObject> {
        let closed = self.closed_paste_indices();
        self.paste_objects()
            .iter()
            .enumerate()
            .filter(|(i, _)| !closed.contains(i))
            .map(|(_, o)| o)
            .collect()
    }
    pub fn closed_paste_objects(&self) -> Vec<&GraphicObject> {
        let closed = self.closed_paste_indices();
        self.paste_objects()
            .iter()
            .enumerate()
            .filter(|(i, _)| closed.contains(i))
            .map(|(_, o)| o)
            .collect()
    }
    pub fn has_openings(&self) -> bool {
        !self.active_paste_objects().is_empty() || self.open_pads().next().is_some()
    }
    /// Worth a stencil cell: has paste openings or pads to decide on.
    pub fn is_relevant(&self) -> bool {
        !self.paste_objects().is_empty() || self.candidates().next().is_some()
    }
}

#[derive(Debug)]
pub struct Project {
    /// from %TF.ProjectId, else the zip/dir name
    pub name: String,
    /// zip path or directory
    pub source: String,
    pub outline: Option<GerberFile>,
    /// board bounding box, board coords
    pub bbox: Bounds,
    /// top then bottom, when present
    pub sides: Vec<Side>,
}

impl Project {
    pub fn width(&self) -> f64 {
        self.bbox.2 - self.bbox.0
    }
    pub fn height(&self) -> f64 {
        self.bbox.3 - self.bbox.1
    }
    pub fn side(&self, name: &str) -> Option<&Side> {
        self.sides.iter().find(|s| s.name == name)
    }
}

/// Index of a side inside a `Vec<Project>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SideId {
    pub project: usize,
    pub side: usize,
}

impl SideId {
    pub fn get<'a>(&self, projects: &'a [Project]) -> &'a Side {
        &projects[self.project].sides[self.side]
    }
    pub fn get_mut<'a>(&self, projects: &'a mut [Project]) -> &'a mut Side {
        &mut projects[self.project].sides[self.side]
    }
}

/// Every side of every project, in order.
pub fn all_sides(projects: &[Project]) -> Vec<SideId> {
    let mut out = Vec::new();
    for (pi, p) in projects.iter().enumerate() {
        for si in 0..p.sides.len() {
            out.push(SideId {
                project: pi,
                side: si,
            });
        }
    }
    out
}

/// Everything on the TUI "Layout" page; persisted in the [layout] section.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutParams {
    /// spacing between neighbouring boards (mm); dotted border at gap/2
    pub gap: f64,
    /// DATUM_SLOTS | DATUM_HOLES | DATUM_NONE
    pub datum: String,
    // -- holes datum (legacy) --
    pub hole_dia: f64,
    /// cell edge to hole edge (mm); may be negative
    pub hole_inset: f64,
    // -- slots datum --
    /// slot size across the cell edge (mm)
    pub slot_width: f64,
    /// slot size along the cell edge (mm)
    pub slot_length: f64,
    /// cell edge to the slot's outer wall (mm)
    pub slot_offset: f64,
    /// raster of the modular jig (mm)
    pub slot_pitch: f64,
    /// minimum foil between a slot's inner wall and the board (mm)
    pub slot_web: f64,
    /// jig pin diameter for the slots datum (mm)
    pub pin_dia: f64,
    /// cut an X at the raster point pin_offset inside the datum corner
    pub marker: bool,
    /// X marker stroke length (mm); stroke width = dot_dia
    pub marker_size: f64,
    // -- dotted border --
    pub dot_dia: f64,
    pub dot_pitch: f64,
    /// every cell's dotted line runs dot_line_gap/2 inside its edge (0 = on the edge)
    pub dot_line_gap: f64,
    /// a divider dot is dropped when less than this much metal would remain between it and a slot, hole or marker (mm)
    pub dot_clearance: f64,
    // -- jig --
    /// holes datum: hole centres snap to this grid (mm); 0 = off
    pub hole_grid: f64,
    /// also dot the cell edges on the outer boundary of the block
    pub outer_border: bool,
    /// SORT_HEIGHT | SORT_NAME
    pub sort: String,
}

impl Default for LayoutParams {
    fn default() -> Self {
        Self {
            gap: 30.0,
            datum: DATUM_SLOTS.to_string(),
            hole_dia: 5.0,
            hole_inset: 2.0,
            slot_width: 4.5,
            slot_length: 8.0,
            slot_offset: 0.5,
            slot_pitch: 20.0,
            slot_web: 3.0,
            pin_dia: 3.0,
            marker: true,
            marker_size: 4.0,
            dot_dia: 0.5,
            dot_pitch: 3.0,
            dot_line_gap: 2.5,
            dot_clearance: 0.5,
            hole_grid: 8.0,
            outer_border: false,
            sort: SORT_HEIGHT.to_string(),
        }
    }
}

impl LayoutParams {
    /// Padding between a board and its cell edge.
    pub fn pad(&self) -> f64 {
        self.gap / 2.0
    }
    pub fn holes(&self) -> bool {
        self.datum == DATUM_HOLES
    }
    pub fn slots(&self) -> bool {
        self.datum == DATUM_SLOTS
    }
    /// Cell edge to hole centre (holes datum).
    pub fn hole_offset(&self) -> f64 {
        self.hole_inset + self.hole_dia / 2.0
    }
    /// Cell edge to the slot's inner wall, the wall the pin touches.
    pub fn slot_inner(&self) -> f64 {
        self.slot_offset + self.slot_width
    }
    /// Cell edge to the pin centre for the active datum.
    pub fn pin_offset(&self) -> f64 {
        if self.slots() {
            self.slot_inner() - self.pin_dia / 2.0
        } else {
            self.hole_offset()
        }
    }
    /// Smallest cell padding that hosts a slot and the web to the board.
    pub fn min_pad_for_slots(&self) -> f64 {
        self.slot_inner() + self.slot_web
    }
}

/// The whole `.stencicrity` file.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// long side first, one of STENCIL_SIZES
    pub size: (u32, u32),
    pub orientation: String,
    pub layout: LayoutParams,
    /// [rules] ignore_prefixes
    pub ignore_prefixes: Vec<String>,
    /// side_key -> enabled
    pub sides: BTreeMap<String, bool>,
    /// pad key -> state
    pub pads: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            size: DEFAULT_STENCIL_SIZE,
            orientation: ORIENTATION_LANDSCAPE.to_string(),
            layout: LayoutParams::default(),
            ignore_prefixes: DEFAULT_IGNORE_PREFIXES
                .iter()
                .map(|s| s.to_string())
                .collect(),
            sides: BTreeMap::new(),
            pads: BTreeMap::new(),
        }
    }
}

impl Config {
    pub fn size_label(&self) -> String {
        size_label(self.size)
    }
    /// (width, height) of the stencil as laid out, honouring the orientation.
    pub fn sheet_size(&self) -> (f64, f64) {
        let long = self.size.0.max(self.size.1) as f64;
        let short = self.size.0.min(self.size.1) as f64;
        if self.orientation == ORIENTATION_PORTRAIT {
            (short, long)
        } else {
            (long, short)
        }
    }
}

/// Placement of one side: its cell on the sheet.
#[derive(Clone, Debug)]
pub struct Area {
    pub side: SideId,
    /// cell bottom-left corner, sheet coords
    pub x: f64,
    pub y: f64,
    /// cell size
    pub w: f64,
    pub h: f64,
    /// where the board bbox lands (minx, miny, maxx, maxy)
    pub board_rect: Bounds,
    /// board -> sheet for this side's objects
    pub transform: Transform,
    /// informational placement order
    pub row: usize,
    /// did not fit on the sheet; parked to the right of it
    pub overflow: bool,
    /// round hole centres, sheet coords (holes datum)
    pub holes: Vec<(f64, f64)>,
    /// obround slots (cx, cy, w, h), sheet coords (slots datum); w along x, h along y
    pub slots: Vec<(f64, f64, f64, f64)>,
    /// jig pin centres, sheet coords (one per slot; == holes for the holes datum)
    pub pins: Vec<(f64, f64)>,
    /// sheet coords of the corner the piece is pushed toward (bottom-left of the cell)
    pub datum_corner: (f64, f64),
    /// centre of the X orientation marker, sheet coords
    pub marker: Option<(f64, f64)>,
}

impl Area {
    pub fn rect(&self) -> Bounds {
        (self.x, self.y, self.x + self.w, self.y + self.h)
    }
}

#[derive(Clone, Debug)]
pub struct Layout {
    pub params: LayoutParams,
    pub areas: Vec<Area>,
    /// stencil sheet size (mm)
    pub width: f64,
    pub height: f64,
    /// bbox of all cells on the sheet
    pub block: Bounds,
    /// block fits inside the sheet
    pub fits: bool,
    /// divider dot centres
    pub dots: Vec<(f64, f64)>,
    /// dotted line segments (x0, y0, x1, y1)
    pub dividers: Vec<(f64, f64, f64, f64)>,
    /// MaxRects heuristic that won
    pub heuristic: String,
    /// number of cells that did not fit
    pub overflow: usize,
    /// dots the `dot_clearance` rule removed (too close to a slot, hole or marker)
    pub dots_dropped: usize,
}

impl Layout {
    pub fn block_width(&self) -> f64 {
        self.block.2 - self.block.0
    }
    pub fn block_height(&self) -> f64 {
        self.block.3 - self.block.1
    }
}
