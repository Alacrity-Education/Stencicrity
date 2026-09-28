//! The `.stencicrity` configuration file: sections stencil/layout/rules/sides/pads.
//!
//! It stores the stencil sheet (size and orientation), the layout parameters,
//! the rules that give fresh pads their default state, the on/off switch of
//! every board side and the open/ignore decision of every copper pad without a
//! paste opening (plus the pads *with* paste whose opening the user closed).
//!
//! Sections, keys and values are case insensitive; `#` starts a comment. Keys
//! that no longer match anything in the current gerbers are kept at the end of
//! their section so hand made decisions are never lost. A flat file in the old
//! format (only `<pad key> = <state>` lines, no sections) still loads: lines
//! before the first section header are read as `[pads]`.

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::model::{
    size_label, Config, Project, DATUM_HOLES, DATUM_MODES, DATUM_NONE, ORIENTATIONS, SORT_ORDERS,
    STATES, STATE_IGNORE, STATE_OPEN, STATE_UNDEFINED, STENCIL_SIZES,
};
use crate::pads::default_state;
use crate::util::natural_key;

pub const CONFIG_FILENAME: &str = ".stencicrity";
pub const LEGACY_CONFIG_SUFFIX: &str = ".stencil";

const SECTION_STENCIL: &str = "stencil";
const SECTION_LAYOUT: &str = "layout";
const SECTION_RULES: &str = "rules";
const SECTION_SIDES: &str = "sides";
const SECTION_PADS: &str = "pads";
const SECTIONS: [&str; 5] = [
    SECTION_STENCIL,
    SECTION_LAYOUT,
    SECTION_RULES,
    SECTION_SIDES,
    SECTION_PADS,
];

/// States a pad that already has a paste opening may be in.
const PASTED_STATES: [&str; 2] = [STATE_OPEN, STATE_IGNORE];

const TRUE_WORDS: [&str; 4] = ["on", "true", "yes", "1"];
const FALSE_WORDS: [&str; 4] = ["off", "false", "no", "0"];

/// Keys of older files that no longer mean anything; read and dropped in
/// silence (`slot_corner` placed the slots before they moved onto the
/// `slot_pitch` raster).
const OBSOLETE_LAYOUT_KEYS: [&str; 1] = ["slot_corner"];

const COMMENT_COLUMN: usize = 26;
const STALE_HEADER: &str = "# --- not found in the current gerbers ---";
const CLOSED_HEADER: &str = "# --- pads with paste whose opening is closed (state ignore) ---";

/// How a `[layout]` float is validated.
#[derive(Clone, Copy, PartialEq)]
enum FloatCheck {
    Any,
    NonNegative,
    Positive,
}

impl FloatCheck {
    fn accepts(self, v: f64) -> bool {
        match self {
            FloatCheck::Any => true,
            FloatCheck::NonNegative => v >= 0.0,
            FloatCheck::Positive => v > 0.0,
        }
    }
    fn requirement(self) -> &'static str {
        match self {
            FloatCheck::Any => "",
            FloatCheck::NonNegative => "must not be negative",
            FloatCheck::Positive => "must be positive",
        }
    }
}

/// `[layout]` key -> (field, check). The field names match `LayoutParams`.
const LAYOUT_FLOATS: [(&str, &str, FloatCheck); 15] = [
    ("spacing", "gap", FloatCheck::NonNegative),
    ("hole_dia", "hole_dia", FloatCheck::Positive),
    ("hole_inset", "hole_inset", FloatCheck::Any),
    ("slot_width", "slot_width", FloatCheck::Positive),
    ("slot_length", "slot_length", FloatCheck::Positive),
    ("slot_offset", "slot_offset", FloatCheck::NonNegative),
    ("slot_pitch", "slot_pitch", FloatCheck::Positive),
    ("slot_web", "slot_web", FloatCheck::Positive),
    ("pin_dia", "pin_dia", FloatCheck::Positive),
    ("marker_size", "marker_size", FloatCheck::Positive),
    ("dot_dia", "dot_dia", FloatCheck::Positive),
    ("dot_pitch", "dot_pitch", FloatCheck::Positive),
    ("dot_line_gap", "dot_line_gap", FloatCheck::NonNegative),
    ("dot_clearance", "dot_clearance", FloatCheck::NonNegative),
    ("hole_grid", "hole_grid", FloatCheck::NonNegative),
];

/// Accepted spellings of a `[layout]` key.
const LAYOUT_ALIASES: [(&str, &str); 2] = [("gap", "spacing"), ("border", "outer_border")];

// --------------------------------------------------------------------------- //
// Small text helpers
// --------------------------------------------------------------------------- //

/// Python's `repr()` of a string, so warnings read exactly like the original's.
pub(crate) fn py_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python's `str(float)`: shortest round-tripping text, always with a point.
pub(crate) fn py_float(v: f64) -> String {
    if v.is_nan() {
        return "nan".to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 {
            "inf".to_string()
        } else {
            "-inf".to_string()
        };
    }
    let s = format!("{v}");
    if s.contains('.') || s.contains('e') || s.contains('E') {
        s
    } else {
        format!("{s}.0")
    }
}

fn bool_text(flag: bool) -> &'static str {
    if flag {
        "on"
    } else {
        "off"
    }
}

/// Compact but exact enough decimal text: `30.0`, `0.5`, `-1.25`.
fn num_text(value: f64) -> String {
    let text = format!("{value:.4}");
    let trimmed = text.trim_end_matches('0');
    if trimmed.ends_with('.') {
        format!("{trimmed}0")
    } else {
        trimmed.to_string()
    }
}

fn entry(key: &str, value: &str, comment: &str) -> String {
    let line = format!("{key} = {value}");
    if comment.is_empty() {
        return line;
    }
    let padding = COMMENT_COLUMN.saturating_sub(line.chars().count()).max(3);
    format!("{line}{}# {comment}", " ".repeat(padding))
}

fn plural(count: usize, word: &str) -> String {
    if count == 1 {
        format!("{count} {word}")
    } else {
        format!("{count} {word}s")
    }
}

/// `on/true/yes/1` -> Some(true), `off/false/no/0` -> Some(false), else None.
pub fn parse_bool(value: &str) -> Option<bool> {
    let text = value.trim().to_lowercase();
    if TRUE_WORDS.contains(&text.as_str()) {
        return Some(true);
    }
    if FALSE_WORDS.contains(&text.as_str()) {
        return Some(false);
    }
    None
}

/// `"nt, TP"` -> `["NT", "TP"]`; empty text gives an empty list.
pub fn parse_prefixes(value: &str) -> Vec<String> {
    let mut prefixes: Vec<String> = Vec::new();
    for token in value.trim().split(|c: char| c == ',' || c.is_whitespace()) {
        let upper = token.trim().to_uppercase();
        if !upper.is_empty() && !prefixes.contains(&upper) {
            prefixes.push(upper);
        }
    }
    prefixes
}

/// Python's `float()` on a stripped token (no thousands separators, no underscores).
fn py_parse_float(text: &str) -> Option<f64> {
    let t = text.trim();
    if t.is_empty() || t.contains('_') {
        return None;
    }
    t.parse::<f64>().ok()
}

// --------------------------------------------------------------------------- //
// Reading
// --------------------------------------------------------------------------- //

/// Split like Python's universal-newline `readlines()` (without the endings).
fn split_lines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                out.push(&text[start..i]);
                i += 1;
                start = i;
            }
            b'\r' => {
                out.push(&text[start..i]);
                i += if i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    2
                } else {
                    1
                };
                start = i;
            }
            _ => i += 1,
        }
    }
    if start < bytes.len() {
        out.push(&text[start..]);
    }
    out
}

/// Cut a line at the first whitespace followed by `#` (`re.split(r"\s#")`).
fn strip_comment(line: &str) -> &str {
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    for w in chars.windows(2) {
        if w[0].1.is_whitespace() && w[1].1 == '#' {
            return &line[..w[0].0];
        }
    }
    line
}

/// Missing file -> defaults; bad values warn and keep the default (never fails on content).
pub fn load_config(path: &Path, warn: &mut dyn FnMut(&str)) -> std::io::Result<Config> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(exc) if exc.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(exc) => {
            warn(&format!(
                "{}: cannot read the configuration ({exc})",
                path.display()
            ));
            return Ok(Config::default());
        }
    };
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(parse_config_at(&text, &path.display().to_string(), warn))
}

/// Parse file text (see `load_config`).
pub fn parse_config(text: &str, warn: &mut dyn FnMut(&str)) -> Config {
    parse_config_at(text, "", warn)
}

/// `origin` prefixes every warning (a file path, or empty for plain text).
fn parse_config_at(text: &str, origin: &str, warn: &mut dyn FnMut(&str)) -> Config {
    let mut config = Config::default();
    // Lines before the first header belong to [pads] (old, flat format).
    let mut section = SECTION_PADS.to_string();
    // Which [layout] keys the file actually carries: an explicit `datum`
    // beats the legacy `holes` switch whichever order they come in.
    let mut seen_layout: Vec<String> = Vec::new();

    for (index, raw) in split_lines(text).iter().enumerate() {
        let number = index + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = strip_comment(line).trim();
        if line.is_empty() {
            continue;
        }
        let where_ = if origin.is_empty() {
            format!("line {number}")
        } else {
            format!("{origin}:{number}")
        };
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_lowercase();
            if !SECTIONS.contains(&section.as_str()) {
                warn(&format!(
                    "{where_}: unknown section [{section}], its lines are ignored"
                ));
            }
            continue;
        }
        let Some(at) = line.rfind('=') else {
            warn(&format!(
                "{where_}: ignoring line without '=': {}",
                py_repr(line)
            ));
            continue;
        };
        let key = line[..at].trim();
        let value = line[at + 1..].trim();
        if key.is_empty() {
            warn(&format!("{where_}: ignoring line without a key"));
            continue;
        }
        match section.as_str() {
            SECTION_STENCIL => read_stencil(&mut config, &key.to_lowercase(), value, &where_, warn),
            SECTION_LAYOUT => read_layout(
                &mut config,
                &key.to_lowercase(),
                value,
                &where_,
                warn,
                &mut seen_layout,
            ),
            SECTION_RULES => read_rule(&mut config, &key.to_lowercase(), value, &where_, warn),
            SECTION_SIDES => read_side(&mut config, key, value, &where_, warn),
            SECTION_PADS => read_pad(&mut config, key, value, &where_, warn),
            _ => {}
        }
    }
    config
}

fn read_stencil(
    config: &mut Config,
    key: &str,
    value: &str,
    where_: &str,
    warn: &mut dyn FnMut(&str),
) {
    if key == "size" {
        let size = crate::model::parse_size(value).ok();
        match size {
            Some(size) if STENCIL_SIZES.contains(&size) => config.size = size,
            _ => {
                let known: Vec<String> = STENCIL_SIZES.iter().map(|s| size_label(*s)).collect();
                warn(&format!(
                    "{where_}: unknown stencil size {} (known: {}), using {}",
                    py_repr(value),
                    known.join(", "),
                    config.size_label()
                ));
            }
        }
    } else if key == "orientation" {
        let text = value.to_lowercase();
        if !ORIENTATIONS.contains(&text.as_str()) {
            warn(&format!(
                "{where_}: unknown orientation {} (use {}), using {}",
                py_repr(value),
                ORIENTATIONS.join(" | "),
                config.orientation
            ));
            return;
        }
        config.orientation = text;
    } else {
        warn(&format!(
            "{where_}: unknown [stencil] key {}, ignored",
            py_repr(key)
        ));
    }
}

/// Read/write access to the `LayoutParams` float fields by config key name.
fn layout_float<'a>(params: &'a mut crate::model::LayoutParams, field: &str) -> &'a mut f64 {
    match field {
        "gap" => &mut params.gap,
        "hole_dia" => &mut params.hole_dia,
        "hole_inset" => &mut params.hole_inset,
        "slot_width" => &mut params.slot_width,
        "slot_length" => &mut params.slot_length,
        "slot_offset" => &mut params.slot_offset,
        "slot_pitch" => &mut params.slot_pitch,
        "slot_web" => &mut params.slot_web,
        "pin_dia" => &mut params.pin_dia,
        "marker_size" => &mut params.marker_size,
        "dot_dia" => &mut params.dot_dia,
        "dot_pitch" => &mut params.dot_pitch,
        "dot_line_gap" => &mut params.dot_line_gap,
        "dot_clearance" => &mut params.dot_clearance,
        "hole_grid" => &mut params.hole_grid,
        other => unreachable!("unknown layout float {other}"),
    }
}

fn read_layout(
    config: &mut Config,
    key: &str,
    value: &str,
    where_: &str,
    warn: &mut dyn FnMut(&str),
    seen: &mut Vec<String>,
) {
    let key = LAYOUT_ALIASES
        .iter()
        .find(|(from, _)| *from == key)
        .map(|(_, to)| *to)
        .unwrap_or(key);
    if !seen.iter().any(|k| k == key) {
        seen.push(key.to_string());
    }
    let params = &mut config.layout;

    if key == "datum" {
        let text = value.to_lowercase();
        if !DATUM_MODES.contains(&text.as_str()) {
            warn(&format!(
                "{where_}: unknown datum {} (use {}), using {}",
                py_repr(value),
                DATUM_MODES.join(" | "),
                params.datum
            ));
            return;
        }
        params.datum = text;
        return;
    }
    if key == "holes" {
        // Older files only had this on/off switch; keep their behaviour
        // unless the same file also names a datum (which wins either way).
        let Some(flag) = parse_bool(value) else {
            warn(&format!(
                "{where_}: holes = {} is not on/off, using datum {}",
                py_repr(value),
                params.datum
            ));
            return;
        };
        if !seen.iter().any(|k| k == "datum") {
            params.datum = if flag {
                DATUM_HOLES.to_string()
            } else {
                DATUM_NONE.to_string()
            };
        }
        return;
    }
    if let Some((_, field, check)) = LAYOUT_FLOATS.iter().find(|(k, _, _)| *k == key) {
        let current = *layout_float(params, field);
        let Some(number) = py_parse_float(value) else {
            warn(&format!(
                "{where_}: {key} = {} is not a number, using {}",
                py_repr(value),
                py_float(current)
            ));
            return;
        };
        if number.is_nan() || number.is_infinite() {
            warn(&format!(
                "{where_}: {key} = {} is not a finite number, using {}",
                py_repr(value),
                py_float(current)
            ));
            return;
        }
        if !check.accepts(number) {
            warn(&format!(
                "{where_}: {key} {}, using {}",
                check.requirement(),
                py_float(current)
            ));
            return;
        }
        *layout_float(params, field) = number;
        return;
    }
    if key == "outer_border" || key == "marker" {
        let current = if key == "marker" {
            params.marker
        } else {
            params.outer_border
        };
        let Some(flag) = parse_bool(value) else {
            warn(&format!(
                "{where_}: {key} = {} is not on/off, using {}",
                py_repr(value),
                bool_text(current)
            ));
            return;
        };
        if key == "marker" {
            params.marker = flag;
        } else {
            params.outer_border = flag;
        }
        return;
    }
    if OBSOLETE_LAYOUT_KEYS.contains(&key) {
        return; // known, but nothing uses it any more
    }
    if key == "sort" {
        let text = value.to_lowercase();
        if !SORT_ORDERS.contains(&text.as_str()) {
            warn(&format!(
                "{where_}: unknown sort order {} (use {}), using {}",
                py_repr(value),
                SORT_ORDERS.join(" | "),
                params.sort
            ));
            return;
        }
        params.sort = text;
        return;
    }
    warn(&format!(
        "{where_}: unknown [layout] key {}, ignored",
        py_repr(key)
    ));
}

fn read_rule(
    config: &mut Config,
    key: &str,
    value: &str,
    where_: &str,
    warn: &mut dyn FnMut(&str),
) {
    if key == "ignore_prefixes" {
        config.ignore_prefixes = parse_prefixes(value);
    } else {
        warn(&format!(
            "{where_}: unknown [{SECTION_RULES}] key {}, ignored",
            py_repr(key)
        ));
    }
}

fn read_side(
    config: &mut Config,
    key: &str,
    value: &str,
    where_: &str,
    warn: &mut dyn FnMut(&str),
) {
    let flag = match parse_bool(value) {
        Some(flag) => flag,
        None => {
            warn(&format!(
                "{where_}: {key} = {} is not on/off, using on",
                py_repr(value)
            ));
            true
        }
    };
    config.sides.insert(key.to_string(), flag);
}

fn read_pad(config: &mut Config, key: &str, value: &str, where_: &str, warn: &mut dyn FnMut(&str)) {
    let mut state = value.to_lowercase();
    if !STATES.contains(&state.as_str()) {
        warn(&format!(
            "{where_}: unknown state {} for {key}, using {STATE_UNDEFINED}",
            py_repr(value)
        ));
        state = STATE_UNDEFINED.to_string();
    }
    config.pads.insert(key.to_string(), state);
}

// --------------------------------------------------------------------------- //
// Config <-> projects
// --------------------------------------------------------------------------- //

/// side.enabled and pad.state from the config dicts (defaults when absent).
pub fn apply_config(config: &Config, projects: &mut [Project]) {
    for project in projects.iter_mut() {
        for side in project.sides.iter_mut() {
            let key = side.key();
            side.enabled = config.sides.get(&key).copied().unwrap_or(true);
            for pad in side.pads.iter_mut() {
                let stored = config.pads.get(&pad.key).map(String::as_str);
                if pad.is_candidate() {
                    pad.state = match stored {
                        Some(state) if STATES.contains(&state) => state.to_string(),
                        _ => default_state(pad, &config.ignore_prefixes).to_string(),
                    };
                } else {
                    pad.state = match stored {
                        Some(state) if PASTED_STATES.contains(&state) => state.to_string(),
                        _ => STATE_OPEN.to_string(),
                    };
                }
            }
        }
    }
}

/// The reverse: every relevant side and every candidate/closed pad into the config dicts.
pub fn collect_config(config: &mut Config, projects: &[Project]) {
    for project in projects {
        for side in &project.sides {
            if side.is_relevant() {
                config.sides.insert(side.key(), side.enabled);
            }
            for pad in &side.pads {
                if pad.is_candidate() {
                    config.pads.insert(pad.key.clone(), pad.state.clone());
                } else if pad.is_closed() {
                    config
                        .pads
                        .insert(pad.key.clone(), STATE_IGNORE.to_string());
                } else {
                    config.pads.remove(&pad.key);
                }
            }
        }
    }
}

// --------------------------------------------------------------------------- //
// Writing
// --------------------------------------------------------------------------- //

fn side_comment(side: &crate::model::Side) -> String {
    format!(
        "{:.1} x {:.1} mm, {}, {} to decide",
        side.board_width(),
        side.board_height(),
        plural(side.paste_objects().len(), "paste opening"),
        plural(side.candidates().count(), "pad")
    )
}

fn pad_comment(pad: &crate::model::Pad) -> String {
    [pad.function.as_str(), pad.shape.as_str()]
        .iter()
        .filter(|p| !p.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Project indices in natural name order (stable, like Python's `sorted`).
fn ordered_projects(projects: &[Project]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..projects.len()).collect();
    order.sort_by_key(|i| natural_key(&projects[*i].name));
    order
}

/// Candidate pad indices of one side, ordered ref / pin / x / y.
fn ordered_candidates(side: &crate::model::Side) -> Vec<usize> {
    let mut order: Vec<usize> = side
        .pads
        .iter()
        .enumerate()
        .filter(|(_, p)| p.is_candidate())
        .map(|(i, _)| i)
        .collect();
    order.sort_by(|a, b| {
        let (pa, pb) = (&side.pads[*a], &side.pads[*b]);
        natural_key(&pa.ref_)
            .cmp(&natural_key(&pb.ref_))
            .then_with(|| natural_key(&pa.pin).cmp(&natural_key(&pb.pin)))
            .then_with(|| pa.x.total_cmp(&pb.x))
            .then_with(|| pa.y.total_cmp(&pb.y))
    });
    order
}

/// The file text.
pub fn format_config(config: &Config, projects: &[Project]) -> String {
    let stamp = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();
    let params = &config.layout;
    let sizes: Vec<String> = STENCIL_SIZES.iter().map(|s| size_label(*s)).collect();

    let mut lines: Vec<String> = vec![
        format!("# stencicrity configuration and pad decisions (generated {stamp})."),
        "# Edit by hand or through the TUI (run stencicrity in this folder).".to_string(),
        String::new(),
        format!("[{SECTION_STENCIL}]"),
        entry("size", &config.size_label(), &sizes.join(" | ")),
        entry("orientation", &config.orientation, "landscape (long side horizontal) | portrait"),
        String::new(),
        format!("[{SECTION_LAYOUT}]"),
        entry("spacing", &num_text(params.gap), "mm between neighbouring boards; the dotted border runs in the middle"),
        entry(
            "datum",
            &params.datum,
            &format!("{} - alignment features cut into every cell", DATUM_MODES.join(" | ")),
        ),
        entry("hole_dia", &num_text(params.hole_dia), "mm (holes datum)"),
        entry(
            "hole_inset",
            &num_text(params.hole_inset),
            "mm from the dotted line to the hole edge (negative = onto the line)",
        ),
        entry("slot_width", &num_text(params.slot_width), "mm across the cell edge (slots datum)"),
        entry("slot_length", &num_text(params.slot_length), "mm along the cell edge (slots datum)"),
        entry(
            "slot_offset",
            &num_text(params.slot_offset),
            "mm from the cell edge to the slot's outer wall; may clip the dotted line, never the neighbour",
        ),
        entry(
            "slot_pitch",
            &num_text(params.slot_pitch),
            "mm, raster of the modular jig: slot centres along the bottom and left edges and pin centres lie on it; cells grow to multiples of it",
        ),
        entry(
            "slot_web",
            &num_text(params.slot_web),
            "mm of foil kept between a slot and the board; cells grow to hold it",
        ),
        entry("pin_dia", &num_text(params.pin_dia), "mm jig pin through a slot (the holes datum uses hole_dia)"),
        entry(
            "marker",
            bool_text(params.marker),
            "slots datum: cut an X at the raster point inside the datum corner so the piece's orientation can be read",
        ),
        entry("marker_size", &num_text(params.marker_size), "mm, stroke length of the X (stroke width = dot_dia)"),
        entry("dot_dia", &num_text(params.dot_dia), "mm"),
        entry("dot_pitch", &num_text(params.dot_pitch), "mm"),
        entry(
            "dot_line_gap",
            &num_text(params.dot_line_gap),
            "each cell's dotted line runs this/2 inside its edge; touching cells show two lines this far apart, cut between them (0 = on the edge)",
        ),
        entry(
            "dot_clearance",
            &num_text(params.dot_clearance),
            "mm, a divider dot is dropped when less metal than this would remain between it and a slot, hole or marker",
        ),
        entry("hole_grid", &num_text(params.hole_grid), "holes datum: hole centres snap to this grid, 0 = off"),
        entry("outer_border", bool_text(params.outer_border), "also dot the cell edges on the outer boundary of the block"),
        entry("sort", &params.sort, "height (tallest boards first) | name"),
        String::new(),
        format!("[{SECTION_RULES}]"),
        entry(
            "ignore_prefixes",
            &config.ignore_prefixes.join(" "),
            "references whose pads without paste default to ignore (prefix + digit)",
        ),
        String::new(),
        format!("[{SECTION_SIDES}]"),
        "# <project>/<side> = on | off".to_string(),
    ];

    let order = ordered_projects(projects);
    let mut known_sides: Vec<String> = Vec::new();
    for pi in &order {
        for side in &projects[*pi].sides {
            if !side.is_relevant() {
                continue;
            }
            let key = side.key();
            let flag = config.sides.get(&key).copied().unwrap_or(side.enabled);
            lines.push(entry(&key, bool_text(flag), &side_comment(side)));
            known_sides.push(key);
        }
    }
    let stale_sides: Vec<&String> = config
        .sides
        .keys()
        .filter(|k| !known_sides.contains(k))
        .collect();
    if !stale_sides.is_empty() {
        lines.push(STALE_HEADER.to_string());
        for key in stale_sides {
            lines.push(entry(key, bool_text(config.sides[key]), ""));
        }
    }

    lines.push(String::new());
    lines.push(format!("[{SECTION_PADS}]"));
    lines.push(format!(
        "# <project>/<side>/<REF>.<pin>@<x>,<y> = {}",
        STATES.join(" | ")
    ));

    let mut known_pads: Vec<String> = Vec::new();
    let mut closed: Vec<String> = Vec::new();
    for pi in &order {
        let project = &projects[*pi];
        for side in &project.sides {
            // Pads with paste are open by default: only the closed ones are
            // written, but none of them may end up in the stale block.
            for pad in side.pasted_pads() {
                known_pads.push(pad.key.clone());
                let state = config.pads.get(&pad.key).unwrap_or(&pad.state);
                if state == STATE_IGNORE {
                    closed.push(entry(&pad.key, STATE_IGNORE, &pad_comment(pad)));
                }
            }
            let candidates = ordered_candidates(side);
            if candidates.is_empty() {
                continue;
            }
            lines.push(format!("# --- {} / {} ---", project.name, side.name));
            for index in candidates {
                let pad = &side.pads[index];
                known_pads.push(pad.key.clone());
                let state = config.pads.get(&pad.key).unwrap_or(&pad.state);
                lines.push(entry(&pad.key, state, &pad_comment(pad)));
            }
        }
    }
    if !closed.is_empty() {
        lines.push(CLOSED_HEADER.to_string());
        lines.extend(closed);
    }
    let stale_pads: Vec<&String> = config
        .pads
        .keys()
        .filter(|k| !known_pads.contains(k))
        .collect();
    if !stale_pads.is_empty() {
        lines.push(STALE_HEADER.to_string());
        for key in stale_pads {
            lines.push(entry(key, &config.pads[key], ""));
        }
    }

    let mut text = lines.join("\n");
    text.push('\n');
    text
}

/// The process umask (Linux `/proc/self/status`), 0o022 when unknown.
fn current_umask() -> u32 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                line.strip_prefix("Umask:")
                    .and_then(|v| u32::from_str_radix(v.trim(), 8).ok())
            })
        })
        .unwrap_or(0o022)
}

/// Write `text` to `path` through a temporary file in the same directory.
fn atomic_write(path: &Path, text: &str) -> std::io::Result<()> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let directory: PathBuf = absolute
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&directory)?;

    // mkstemp: an exclusive 0600 file with a unique name in that directory.
    let mut tmp = PathBuf::new();
    let mut handle = None;
    for attempt in 0u32..1000 {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let candidate = directory.join(format!(
            ".stencicrity-{}{stamp:09}{attempt}.tmp",
            std::process::id()
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&candidate)
        {
            Ok(file) => {
                tmp = candidate;
                handle = Some(file);
                break;
            }
            Err(exc) if exc.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(exc) => return Err(exc),
        }
    }
    let Some(mut file) = handle else {
        return Err(std::io::Error::other("could not create a temporary file"));
    };

    let result = (|| -> std::io::Result<()> {
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        // mkstemp creates 0600 files; keep the existing mode or a normal umask mode.
        let mode = std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o777)
            .unwrap_or(0o666 & !current_umask());
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// collect_config + atomic write (temp file + rename, keeping an existing mode).
pub fn save_config(path: &Path, config: &mut Config, projects: &[Project]) -> std::io::Result<()> {
    collect_config(config, projects);
    atomic_write(path, &format_config(config, projects))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn silent() -> impl FnMut(&str) {
        |_: &str| {}
    }

    #[test]
    fn booleans() {
        for word in ["on", "TRUE", " yes ", "1"] {
            assert_eq!(parse_bool(word), Some(true), "{word}");
        }
        for word in ["off", "False", "NO", "0"] {
            assert_eq!(parse_bool(word), Some(false), "{word}");
        }
        assert_eq!(parse_bool("maybe"), None);
    }

    #[test]
    fn prefixes() {
        assert_eq!(parse_prefixes("nt, TP"), vec!["NT", "TP"]);
        assert_eq!(parse_prefixes("  NT   nt  "), vec!["NT"]);
        assert_eq!(parse_prefixes(""), Vec::<String>::new());
    }

    #[test]
    fn numbers() {
        assert_eq!(num_text(30.0), "30.0");
        assert_eq!(num_text(0.5), "0.5");
        assert_eq!(num_text(-1.25), "-1.25");
        assert_eq!(num_text(0.0), "0.0");
        assert_eq!(num_text(2.5), "2.5");
        assert_eq!(py_float(30.0), "30.0");
        assert_eq!(py_float(0.5), "0.5");
    }

    #[test]
    fn comment_column() {
        assert_eq!(
            entry("size", "380x280", "a"),
            "size = 380x280            # a"
        );
        // a long key keeps at least three spaces
        assert_eq!(
            entry("a-very-long-key-indeed", "value", "c"),
            "a-very-long-key-indeed = value   # c"
        );
        assert_eq!(entry("k", "v", ""), "k = v");
    }

    #[test]
    fn legacy_flat_file() {
        let text = "RBARF/top/TP1.1@1.000,2.000 = ignore\nRBARF/top/R1.1@3.000,4.000 = open\n";
        let config = parse_config(text, &mut silent());
        assert_eq!(
            config
                .pads
                .get("RBARF/top/TP1.1@1.000,2.000")
                .map(String::as_str),
            Some("ignore")
        );
        assert_eq!(
            config
                .pads
                .get("RBARF/top/R1.1@3.000,4.000")
                .map(String::as_str),
            Some("open")
        );
    }

    #[test]
    fn legacy_holes_switch() {
        let on = parse_config("[layout]\nholes = on\n", &mut silent());
        assert_eq!(on.layout.datum, crate::model::DATUM_HOLES);
        let off = parse_config("[layout]\nholes = off\n", &mut silent());
        assert_eq!(off.layout.datum, DATUM_NONE);
        // an explicit datum wins, whichever order
        let before = parse_config("[layout]\ndatum = slots\nholes = on\n", &mut silent());
        assert_eq!(before.layout.datum, crate::model::DATUM_SLOTS);
        let after = parse_config("[layout]\nholes = on\ndatum = none\n", &mut silent());
        assert_eq!(after.layout.datum, DATUM_NONE);
    }

    #[test]
    fn aliases_and_obsolete_keys() {
        let mut warnings = Vec::new();
        let config = parse_config(
            "[layout]\ngap = 12\nborder = on\nslot_corner = 3\n",
            &mut |m: &str| warnings.push(m.to_string()),
        );
        assert_eq!(config.layout.gap, 12.0);
        assert!(config.layout.outer_border);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn last_equals_wins_and_comments() {
        let config = parse_config(
            "[pads]\na/b/C.1@0.000,0.000 = open   # trailing\n",
            &mut silent(),
        );
        assert_eq!(
            config.pads.get("a/b/C.1@0.000,0.000").map(String::as_str),
            Some("open")
        );
    }

    #[test]
    fn bad_values_warn_and_keep_defaults() {
        let defaults = Config::default();
        let mut warnings = Vec::new();
        let text = "[stencil]\nsize = 123x456\norientation = sideways\n\
                    [layout]\nspacing = -1\nslot_pitch = zero\ndot_dia = 0\n\
                    datum = spikes\nsort = colour\nmarker = perhaps\nnonsense = 1\n\
                    [rules]\nwhatever = x\n[sides]\nP/top = perhaps\n[pads]\nP/top/R1.1@0.000,0.000 = maybe\n";
        let config = parse_config(text, &mut |m: &str| warnings.push(m.to_string()));
        assert_eq!(config.size, defaults.size);
        assert_eq!(config.orientation, defaults.orientation);
        assert_eq!(config.layout.gap, defaults.layout.gap);
        assert_eq!(config.layout.slot_pitch, defaults.layout.slot_pitch);
        assert_eq!(config.layout.dot_dia, defaults.layout.dot_dia);
        assert_eq!(config.layout.datum, defaults.layout.datum);
        assert_eq!(config.layout.sort, defaults.layout.sort);
        assert_eq!(config.layout.marker, defaults.layout.marker);
        assert_eq!(config.sides.get("P/top"), Some(&true));
        assert_eq!(
            config
                .pads
                .get("P/top/R1.1@0.000,0.000")
                .map(String::as_str),
            Some(STATE_UNDEFINED)
        );
        assert_eq!(warnings.len(), 12, "{warnings:#?}");
        assert!(
            warnings[0].contains("unknown stencil size '123x456'"),
            "{}",
            warnings[0]
        );
        assert!(
            warnings[2].contains("spacing must not be negative"),
            "{}",
            warnings[2]
        );
        assert!(
            warnings[3].contains("slot_pitch = 'zero' is not a number"),
            "{}",
            warnings[3]
        );
    }

    #[test]
    fn unknown_section_warns() {
        let mut warnings = Vec::new();
        parse_config("[nope]\nkey = value\n", &mut |m: &str| {
            warnings.push(m.to_string())
        });
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("unknown section [nope]"));
    }

    #[test]
    fn round_trip_of_the_scalar_settings() {
        let mut config = Config {
            size: (450, 350),
            orientation: "portrait".to_string(),
            ..Config::default()
        };
        config.layout.gap = 12.5;
        config.layout.datum = DATUM_NONE.to_string();
        config.layout.dot_clearance = 0.75;
        config.layout.marker = false;
        config.layout.outer_border = true;
        config.layout.sort = crate::model::SORT_NAME.to_string();
        config.ignore_prefixes = vec!["NT".to_string(), "TP".to_string(), "FID".to_string()];
        let text = format_config(&config, &[]);
        let back = parse_config(&text, &mut silent());
        assert_eq!(back, config);
    }

    #[test]
    fn stale_entries_are_kept() {
        let mut config = Config::default();
        config.sides.insert("gone/top".to_string(), false);
        config.pads.insert(
            "gone/top/R1.1@0.000,0.000".to_string(),
            STATE_IGNORE.to_string(),
        );
        let text = format_config(&config, &[]);
        assert!(text.contains(STALE_HEADER));
        assert!(text.contains("gone/top = off"));
        assert!(text.contains("gone/top/R1.1@0.000,0.000 = ignore"));
        let back = parse_config(&text, &mut silent());
        assert_eq!(back.sides, config.sides);
        assert_eq!(back.pads, config.pads);
    }
}
