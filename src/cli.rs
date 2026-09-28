//! Command line front-end: gerbers in, one merged stencil order out.
//!
//! Run it from the folder that holds the KiCad gerber zips:
//!
//! ```text
//! stencicrity
//! ```
//!
//! It discovers the projects, loads `./.stencicrity` (the project's
//! configuration: stencil size, layout, the rules that give fresh pads their
//! default state, which sides to place, what to do with copper pads without
//! paste and which paste openings to close), applies the options given on the
//! command line, shows a preview and a TUI, saves the configuration again and
//! writes the `F_Paste` / `F_Cu` gerbers, a preview PNG, a report and a zip.
//!
//! Precedence: an explicitly given command line option wins over the
//! `.stencicrity` file, which wins over the built-in default. Options that can
//! come from the file default to `None` ("not given") instead of a value.

use std::io::Write as _;
use std::path::{Component, Path, PathBuf};

use clap::builder::PossibleValuesParser;
use clap::{CommandFactory, Parser};

use crate::config::{
    apply_config, load_config, parse_prefixes, save_config, CONFIG_FILENAME, LEGACY_CONFIG_SUFFIX,
};
use crate::layout::{layout_report, marker_strokes, pack};
use crate::model::{
    parse_size, Config, Layout, Project, Side, SideId, DATUM_HOLES, DATUM_MODES, DATUM_NONE,
    ORIENTATION_LANDSCAPE, ORIENTATION_PORTRAIT, SIDE_BOTTOM, SIDE_TOP, SORT_ORDERS,
};
use crate::pads::{closed_count, detect_pads, sorted_pads, state_counts, DetectOptions};
use crate::project::{discover_projects, DiscoverOptions};
use crate::render::{open_file, render_preview, RenderOptions};
use crate::tui::{run_tui, TuiHooks};
use crate::writer::GerberWriter;

pub const DEFAULT_OUT: &str = "./stencil-out";
pub const DEFAULT_NAME: &str = "stencil";

/// The `--size` choices, `STENCIL_SIZES` as the file spells them.
const SIZE_CHOICES: [&str; 8] = [
    "270x270", "380x280", "420x320", "450x350", "460x460", "520x420", "600x600", "700x600",
];

/// Shapely's `join_style="mitre"` keeps its default `mitre_limit` of 5, i.e.
/// the sharp corner survives while the miter stays within five offsets of the
/// original vertex. That is an interior angle of `2 * asin(1 / 5)`, which is
/// exactly what `geo`'s `LineJoin::Miter` wants.
fn mitre_min_angle() -> f64 {
    2.0 * (1.0f64 / 5.0).asin()
}

// --------------------------------------------------------------------------- //
// Argument parsing
// --------------------------------------------------------------------------- //

/// The full `stencicrity` command line.
#[derive(Debug, Parser)]
#[command(
    name = "stencicrity",
    version,
    // argparse lets `--gap -1` through (no option looks like a negative
    // number), so the port has to as well.
    allow_negative_numbers = true,
    about = "Merge KiCad paste gerbers of several projects into one stencil order.",
    after_help = "Options marked 'from the .stencicrity file' keep the value stored in the \
                  configuration file when they are not given; when they are given they override \
                  it and are saved back."
)]
pub struct Args {
    /// zip files or gerber directories (default: every *.zip and gerber subdirectory of the current folder)
    #[arg(value_name = "INPUTS")]
    pub inputs: Vec<String>,

    /// output directory
    #[arg(long, default_value = DEFAULT_OUT, value_name = "DIR")]
    pub out: String,

    /// base name of the generated files
    #[arg(long, default_value = DEFAULT_NAME, value_name = "NAME")]
    pub name: String,

    /// configuration and pad decision file (default: ./.stencicrity)
    // `--overrides` is the pre-0.1.2 spelling: still accepted, no longer advertised.
    #[arg(long, alias = "overrides", value_name = "FILE")]
    pub config: Option<String>,

    // -- stencil sheet ----------------------------------------------------- //
    /// stencil sheet size in mm (from the .stencicrity file, else 380x280)
    #[arg(long, value_parser = PossibleValuesParser::new(SIZE_CHOICES),
          help_heading = "stencil sheet")]
    pub size: Option<String>,

    /// long side horizontal
    #[arg(long, help_heading = "stencil sheet")]
    pub landscape: bool,

    /// long side vertical
    #[arg(long, conflicts_with = "landscape", help_heading = "stencil sheet")]
    pub portrait: bool,

    // -- layout ------------------------------------------------------------ //
    /// spacing between neighbouring boards; the dotted border runs in the middle of it (default 30)
    #[arg(
        long,
        value_name = "MM",
        help_heading = "layout (mm; from the .stencicrity file)"
    )]
    pub gap: Option<f64>,

    /// alignment features cut into every cell: 'slots' opens an obround slot at every jig raster position along the cell's bottom and left edge, 'holes' the four corner dowel holes, 'none' nothing (default slots)
    #[arg(long, value_parser = PossibleValuesParser::new(DATUM_MODES),
          help_heading = "layout (mm; from the .stencicrity file)")]
    pub datum: Option<String>,

    /// legacy alias for --datum holes
    #[arg(
        long,
        conflicts_with = "datum",
        help_heading = "layout (mm; from the .stencicrity file)"
    )]
    pub holes: bool,

    /// legacy alias for --datum none
    #[arg(long = "no-holes", conflicts_with_all = ["datum", "holes"],
          help_heading = "layout (mm; from the .stencicrity file)")]
    pub no_holes: bool,

    /// dowel pin hole diameter, holes datum (default 5)
    #[arg(
        long = "hole-dia",
        value_name = "MM",
        help_heading = "layout (mm; from the .stencicrity file)"
    )]
    pub hole_dia: Option<f64>,

    /// dotted line to hole edge; negative puts the hole onto the line, holes datum (default 2)
    #[arg(
        long = "hole-inset",
        value_name = "MM",
        help_heading = "layout (mm; from the .stencicrity file)"
    )]
    pub hole_inset: Option<f64>,

    /// slot size across the cell edge (default 4.5)
    #[arg(
        long = "slot-width",
        value_name = "MM",
        help_heading = "layout (mm; from the .stencicrity file)"
    )]
    pub slot_width: Option<f64>,

    /// slot size along the cell edge (default 8)
    #[arg(
        long = "slot-length",
        value_name = "MM",
        help_heading = "layout (mm; from the .stencicrity file)"
    )]
    pub slot_length: Option<f64>,

    /// cell edge to the slot's outer wall; the slot may clip the cell's own dotted line, never the neighbouring cell (default 0.5)
    #[arg(
        long = "slot-offset",
        value_name = "MM",
        help_heading = "layout (mm; from the .stencicrity file)"
    )]
    pub slot_offset: Option<f64>,

    /// raster of the modular jig: slot and pin centres along the bottom and left edge sit on it and cells grow to whole multiples of it (default 20)
    #[arg(
        long = "slot-pitch",
        value_name = "MM",
        help_heading = "layout (mm; from the .stencicrity file)"
    )]
    pub slot_pitch: Option<f64>,

    /// foil kept between a slot and the board; cells grow until it fits (default 3)
    #[arg(
        long = "slot-web",
        value_name = "MM",
        help_heading = "layout (mm; from the .stencicrity file)"
    )]
    pub slot_web: Option<f64>,

    /// jig pin diameter for the slots datum; the pin touches the slot wall nearest the board (default 3)
    #[arg(
        long = "pin-dia",
        value_name = "MM",
        help_heading = "layout (mm; from the .stencicrity file)"
    )]
    pub pin_dia: Option<f64>,

    /// slots datum: cut an X at the raster point inside the datum corner so the piece's orientation can be read (default)
    #[arg(long, help_heading = "layout (mm; from the .stencicrity file)")]
    pub marker: bool,

    /// do not cut the orientation X
    #[arg(
        long = "no-marker",
        conflicts_with = "marker",
        help_heading = "layout (mm; from the .stencicrity file)"
    )]
    pub no_marker: bool,

    /// stroke length of the orientation X; its stroke width is --dot-dia (default 4)
    #[arg(
        long = "marker-size",
        value_name = "MM",
        help_heading = "layout (mm; from the .stencicrity file)"
    )]
    pub marker_size: Option<f64>,

    /// divider dot diameter (default 0.5)
    #[arg(
        long = "dot-dia",
        value_name = "MM",
        help_heading = "layout (mm; from the .stencicrity file)"
    )]
    pub dot_dia: Option<f64>,

    /// divider dot spacing (default 3)
    #[arg(
        long = "dot-pitch",
        value_name = "MM",
        help_heading = "layout (mm; from the .stencicrity file)"
    )]
    pub dot_pitch: Option<f64>,

    /// distance between the two dotted lines of a cell edge; 0 draws a single line on the edge (default 2.5)
    #[arg(
        long = "dot-line-gap",
        value_name = "MM",
        help_heading = "layout (mm; from the .stencicrity file)"
    )]
    pub dot_line_gap: Option<f64>,

    /// metal kept between a divider dot and a slot, hole or marker; a dot that would leave less is dropped, 0 drops only the dots that overlap one (default 0.5)
    #[arg(
        long = "dot-clearance",
        value_name = "MM",
        help_heading = "layout (mm; from the .stencicrity file)"
    )]
    pub dot_clearance: Option<f64>,

    /// holes datum: put every dowel hole of the sheet on one common grid of this pitch, 0 switches it off; the slots datum uses --slot-pitch (default 8)
    #[arg(
        long = "hole-grid",
        value_name = "MM",
        help_heading = "layout (mm; from the .stencicrity file)"
    )]
    pub hole_grid: Option<f64>,

    /// also dot the cell edges on the outer boundary of the block
    #[arg(
        long = "outer-border",
        help_heading = "layout (mm; from the .stencicrity file)"
    )]
    pub outer_border: bool,

    /// dot only the shared edges (default)
    #[arg(
        long = "no-outer-border",
        conflicts_with = "outer_border",
        help_heading = "layout (mm; from the .stencicrity file)"
    )]
    pub no_outer_border: bool,

    /// order the cells are packed in: 'height' offers the tallest boards first (tighter packing), 'name' keeps projects alphabetical; top always comes before bottom of the same board (default height)
    #[arg(long, value_parser = PossibleValuesParser::new(SORT_ORDERS),
          help_heading = "layout (mm; from the .stencicrity file)")]
    pub sort: Option<String>,

    // -- pads --------------------------------------------------------------- //
    /// do not mirror bottom sides (they normally are)
    #[arg(long = "no-mirror-bottom", help_heading = "pads")]
    pub no_mirror_bottom: bool,

    /// also offer through hole pads as candidates
    #[arg(long = "include-tht", help_heading = "pads")]
    pub include_tht: bool,

    /// shrink openings cut for copper pads by this much
    #[arg(
        long = "open-shrink",
        default_value_t = 0.0,
        value_name = "MM",
        help_heading = "pads"
    )]
    pub open_shrink: f64,

    /// reference prefix whose pads without paste default to 'ignore' (prefix + digit, e.g. TP3); repeatable, replaces the stored list and is saved to the .stencicrity file; pass an empty string to clear it (from the file, else NT TP)
    #[arg(long = "ignore-prefix", value_name = "PREFIX", action = clap::ArgAction::Append,
          help_heading = "pads")]
    pub ignore_prefix: Option<Vec<String>>,

    // -- selection ---------------------------------------------------------- //
    /// switch a project (or one of its sides) off; repeatable, saved to the .stencicrity file
    #[arg(long, value_name = "PROJECT[:top|bottom]", action = clap::ArgAction::Append,
          help_heading = "selection")]
    pub exclude: Vec<String>,

    /// switch everything else off; repeatable, saved to the .stencicrity file
    #[arg(long, value_name = "PROJECT[:top|bottom]", action = clap::ArgAction::Append,
          help_heading = "selection")]
    pub only: Vec<String>,

    // -- output ------------------------------------------------------------- //
    /// preview resolution
    #[arg(long = "px-per-mm", default_value_t = 20.0, help_heading = "output")]
    pub px_per_mm: f64,

    /// do not open the preview in an image viewer
    #[arg(long = "no-open", help_heading = "output")]
    pub no_open: bool,

    /// no TUI: undefined pads are left closed
    #[arg(long, help_heading = "output")]
    pub batch: bool,

    /// also write the stencil rectangle as Edge_Cuts
    #[arg(long, help_heading = "output")]
    pub outline: bool,

    /// do not write the F_Cu reference layer
    #[arg(long = "no-copper", help_heading = "output")]
    pub no_copper: bool,
}

impl Args {
    /// `--landscape` / `--portrait` as one value, `None` when neither is given.
    pub fn orientation(&self) -> Option<&'static str> {
        if self.landscape {
            Some(ORIENTATION_LANDSCAPE)
        } else if self.portrait {
            Some(ORIENTATION_PORTRAIT)
        } else {
            None
        }
    }

    /// `--datum` / `--holes` / `--no-holes` as one value.
    pub fn datum_mode(&self) -> Option<String> {
        if let Some(datum) = &self.datum {
            Some(datum.clone())
        } else if self.holes {
            Some(DATUM_HOLES.to_string())
        } else if self.no_holes {
            Some(DATUM_NONE.to_string())
        } else {
            None
        }
    }

    /// `--marker` / `--no-marker`.
    pub fn marker_flag(&self) -> Option<bool> {
        tri(self.marker, self.no_marker)
    }

    /// `--outer-border` / `--no-outer-border`.
    pub fn outer_border_flag(&self) -> Option<bool> {
        tri(self.outer_border, self.no_outer_border)
    }
}

fn tri(yes: bool, no: bool) -> Option<bool> {
    if yes {
        Some(true)
    } else if no {
        Some(false)
    } else {
        None
    }
}

/// Reject impossible numbers before anything is loaded; `Err` is the message.
pub fn check_args(args: &Args) -> Result<(), String> {
    let non_negative = "must not be negative";
    let positive = "must be positive";
    let checks: [(&str, Option<f64>, bool, &str); 16] = [
        ("--gap", args.gap, false, non_negative),
        ("--hole-dia", args.hole_dia, true, positive),
        ("--slot-width", args.slot_width, true, positive),
        ("--slot-length", args.slot_length, true, positive),
        ("--slot-offset", args.slot_offset, false, non_negative),
        ("--slot-pitch", args.slot_pitch, true, positive),
        ("--slot-web", args.slot_web, true, positive),
        ("--pin-dia", args.pin_dia, true, positive),
        ("--marker-size", args.marker_size, true, positive),
        ("--dot-dia", args.dot_dia, true, positive),
        ("--dot-pitch", args.dot_pitch, true, positive),
        ("--dot-line-gap", args.dot_line_gap, false, non_negative),
        ("--dot-clearance", args.dot_clearance, false, non_negative),
        ("--hole-grid", args.hole_grid, false, non_negative),
        ("--px-per-mm", Some(args.px_per_mm), true, positive),
        ("--open-shrink", Some(args.open_shrink), false, non_negative),
    ];
    for (option, value, strict, requirement) in checks {
        if let Some(v) = value {
            let ok = if strict { v > 0.0 } else { v >= 0.0 };
            if !ok {
                return Err(format!("{option} {requirement}"));
            }
        }
    }
    Ok(())
}

/// Overwrite the stored configuration with the options actually given.
pub fn apply_cli_config(config: &mut Config, args: &Args) {
    if let Some(size) = &args.size {
        if let Ok(parsed) = parse_size(size) {
            config.size = parsed;
        }
    }
    if let Some(specs) = &args.ignore_prefix {
        let mut prefixes: Vec<String> = Vec::new();
        for spec in specs {
            for prefix in parse_prefixes(spec) {
                if !prefixes.contains(&prefix) {
                    prefixes.push(prefix);
                }
            }
        }
        config.ignore_prefixes = prefixes;
    }
    if let Some(orientation) = args.orientation() {
        config.orientation = orientation.to_string();
    }
    let params = &mut config.layout;
    if let Some(v) = args.gap {
        params.gap = v;
    }
    if let Some(v) = args.datum_mode() {
        params.datum = v;
    }
    if let Some(v) = args.hole_dia {
        params.hole_dia = v;
    }
    if let Some(v) = args.hole_inset {
        params.hole_inset = v;
    }
    if let Some(v) = args.slot_width {
        params.slot_width = v;
    }
    if let Some(v) = args.slot_length {
        params.slot_length = v;
    }
    if let Some(v) = args.slot_offset {
        params.slot_offset = v;
    }
    if let Some(v) = args.slot_pitch {
        params.slot_pitch = v;
    }
    if let Some(v) = args.slot_web {
        params.slot_web = v;
    }
    if let Some(v) = args.pin_dia {
        params.pin_dia = v;
    }
    if let Some(v) = args.marker_flag() {
        params.marker = v;
    }
    if let Some(v) = args.marker_size {
        params.marker_size = v;
    }
    if let Some(v) = args.dot_dia {
        params.dot_dia = v;
    }
    if let Some(v) = args.dot_pitch {
        params.dot_pitch = v;
    }
    if let Some(v) = args.dot_line_gap {
        params.dot_line_gap = v;
    }
    if let Some(v) = args.dot_clearance {
        params.dot_clearance = v;
    }
    if let Some(v) = args.hole_grid {
        params.hole_grid = v;
    }
    if let Some(v) = args.outer_border_flag() {
        params.outer_border = v;
    }
    if let Some(v) = &args.sort {
        params.sort = v.clone();
    }
}

// --------------------------------------------------------------------------- //
// Project / side selection
// --------------------------------------------------------------------------- //

/// `"board:bottom"` -> `("board", Some("bottom"))`; the side is optional.
pub fn parse_filter(spec: &str) -> (String, Option<String>) {
    if let Some(cut) = spec.rfind(':') {
        let (name, side) = (&spec[..cut], &spec[cut + 1..]);
        let side = side.trim().to_lowercase();
        if side == SIDE_TOP || side == SIDE_BOTTOM {
            return (name.trim().to_lowercase(), Some(side));
        }
    }
    (spec.trim().to_lowercase(), None)
}

/// True when a `--only` / `--exclude` spec selects this side.
pub fn filter_matches(side: &Side, spec: &(String, Option<String>)) -> bool {
    let (name, want_side) = spec;
    let project = side.project_name.to_lowercase();
    if !name.is_empty() && *name != project && !project.contains(name.as_str()) {
        return false;
    }
    match want_side {
        None => true,
        Some(want) => *want == side.name,
    }
}

/// Switch sides on/off from `--only` / `--exclude`; true when either was used.
pub fn apply_selection(projects: &mut [Project], only: &[String], exclude: &[String]) -> bool {
    let only_specs: Vec<_> = only.iter().map(|s| parse_filter(s)).collect();
    let exclude_specs: Vec<_> = exclude.iter().map(|s| parse_filter(s)).collect();
    if only_specs.is_empty() && exclude_specs.is_empty() {
        return false;
    }
    for project in projects.iter_mut() {
        for side in project.sides.iter_mut() {
            if !only_specs.is_empty() {
                side.enabled = only_specs.iter().any(|f| filter_matches(side, f));
            }
            if side.enabled && exclude_specs.iter().any(|f| filter_matches(side, f)) {
                side.enabled = false;
            }
        }
    }
    true
}

/// Rendered overview table of every relevant side.
pub fn side_table(projects: &[Project], sides: &[SideId]) -> Vec<String> {
    let head = format!(
        "{:<24} {:<7} {:<4} {:<4} {:>16} {:>7} {:>7} {:>6}",
        "project", "side", "use", "mir", "board (mm)", "paste", "closed", "cand"
    );
    let rule = "-".repeat(head.chars().count());
    let mut lines = vec![head, rule];
    for id in sides {
        let side = id.get(projects);
        let size = format!("{:.1} x {:.1}", side.board_width(), side.board_height());
        let name: String = side.project_name.chars().take(24).collect();
        lines.push(format!(
            "{:<24} {:<7} {:<4} {:<4} {:>16} {:>7} {:>7} {:>6}",
            name,
            side.name,
            if side.enabled { "on" } else { "off" },
            if side.mirror { "yes" } else { "no" },
            size,
            side.paste_objects().len(),
            side.closed_pads().count(),
            side.candidates().count()
        ));
    }
    lines
}

// --------------------------------------------------------------------------- //
// Generation
// --------------------------------------------------------------------------- //

/// (undefined, open, ignore) over the side's candidate pads.
fn pad_counts(side: &Side) -> (usize, usize, usize) {
    state_counts(side.candidates())
}

/// Write the paste layer: source openings, decided pads, dots and the datum.
///
/// Openings of pads the user closed are left out (`Side::active_paste_objects`).
/// The alignment features come last: one obround slot per jig raster position
/// along the bottom and left edge of every cell for the `slots` datum, four
/// round holes per cell for `holes`, nothing for `none`; the `slots` datum also
/// strokes an X into the foil just inside every datum corner (`marker`), so the
/// orientation of a cut-out piece can be read.
fn write_paste(
    projects: &[Project],
    layout: &Layout,
    path: &Path,
    name: &str,
    open_shrink: f64,
) -> std::io::Result<()> {
    let mut writer = GerberWriter::new("Paste,Top");
    writer.comment(&format!("stencil {name} - merged paste layer"));
    for area in &layout.areas {
        let side = area.side.get(projects);
        let suffix = if area.transform.mirror {
            " (mirrored)"
        } else {
            ""
        };
        writer.comment(&format!("--- {}{suffix} ---", side.label()));
        for obj in side.active_paste_objects() {
            writer.add_object(obj, &area.transform);
        }
        for pad in side.open_pads() {
            if open_shrink <= 0.0 {
                writer.add_object(
                    &crate::gerber::GraphicObject::Flash(pad.flash.clone()),
                    &area.transform,
                );
                continue;
            }
            let shrunk = shrink(&pad.geom, open_shrink);
            if shrunk.0.is_empty() {
                continue;
            }
            for poly in area.transform.apply_geom(&shrunk).0 {
                let points: Vec<(f64, f64)> =
                    poly.exterior().coords().map(|c| (c.x, c.y)).collect();
                writer.add_polygon(&points);
            }
        }
    }
    if !layout.dots.is_empty() {
        writer.comment("--- border dots ---");
        for &(x, y) in &layout.dots {
            writer.add_circle(x, y, layout.params.dot_dia, None);
        }
    }
    if layout.areas.iter().any(|a| !a.slots.is_empty()) {
        writer.comment("--- alignment slots ---");
        for area in &layout.areas {
            for &(cx, cy, w, h) in &area.slots {
                writer.add_obround(cx, cy, w, h, None);
            }
        }
    }
    if layout.areas.iter().any(|a| !a.holes.is_empty()) {
        writer.comment("--- dowel pin holes ---");
        for area in &layout.areas {
            for &(x, y) in &area.holes {
                writer.add_circle(x, y, layout.params.hole_dia, None);
            }
        }
    }
    if layout.areas.iter().any(|a| a.marker.is_some()) {
        writer.comment("--- orientation markers ---");
        let params = &layout.params;
        for area in &layout.areas {
            let Some(marker) = area.marker else { continue };
            for (x0, y0, x1, y1) in marker_strokes(marker, params.marker_size) {
                writer.add_line(x0, y0, x1, y1, params.dot_dia);
            }
        }
    }
    writer.write(path)
}

/// `geom` offset inward by `distance` with mitred joins (Shapely's
/// `buffer(-d, join_style="mitre")`).
fn shrink(geom: &crate::model::Geom, distance: f64) -> crate::model::Geom {
    use geo::algorithm::buffer::{Buffer, BufferStyle, LineJoin};
    let style = BufferStyle::new(-distance).line_join(LineJoin::Miter(mitre_min_angle()));
    geom.buffer_with_style(style)
}

/// Write the copper reference layer (not cut, only for checking).
fn write_copper(
    projects: &[Project],
    layout: &Layout,
    path: &Path,
    name: &str,
) -> std::io::Result<()> {
    let mut writer = GerberWriter::new("Copper,L1,Top");
    writer.comment(&format!("stencil {name} - copper reference"));
    for area in &layout.areas {
        let side = area.side.get(projects);
        let suffix = if area.transform.mirror {
            " (mirrored)"
        } else {
            ""
        };
        writer.comment(&format!("--- {}{suffix} ---", side.label()));
        for obj in side.copper_objects() {
            writer.add_object(obj, &area.transform);
        }
    }
    writer.write(path)
}

/// Write the stencil rectangle (0, 0) - (W, H) as a profile layer.
fn write_outline(layout: &Layout, path: &Path, name: &str) -> std::io::Result<()> {
    let mut writer = GerberWriter::new("Profile,NP");
    writer.comment(&format!("stencil {name} - sheet outline"));
    writer.add_rect_outline(0.0, 0.0, layout.width, layout.height, 0.1);
    writer.write(path)
}

/// Pack the written gerbers into one deflate zip.
fn zip_files(zip_path: &Path, paths: &[PathBuf]) -> std::io::Result<()> {
    use zip::write::SimpleFileOptions;
    let file = std::fs::File::create(zip_path)?;
    let mut archive = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for path in paths {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        archive
            .start_file(name, options)
            .map_err(std::io::Error::other)?;
        let bytes = std::fs::read(path)?;
        archive.write_all(&bytes)?;
    }
    archive.finish().map_err(std::io::Error::other)?;
    Ok(())
}

fn fit_text(layout: &Layout) -> &'static str {
    if layout.fits {
        "fits"
    } else {
        "DOES NOT FIT"
    }
}

/// Human readable summary shown on stdout and stored in the report.
#[allow(clippy::too_many_arguments)]
fn summary(
    projects: &[Project],
    layout: &Layout,
    config: &Config,
    dropped: &[SideId],
    disabled: &[SideId],
    files: &[String],
    config_path: &str,
    undefined: usize,
) -> Vec<String> {
    let mut lines = vec![
        format!(
            "stencil: {:.1} x {:.1} mm ({}, {}), datum: {}",
            layout.width,
            layout.height,
            config.size_label(),
            config.orientation,
            config.layout.datum
        ),
        format!(
            "block:   {:.1} x {:.1} mm, {} area(s) — {}",
            layout.block_width(),
            layout.block_height(),
            layout.areas.len(),
            fit_text(layout)
        ),
    ];
    for area in &layout.areas {
        let side = area.side.get(projects);
        let (undef, open, ignore) = pad_counts(side);
        let suffix = if area.transform.mirror {
            " (mirrored)"
        } else {
            ""
        };
        lines.push(format!(
            "  {:<34} at ({:7.2}, {:7.2}) {:6.1} x {:6.1}  paste={:<5} closed={:<4} open={:<4} \
             ignore={:<4} undefined={:<4} pins={}",
            format!("{}{suffix}", side.label()),
            area.x,
            area.y,
            area.w,
            area.h,
            side.active_paste_objects().len(),
            side.closed_pads().count(),
            open,
            ignore,
            undef,
            area.pins.len()
        ));
    }
    for id in dropped {
        lines.push(format!(
            "  dropped (no openings): {}",
            id.get(projects).label()
        ));
    }
    for id in disabled {
        lines.push(format!("  switched off: {}", id.get(projects).label()));
    }
    lines.push(format!("configuration: {config_path}"));
    lines.push("files:".to_string());
    lines.extend(files.iter().map(|p| format!("  {p}")));
    if undefined > 0 {
        lines.push(format!(
            "note: {undefined} pad(s) are still undefined and got NO opening"
        ));
    }
    lines
}

// --------------------------------------------------------------------------- //
// Paths
// --------------------------------------------------------------------------- //

/// Lexical `os.path.abspath`: join to the working directory, drop `.`, fold `..`.
fn abspath(path: &str) -> PathBuf {
    let raw = Path::new(path);
    let joined = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(raw)
    };
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn show(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

// --------------------------------------------------------------------------- //
// Main flow
// --------------------------------------------------------------------------- //

/// Print a warning to stderr, keeping it in order with stdout.
fn warn(message: &str) {
    let _ = std::io::stdout().flush();
    eprintln!("warning: {message}");
    let _ = std::io::stderr().flush();
}

/// The whole pipeline; `Err` is a user level problem (`GerberError`/io).
fn run(args: &Args) -> anyhow::Result<i32> {
    let cwd = std::env::current_dir()?;
    let out_dir = abspath(&args.out);
    let name = args.name.as_str();
    let config_path: PathBuf = match &args.config {
        Some(path) => PathBuf::from(path),
        None => cwd.join(CONFIG_FILENAME),
    };
    // Pre-0.1.2 runs stored the configuration in ./<name>.stencil; read it once
    // more and write it to the new place (the old file is left alone).
    let mut legacy_path: Option<PathBuf> = None;
    if args.config.is_none() && !config_path.exists() {
        let candidate = cwd.join(format!("{name}{LEGACY_CONFIG_SUFFIX}"));
        if candidate.is_file() {
            legacy_path = Some(candidate);
        }
    }

    // 1. discover ---------------------------------------------------------- //
    let mut projects = discover_projects(
        &args.inputs,
        &cwd.to_string_lossy(),
        &DiscoverOptions {
            mirror_bottom: !args.no_mirror_bottom,
            exclude_dirs: vec![show(&out_dir)],
        },
        &mut |m: &str| warn(m),
    )?;
    if projects.is_empty() {
        eprintln!("error: no gerber projects found (pass zip files or directories)");
        return Ok(2);
    }
    println!("{} project(s) found", projects.len());

    // 2. configuration ------------------------------------------------------ //
    // The [rules] section decides the default state of a pad nobody decided on
    // yet, so the configuration has to be read before the pads are detected.
    let source_path = legacy_path.clone().unwrap_or_else(|| config_path.clone());
    let existed = source_path.exists();
    let mut config = load_config(&source_path, &mut |m: &str| warn(m))?;
    if let Some(legacy) = &legacy_path {
        println!("note: migrated {} to {CONFIG_FILENAME}", show(legacy));
    }
    apply_cli_config(&mut config, args);
    let opts = DetectOptions {
        include_tht: args.include_tht,
        ignore_prefixes: config.ignore_prefixes.clone(),
        ..DetectOptions::default()
    };
    for project in projects.iter_mut() {
        for side in project.sides.iter_mut() {
            detect_pads(side, &opts);
        }
    }
    apply_config(&config, &mut projects);
    apply_selection(&mut projects, &args.only, &args.exclude);
    save_config(&config_path, &mut config, &projects)?;
    let config_text = show(&config_path);
    if existed {
        println!(
            "configuration: {config_text} ({} side switch(es), {} pad decision(s))",
            config.sides.len(),
            config.pads.len()
        );
    } else {
        println!("configuration: {config_text} (new)");
    }

    // 3. the sides we can place --------------------------------------------- //
    let sides: Vec<SideId> = crate::model::all_sides(&projects)
        .into_iter()
        .filter(|id| id.get(&projects).is_relevant())
        .collect();
    if sides.is_empty() {
        eprintln!("error: no side has paste openings or pads to decide on");
        return Ok(2);
    }
    println!();
    for line in side_table(&projects, &sides) {
        println!("{line}");
    }
    println!();

    // 4. first preview ------------------------------------------------------ //
    std::fs::create_dir_all(&out_dir)?;
    let preview_path = out_dir.join(format!("{name}-preview.png"));
    let mut layout = pack(&projects, &sides, &config);
    println!(
        "stencil: {:.1} x {:.1} mm ({}, {}), datum: {}",
        layout.width,
        layout.height,
        config.size_label(),
        config.orientation,
        config.layout.datum
    );
    println!(
        "block:   {:.1} x {:.1} mm — {}",
        layout.block_width(),
        layout.block_height(),
        fit_text(&layout)
    );

    if !layout.areas.is_empty() {
        render_preview(
            &projects,
            &layout,
            &preview_path,
            &RenderOptions {
                px_per_mm: args.px_per_mm,
                title: name.to_string(),
                ..RenderOptions::default()
            },
        )?;
        println!("preview: {}", show(&preview_path));
        if !args.no_open {
            open_file(&preview_path);
        }
    } else {
        warn("every side is switched off, nothing to preview");
    }

    // 5. pad decisions -------------------------------------------------------- //
    let all_pads = sorted_pads(&projects, &sides, true);
    let candidates: Vec<(SideId, usize)> = all_pads
        .iter()
        .copied()
        .filter(|(id, i)| id.get(&projects).pads[*i].is_candidate())
        .collect();
    let (undef, open, ignore) =
        state_counts(candidates.iter().map(|(id, i)| &id.get(&projects).pads[*i]));
    println!(
        "candidate pads (copper without paste): {} (undefined {undef}, open {open}, ignore {ignore})",
        candidates.len()
    );
    println!(
        "closed openings: {}",
        closed_count(all_pads.iter().map(|(id, i)| &id.get(&projects).pads[*i]))
    );

    // 6. the TUI ------------------------------------------------------------- //
    if !args.batch {
        // The TUI gets every pad: it shows the candidates by default and the
        // pads that already have paste on demand, so they can be closed.
        let title = format!("{name}: {} pad(s) without paste", candidates.len());
        let px_per_mm = args.px_per_mm;
        let sides_ref = &sides;
        let preview_ref = &preview_path;
        let mut on_preview = |live_projects: &[Project],
                              cfg: &Config,
                              selected: Option<(SideId, usize)>,
                              do_open: bool|
         -> String {
            let live = pack(live_projects, sides_ref, cfg);
            if live.areas.is_empty() {
                return show(preview_ref);
            }
            let opts = RenderOptions {
                px_per_mm,
                selected,
                title: name.to_string(),
                ..RenderOptions::default()
            };
            if render_preview(live_projects, &live, preview_ref, &opts).is_ok() && do_open {
                open_file(preview_ref);
            }
            show(preview_ref)
        };
        let mut compute_layout =
            |live_projects: &[Project], cfg: &Config| pack(live_projects, sides_ref, cfg);
        let hooks = TuiHooks {
            on_preview: &mut on_preview,
            compute_layout: &mut compute_layout,
        };
        let confirmed = run_tui(&mut projects, &sides, &mut config, hooks, &title)?;
        save_config(&config_path, &mut config, &projects)?;
        if !confirmed {
            println!("aborted — configuration saved to {config_text}, nothing generated");
            return Ok(1);
        }
    } else {
        let undefined =
            state_counts(candidates.iter().map(|(id, i)| &id.get(&projects).pads[*i])).0;
        if undefined > 0 {
            warn(&format!(
                "--batch: {undefined} undefined pad(s) are treated as closed (edit {config_text} \
                 to change them)"
            ));
        }
        if !layout.fits {
            warn(&format!(
                "block {:.1} x {:.1} mm does not fit the {:.0} x {:.0} mm stencil",
                layout.block_width(),
                layout.block_height(),
                layout.width,
                layout.height
            ));
        }
    }

    // 7. generate ------------------------------------------------------------ //
    let final_sides: Vec<SideId> = sides
        .iter()
        .copied()
        .filter(|id| {
            let side = id.get(&projects);
            side.enabled && side.has_openings()
        })
        .collect();
    let dropped: Vec<SideId> = sides
        .iter()
        .copied()
        .filter(|id| {
            let side = id.get(&projects);
            side.enabled && !side.has_openings()
        })
        .collect();
    let disabled: Vec<SideId> = sides
        .iter()
        .copied()
        .filter(|id| !id.get(&projects).enabled)
        .collect();
    for id in &dropped {
        println!("dropped {}: no openings at all", id.get(&projects).label());
    }
    if final_sides.is_empty() {
        eprintln!("error: no enabled side has a single opening, nothing to generate");
        return Ok(2);
    }
    layout = pack(&projects, &final_sides, &config);
    if !layout.fits {
        warn(&format!(
            "the block of boards ({:.1} x {:.1} mm) does not fit the {:.0} x {:.0} mm stencil — \
             use a larger size, a smaller spacing or fewer boards",
            layout.block_width(),
            layout.block_height(),
            layout.width,
            layout.height
        ));
    }

    let paste_path = out_dir.join(format!("{name}-F_Paste.gbr"));
    write_paste(&projects, &layout, &paste_path, name, args.open_shrink)?;
    let mut gerbers = vec![paste_path];
    if !args.no_copper {
        let copper_path = out_dir.join(format!("{name}-F_Cu.gbr"));
        write_copper(&projects, &layout, &copper_path, name)?;
        gerbers.push(copper_path);
    }
    if args.outline {
        let edge_path = out_dir.join(format!("{name}-Edge_Cuts.gbr"));
        write_outline(&layout, &edge_path, name)?;
        gerbers.push(edge_path);
    }
    let zip_path = out_dir.join(format!("{name}.zip"));
    zip_files(&zip_path, &gerbers)?;

    render_preview(
        &projects,
        &layout,
        &preview_path,
        &RenderOptions {
            px_per_mm: args.px_per_mm,
            title: name.to_string(),
            ..RenderOptions::default()
        },
    )?;

    let remaining = state_counts(
        sorted_pads(&projects, &final_sides, false)
            .iter()
            .map(|(id, i)| &id.get(&projects).pads[*i]),
    )
    .0;
    let report_path = out_dir.join(format!("{name}-report.txt"));
    let mut files: Vec<String> = gerbers.iter().map(|p| show(p)).collect();
    files.push(show(&zip_path));
    files.push(show(&preview_path));
    let mut report_files = files.clone();
    report_files.push(show(&report_path));
    let summary = summary(
        &projects,
        &layout,
        &config,
        &dropped,
        &disabled,
        &report_files,
        &config_text,
        remaining,
    );
    let report = layout_report(&projects, &layout, &config);
    std::fs::write(
        &report_path,
        format!(
            "{}\n\n{}\n",
            report.trim_end_matches('\n'),
            summary.join("\n")
        ),
    )?;

    println!();
    println!("{}", summary.join("\n"));
    Ok(0)
}

/// Entry point; returns the process exit code (0 ok, 1 aborted in the TUI, 2 bad input).
pub fn main(args: Vec<String>) -> i32 {
    let parsed = match Args::try_parse_from(args) {
        Ok(parsed) => parsed,
        Err(err) => {
            let _ = err.print();
            return err.exit_code();
        }
    };
    if let Err(message) = check_args(&parsed) {
        let err = Args::command().error(clap::error::ErrorKind::ValueValidation, message);
        let _ = err.print();
        return 2;
    }
    match run(&parsed) {
        Ok(code) => code,
        Err(exc) => {
            let _ = std::io::stdout().flush();
            eprintln!("error: {exc}");
            2
        }
    }
}

// --------------------------------------------------------------------------- //
// Tests
// --------------------------------------------------------------------------- //

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{size_label, STENCIL_SIZES};

    #[test]
    fn the_command_line_is_well_formed() {
        Args::command().debug_assert();
    }

    #[test]
    fn size_choices_match_the_model() {
        let want: Vec<String> = STENCIL_SIZES.iter().map(|s| size_label(*s)).collect();
        assert_eq!(SIZE_CHOICES.to_vec(), want);
    }

    fn parse(extra: &[&str]) -> Args {
        let mut argv = vec!["stencicrity".to_string()];
        argv.extend(extra.iter().map(|s| s.to_string()));
        Args::try_parse_from(argv).expect("parse")
    }

    #[test]
    fn options_default_to_none() {
        let args = parse(&[]);
        assert_eq!(args.gap, None);
        assert_eq!(args.datum_mode(), None);
        assert_eq!(args.marker_flag(), None);
        assert_eq!(args.outer_border_flag(), None);
        assert_eq!(args.orientation(), None);
        assert_eq!(args.ignore_prefix, None);
        assert_eq!(args.out, DEFAULT_OUT);
        assert_eq!(args.name, DEFAULT_NAME);
        assert_eq!(args.open_shrink, 0.0);
        assert_eq!(args.px_per_mm, 20.0);
    }

    #[test]
    fn legacy_datum_aliases() {
        assert_eq!(parse(&["--holes"]).datum_mode().as_deref(), Some("holes"));
        assert_eq!(parse(&["--no-holes"]).datum_mode().as_deref(), Some("none"));
        assert_eq!(
            parse(&["--datum", "none"]).datum_mode().as_deref(),
            Some("none")
        );
    }

    #[test]
    fn overrides_is_a_hidden_alias_of_config() {
        assert_eq!(parse(&["--overrides", "x"]).config.as_deref(), Some("x"));
        assert_eq!(parse(&["--config", "y"]).config.as_deref(), Some("y"));
    }

    #[test]
    fn mutually_exclusive_groups_conflict() {
        for pair in [
            ["--landscape", "--portrait"],
            ["--marker", "--no-marker"],
            ["--outer-border", "--no-outer-border"],
            ["--holes", "--no-holes"],
        ] {
            let argv = vec!["stencicrity", pair[0], pair[1]];
            assert!(Args::try_parse_from(argv).is_err(), "{pair:?}");
        }
        assert!(Args::try_parse_from(["stencicrity", "--datum", "none", "--holes"]).is_err());
    }

    #[test]
    fn range_checks() {
        assert_eq!(
            check_args(&parse(&["--gap", "-1"])),
            Err("--gap must not be negative".to_string())
        );
        assert_eq!(
            check_args(&parse(&["--dot-dia", "0"])),
            Err("--dot-dia must be positive".to_string())
        );
        assert_eq!(
            check_args(&parse(&["--px-per-mm", "0"])),
            Err("--px-per-mm must be positive".to_string())
        );
        assert_eq!(
            check_args(&parse(&["--open-shrink", "-0.1"])),
            Err("--open-shrink must not be negative".to_string())
        );
        assert!(check_args(&parse(&["--gap", "0", "--hole-inset", "-3"])).is_ok());
    }

    #[test]
    fn prefixes_replace_and_clear() {
        let mut config = Config::default();
        apply_cli_config(&mut config, &parse(&["--ignore-prefix", "j, nt"]));
        assert_eq!(config.ignore_prefixes, vec!["J", "NT"]);
        let mut config = Config::default();
        apply_cli_config(&mut config, &parse(&["--ignore-prefix", ""]));
        assert!(config.ignore_prefixes.is_empty());
    }

    #[test]
    fn filters_parse_like_python() {
        assert_eq!(
            parse_filter("Board:Bottom"),
            ("board".to_string(), Some("bottom".to_string()))
        );
        assert_eq!(parse_filter(" MiXeD "), ("mixed".to_string(), None));
        // a colon that is not a side stays part of the name
        assert_eq!(parse_filter("a:b"), ("a:b".to_string(), None));
    }

    #[test]
    fn cli_overrides_the_file() {
        let mut config = Config::default();
        config.layout.gap = 12.0;
        apply_cli_config(&mut config, &parse(&[]));
        assert_eq!(config.layout.gap, 12.0);
        apply_cli_config(&mut config, &parse(&["--gap", "7.5", "--portrait"]));
        assert_eq!(config.layout.gap, 7.5);
        assert_eq!(config.orientation, ORIENTATION_PORTRAIT);
    }

    #[test]
    fn a_shrunk_square_keeps_its_corners() {
        let square = geo::MultiPolygon::new(vec![geo::Polygon::new(
            geo::LineString::from(vec![
                (0.0, 0.0),
                (2.0, 0.0),
                (2.0, 1.0),
                (0.0, 1.0),
                (0.0, 0.0),
            ]),
            vec![],
        )]);
        let small = shrink(&square, 0.1);
        let b = crate::gerber::geom_bounds(&small).expect("bounds");
        for (got, want) in [(b.0, 0.1), (b.1, 0.1), (b.2, 1.9), (b.3, 0.9)] {
            assert!((got - want).abs() < 1e-6, "{got} != {want}");
        }
        // shrinking a shape away leaves nothing
        assert!(shrink(&square, 1.0).0.is_empty());
    }
}
