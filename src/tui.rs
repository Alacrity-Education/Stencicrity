//! Terminal UI (ratatui + crossterm): pages Pads / Sides / Stencil / Layout.
//!
//! Pages (tabs in the header, switched with `1`-`4` or Tab / Shift-Tab):
//!
//! 1. **Pads** - every candidate pad (copper without paste) of the *enabled*
//!    sides; each one is marked `open` (cut an opening), `ignore` (leave it
//!    closed) or left `undefined` (treated as closed). `*` toggles the view to
//!    *all* pads: the pads that already have a paste opening are shown dimmed
//!    and can be set to `ignore`, which removes their opening from the stencil
//!    (they are `open` by default and never `undefined`).
//! 2. **Sides** - which project sides get a cell on the stencil.
//! 3. **Stencil** - the orderable sheet sizes and the orientation, with a
//!    fits / does not fit verdict for every preset.
//! 4. **Layout** - the layout parameters: gap, the `datum` (which alignment
//!    features every cell gets: `slots`, `holes` or `none`), the numbers of
//!    both datums, divider dots, the gap between the two dotted lines and the
//!    clearance that drops a dot next to a slot, outer border, cell order. The
//!    rows of the datum that is not selected are drawn dimmed but stay
//!    editable.
//!
//! On the two list pages (Pads and Sides) `/` starts a vim like search: every
//! printable character is appended to the query and the list is filtered while
//! you type (a row survives when any whitespace separated word of the query is
//! a substring of any of its fields). The matches are ordered by how many
//! distinct words of the query they match - the best matches first - and a row
//! that matches *every* word is drawn bold. Enter leaves the search but *keeps*
//! the filter; Esc leaves the search and clears the filter at once. Either way
//! the cursor stays on the row it was on.
//!
//! `p` renders the preview PNG and opens it in the image viewer. `v` toggles
//! the *split view*: a vertical line divides the terminal, the four pages keep
//! the left half and the right half shows a terminal rendering of the sheet
//! around the pad the Pads page cursor is on (see [`crate::ascii`]). The split
//! wants a 150 column terminal - 90 for the app, one for the line, the rest
//! for the picture; below that the right half only asks for a wider terminal
//! ([`SPLIT_PROMPT`]) and the app keeps at least [`SPLIT_LEFT_MIN`] columns.
//!
//! `w` generates on every page - twice when the layout does not fit - and `q`
//! quits. Esc never leaves the program: it cancels an inline edit, then a
//! pending generate confirmation, then an active filter, and does nothing when
//! there is none of those. `g` / `G` are the first / last row on every page.
//!
//! Everything above [`App`] is pure and unit testable: state cycling, row
//! formatting, field stepping/validation, the search matching/filtering, the
//! footer key-help wrapping and the fit line do not touch a terminal. [`App`]
//! only draws ([`App::draw`]) and dispatches keys ([`App::handle_key`]);
//! [`run_tui`] owns the terminal loop and nothing else.
//!
//! The live objects are mutated in place: `pad.state`, `side.enabled`,
//! `config.size`, `config.orientation` and `config.layout.*`. The
//! `config.sides` / `config.pads` maps are left alone - the caller collects
//! them from the objects.

use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::Frame;

use crate::ascii::{self, Class, GeomCache, Raster};
use crate::model::{
    Config, Layout, LayoutParams, Pad, Project, Side, SideId, DATUM_HOLES, DATUM_MODES,
    DATUM_SLOTS, ORIENTATIONS, ORIENTATION_PORTRAIT, SORT_ORDERS, STATES, STATE_IGNORE, STATE_OPEN,
    STATE_UNDEFINED, STENCIL_SIZES,
};
use crate::util::fmt_mm;

/// Callbacks the TUI needs from the caller.
pub struct TuiHooks<'a> {
    /// Re-render the preview with the live state; `selected` pad highlighted; open the viewer when asked. Returns the PNG path.
    #[allow(clippy::type_complexity)]
    pub on_preview: &'a mut dyn FnMut(&[Project], &Config, Option<(SideId, usize)>, bool) -> String,
    /// Layout for the live state (or for an alternative config).
    pub compute_layout: &'a mut dyn FnMut(&[Project], &Config) -> Layout,
}

/// A pad: which side it belongs to and its index in `side.pads`.
pub type PadId = (SideId, usize);

/// Not a pad state: the counts line calls a pasted pad set to "ignore" closed.
pub const STATE_CLOSED: &str = "closed";
/// In front of a pad that already has paste.
pub const PASTE_MARK: &str = "· ";

pub const PAGE_PADS: usize = 0;
pub const PAGE_SIDES: usize = 1;
pub const PAGE_STENCIL: usize = 2;
pub const PAGE_LAYOUT: usize = 3;
pub const PAGE_NAMES: [&str; 4] = ["Pads", "Sides", "Stencil", "Layout"];

const COL_LABEL: usize = 12;
const COL_PROJECT: usize = 20;
const COL_SIDE: usize = 7;
const COL_FUNCTION: usize = 13;
const COL_SHAPE: usize = 16;
const STATE_WIDTH: usize = 9;
const COL_FIELD: usize = 38;
const COL_SIDE_NAME: usize = 30;

/// The footer help of every page as a list of "key description" items; they
/// are laid out on one or two lines by [`wrap_items`] so nothing is lost on a
/// narrow terminal.
pub const PAGE_ITEMS: [&[&str]; 4] = [
    &[
        "↑↓/jk move",
        "/ search",
        "space cycle",
        "o open",
        "i ignore",
        "a component",
        "n/N undef",
        "* all pads",
        "p preview",
        "v split view",
        "w generate",
        "1-4/tab page",
        "q quit",
    ],
    &[
        "↑↓ move",
        "/ search",
        "space/enter toggle side",
        "A all",
        "N none",
        "w generate",
        "p preview",
        "v split view",
        "1-4/tab page",
        "q quit",
    ],
    &[
        "↑↓ move",
        "space/enter pick size",
        "o orientation",
        "w generate",
        "p preview",
        "v split view",
        "1-4/tab page",
        "q quit",
    ],
    &[
        "↑↓ move",
        "+/- step",
        "space toggle",
        "e/enter edit",
        "w generate",
        "p preview",
        "v split view",
        "1-4/tab page",
        "q quit",
    ],
];
pub const EDIT_ITEMS: [&str; 4] = ["type a number", "backspace", "enter accept", "esc cancel"];
pub const SEARCH_ITEMS: [&str; 5] = [
    "type to filter",
    "backspace",
    "↑↓ move",
    "enter keep filter",
    "esc clear",
];
/// Added to the page help while a filter is active (Esc drops it again).
pub const FILTER_ITEM: &str = "esc clear filter";

/// Between two key-help items on one line.
pub const ITEM_SEP: &str = "  ";
/// The key help never grows past two lines.
pub const FOOTER_LINES: usize = 2;

pub const NOFIT_WARNING: &str =
    "layout does not fit the stencil — press w again to generate anyway";

// --------------------------------------------------------------------------- //
// Pure helpers: the split view
// --------------------------------------------------------------------------- //

/// The vertical line between the app and the terminal preview.
pub const SEPARATOR: &str = "│";
/// The whole right half below [`SPLIT_MIN_WIDTH`] columns.
pub const SPLIT_PROMPT: &str = "Terminal preview requires a larger terminal, please resize.";
/// The right half with no pad to draw.
pub const NO_PAD: &str = "no pad selected";
/// The one line legend under the picture.
pub const LEGEND: [(&str, Class); 4] = [
    ("red opening", Class::Opening),
    ("yellow undefined", Class::Undefined),
    ("blue ignored", Class::Ignored),
    ("cyan selected", Class::Selected),
];
/// From here up the right half shows the picture; below it only [`SPLIT_PROMPT`].
pub const SPLIT_MIN_WIDTH: u16 = 150;
/// Columns the app keeps at [`SPLIT_MIN_WIDTH`] and above.
pub const SPLIT_LEFT_WIDTH: u16 = 90;
/// The app never gets fewer columns than this; below that the prompt takes over.
pub const SPLIT_LEFT_MIN: u16 = 60;
/// The picture needs a caption; the legend wants this much height on top.
pub const LEGEND_MIN_HEIGHT: u16 = 5;

/// How the split view divides one terminal row.
///
/// `left` is the app (0 when even [`SPLIT_LEFT_MIN`] columns are impossible:
/// the prompt then takes the whole width), the separator sits at `left` and
/// the right half runs from `right_x` for `right_w` columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SplitPlan {
    pub left: u16,
    pub right_x: u16,
    pub right_w: u16,
    /// draw the picture (else only the resize prompt)
    pub preview: bool,
}

/// Divide a terminal `width` columns wide between the app and the preview.
///
/// At [`SPLIT_MIN_WIDTH`] and above the app gets [`SPLIT_LEFT_WIDTH`] columns,
/// one goes to the separator and the rest to the picture. Below that the right
/// half only carries [`SPLIT_PROMPT`], so it is given exactly that many columns
/// and the app keeps what is left; when that would leave the app fewer than
/// [`SPLIT_LEFT_MIN`] columns the prompt takes the whole width instead.
pub fn split_plan(width: u16) -> SplitPlan {
    if width >= SPLIT_MIN_WIDTH {
        return SplitPlan {
            left: SPLIT_LEFT_WIDTH,
            right_x: SPLIT_LEFT_WIDTH + 1,
            right_w: width - SPLIT_LEFT_WIDTH - 1,
            preview: true,
        };
    }
    let want = SPLIT_PROMPT.chars().count() as u16;
    if width >= SPLIT_LEFT_MIN + 1 + want {
        let left = width - 1 - want;
        return SplitPlan {
            left,
            right_x: left + 1,
            right_w: want,
            preview: false,
        };
    }
    SplitPlan {
        left: 0,
        right_x: 0,
        right_w: width,
        preview: false,
    }
}

/// Character rows the picture gets in a right half `height` rows tall: one
/// goes to the caption and, when there is room, one to the legend.
pub fn panel_rows(height: u16) -> u16 {
    height.saturating_sub(1 + u16::from(height >= LEGEND_MIN_HEIGHT))
}

// --------------------------------------------------------------------------- //
// Pure helpers: pad states
// --------------------------------------------------------------------------- //

/// The marker in front of a pad row.
pub fn state_marker(state: &str) -> &'static str {
    match state {
        STATE_OPEN => "+",
        STATE_IGNORE => "-",
        _ => "?",
    }
}

/// Next state for the space bar: undefined -> open -> ignore -> open ...
///
/// `undefined` is only ever a starting point; once a pad has been touched it
/// toggles between `open` and `ignore`.
pub fn cycle_state(state: &str) -> &'static str {
    if state == STATE_OPEN {
        STATE_IGNORE
    } else {
        STATE_OPEN
    }
}

/// The state the space bar gives `pad`.
///
/// Candidates cycle undefined -> open -> ignore -> open …; a pad that already
/// has a paste opening only toggles open <-> ignore and is never `undefined`
/// (`ignore` closes its opening).
pub fn next_state(pad: &Pad) -> &'static str {
    if pad.has_paste && pad.state != STATE_OPEN && pad.state != STATE_IGNORE {
        return STATE_OPEN;
    }
    cycle_state(&pad.state)
}

/// May `pad` be put in `state`? Pasted pads are open or ignore only.
pub fn allowed_state(pad: &Pad, state: &str) -> bool {
    if pad.has_paste {
        state == STATE_OPEN || state == STATE_IGNORE
    } else {
        STATES.contains(&state)
    }
}

/// Pads per state, candidates and pasted pads apart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StateCounts {
    pub undefined: usize,
    pub open: usize,
    pub ignore: usize,
    /// pads that have a paste opening the user removed (state `ignore`)
    pub closed: usize,
}

/// Count pads per state; see [`StateCounts`].
pub fn count_states<'a>(pads: impl IntoIterator<Item = &'a Pad>) -> StateCounts {
    let mut counts = StateCounts::default();
    for pad in pads {
        if pad.has_paste {
            if pad.is_closed() {
                counts.closed += 1;
            }
        } else {
            match pad.state.as_str() {
                STATE_UNDEFINED => counts.undefined += 1,
                STATE_OPEN => counts.open += 1,
                STATE_IGNORE => counts.ignore += 1,
                _ => {}
            }
        }
    }
    counts
}

/// The pad behind an id.
pub fn pad_of(projects: &[Project], id: PadId) -> &Pad {
    &projects[id.0.project].sides[id.0.side].pads[id.1]
}

/// The pad behind an id, mutable.
pub fn pad_of_mut(projects: &mut [Project], id: PadId) -> &mut Pad {
    &mut projects[id.0.project].sides[id.0.side].pads[id.1]
}

/// Every pad of `rows` that belongs to the same component as `pad`
/// (same project, side and reference).
pub fn component_pads(projects: &[Project], rows: &[PadId], pad: PadId) -> Vec<PadId> {
    let it = pad_of(projects, pad);
    let (project, side, ref_) = (it.project.clone(), it.side.clone(), it.ref_.clone());
    rows.iter()
        .copied()
        .filter(|id| {
            let p = pad_of(projects, *id);
            p.project == project && p.side == side && p.ref_ == ref_
        })
        .collect()
}

/// Set `state` (default: the pad's own state) on the whole component.
///
/// Pads that may not take the state are skipped (a pad with paste is never
/// `undefined`). Returns the number of pads changed, the cursor pad included.
pub fn apply_component(
    projects: &mut [Project],
    rows: &[PadId],
    pad: PadId,
    state: Option<&str>,
) -> usize {
    let target = match state {
        Some(s) => s.to_string(),
        None => pad_of(projects, pad).state.clone(),
    };
    let members = component_pads(projects, rows, pad);
    let mut changed = 0;
    for id in members {
        let other = pad_of_mut(projects, id);
        if !allowed_state(other, &target) {
            continue;
        }
        other.state = target.clone();
        changed += 1;
    }
    changed
}

/// Index of the next/previous undefined *candidate*, wrapping around `start`.
///
/// Pads that already have paste are never undefined, so `n`/`N` walk the
/// candidates only, even in the all-pads view.
pub fn find_undefined(
    projects: &[Project],
    rows: &[PadId],
    start: usize,
    forward: bool,
) -> Option<usize> {
    let total = rows.len();
    if total == 0 {
        return None;
    }
    let step: isize = if forward { 1 } else { -1 };
    for offset in 1..=total {
        let index = (start as isize + step * offset as isize).rem_euclid(total as isize) as usize;
        let pad = pad_of(projects, rows[index]);
        if pad.is_candidate() && pad.state == STATE_UNDEFINED {
            return Some(index);
        }
    }
    None
}

/// The pads of the enabled sides the Pads page shows (order preserved).
///
/// By default only the candidates (pads without a paste opening) are listed;
/// with `show_all` the pads that already have paste are listed too, so the user
/// can close their opening. With no sides at all nothing can be filtered by
/// side, so every pad passes that step.
pub fn visible_pads(
    projects: &[Project],
    pads: &[PadId],
    sides: &[SideId],
    show_all: bool,
) -> Vec<PadId> {
    let rows: Vec<PadId> = if sides.is_empty() {
        pads.to_vec()
    } else {
        let enabled: Vec<SideId> = sides
            .iter()
            .copied()
            .filter(|s| s.get(projects).enabled)
            .collect();
        pads.iter()
            .copied()
            .filter(|(side, _)| enabled.contains(side))
            .collect()
    };
    if show_all {
        return rows;
    }
    rows.into_iter()
        .filter(|id| pad_of(projects, *id).is_candidate())
        .collect()
}

// --------------------------------------------------------------------------- //
// Pure helpers: text
// --------------------------------------------------------------------------- //

/// Truncate `text` so it always fits in `width` columns.
pub fn clip(text: &str, width: usize) -> String {
    text.chars().take(width).collect()
}

/// Pad or truncate `text` to exactly `width` characters.
fn col(text: &str, width: usize) -> String {
    let n = text.chars().count();
    if n > width {
        let mut out: String = text.chars().take(width.saturating_sub(1)).collect();
        out.push('…');
        return out;
    }
    let mut out = text.to_string();
    out.extend(std::iter::repeat_n(' ', width - n));
    out
}

/// `text` cut to `width` columns, marking the cut with `…`.
pub fn ellipsis(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if text.chars().count() <= width {
        return text.to_string();
    }
    if width == 1 {
        return "…".to_string();
    }
    let mut out: String = text.chars().take(width - 1).collect();
    out.push('…');
    out
}

/// Greedy line breaking of `items` joined by `sep`; at most `max_lines`.
fn wrap(items: &[String], width: usize, max_lines: usize, sep: &str) -> Vec<String> {
    let items: Vec<&str> = items
        .iter()
        .map(String::as_str)
        .filter(|s| !s.is_empty())
        .collect();
    if items.is_empty() || width == 0 || max_lines == 0 {
        return Vec::new();
    }
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut index = 0usize;
    while index < items.len() && lines.len() < max_lines {
        let item = items[index];
        let candidate = if current.is_empty() {
            item.to_string()
        } else {
            format!("{current}{sep}{item}")
        };
        if candidate.chars().count() <= width {
            current = candidate;
            index += 1;
            continue;
        }
        if current.is_empty() {
            // a single item wider than one line
            current = item.to_string();
            index += 1;
        }
        lines.push(std::mem::take(&mut current));
    }
    if !current.is_empty() && lines.len() < max_lines {
        lines.push(std::mem::take(&mut current));
    }
    let left = index < items.len() || !current.is_empty();
    let mut lines: Vec<String> = lines.iter().map(|l| ellipsis(l, width)).collect();
    if left && !lines.is_empty() {
        // say that something was dropped
        let last = lines.len() - 1;
        lines[last] = ellipsis(&format!("{}{sep}…", lines[last]), width);
    }
    lines
}

/// Lay `key description` items out on at most `max_lines` lines.
///
/// Items are separated by two spaces and never split: as many as fit go on a
/// line, the rest start the next one. When even `max_lines` lines are not
/// enough the last one is truncated with `…`, so the result is always at most
/// `max_lines` lines of at most `width` columns.
pub fn wrap_items(items: &[String], width: usize, max_lines: usize) -> Vec<String> {
    wrap(items, width, max_lines, ITEM_SEP)
}

/// Same as [`wrap_items`] for a sentence (status messages).
pub fn wrap_text(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let words: Vec<String> = text.split_whitespace().map(str::to_string).collect();
    wrap(&words, width, max_lines, " ")
}

/// The footer key help of `page`; `esc clear filter` when one is active.
///
/// The item is put right behind `/ search` (the only pages that can be filtered
/// are the ones that offer the search), so the two filter keys sit next to each
/// other.
pub fn page_items(page: usize, filtered: bool) -> Vec<String> {
    let mut items: Vec<String> = PAGE_ITEMS[page].iter().map(|s| s.to_string()).collect();
    if !filtered {
        return items;
    }
    match items.iter().position(|it| it.starts_with("/ ")) {
        Some(index) => items.insert(index + 1, FILTER_ITEM.to_string()),
        None => items.push(FILTER_ITEM.to_string()),
    }
    items
}

// --------------------------------------------------------------------------- //
// Pure helpers: search
// --------------------------------------------------------------------------- //

/// The distinct lower case words of `query`, in the order they appear.
///
/// The query is split on whitespace and a word typed twice only counts once, so
/// `"u1 u1"` is the very same one word query as `"u1"`.
pub fn query_words(query: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in query.to_lowercase().split_whitespace() {
        if !out.iter().any(|w| w == word) {
            out.push(word.to_string());
        }
    }
    out
}

/// How many distinct words of `query` match these `fields`?
///
/// A word matches when it is a case-insensitive substring of *any* field. The
/// words are OR-ed for the filter (a row is kept as soon as the count is >= 1)
/// and the count itself orders the result: the rows that match every word - the
/// *full* matches - come first. A blank query has no words at all, so it counts
/// 0 for every row and nothing is a full match.
pub fn match_count(fields: &[String], query: &str) -> usize {
    let words = query_words(query);
    if words.is_empty() {
        return 0;
    }
    let haystack: Vec<String> = fields
        .iter()
        .filter(|f| !f.is_empty())
        .map(|f| f.to_lowercase())
        .collect();
    words
        .iter()
        .filter(|word| haystack.iter().any(|text| text.contains(word.as_str())))
        .count()
}

/// Does `query` match a row with these `fields` (count >= 1)? A blank query
/// matches everything.
pub fn matches(fields: &[String], query: &str) -> bool {
    if query_words(query).is_empty() {
        return true;
    }
    match_count(fields, query) >= 1
}

/// The matching `(row, match count)` pairs, the best matches first.
///
/// The pairs are sorted by the number of matched words, descending; rows with
/// the same count keep their original order (the sort is stable). A blank query
/// keeps every row, in order, with a count of 0.
pub fn filter_counts<T: Copy>(
    rows: &[T],
    fields_of: impl Fn(&T) -> Vec<String>,
    query: &str,
) -> Vec<(T, usize)> {
    if query_words(query).is_empty() {
        return rows.iter().map(|row| (*row, 0)).collect();
    }
    let mut pairs: Vec<(T, usize)> = rows
        .iter()
        .filter_map(|row| {
            let count = match_count(&fields_of(row), query);
            (count > 0).then_some((*row, count))
        })
        .collect();
    pairs.sort_by_key(|pair| std::cmp::Reverse(pair.1));
    pairs
}

/// The rows matching `query`, best (most words matched) first.
pub fn filter_rows<T: Copy>(
    rows: &[T],
    fields_of: impl Fn(&T) -> Vec<String>,
    query: &str,
) -> Vec<T> {
    filter_counts(rows, fields_of, query)
        .into_iter()
        .map(|(row, _)| row)
        .collect()
}

/// How many of `counts` matched *every* distinct word of `query`.
pub fn full_matches(counts: &[usize], query: &str) -> usize {
    let words = query_words(query).len();
    if words == 0 {
        return 0;
    }
    counts.iter().filter(|c| **c >= words).count()
}

/// The `(n of N, k full)` tail of the search sub-header.
///
/// `, k full` is left out for a one word query, where every match is a full
/// match anyway, and for the blank query that filters nothing.
pub fn match_summary(shown: usize, total: usize, full: usize, words: usize) -> String {
    if shown == 0 {
        return "no match".to_string();
    }
    if words > 1 {
        return format!("({shown} of {total}, {full} full)");
    }
    format!("({shown} of {total})")
}

/// The character `key` adds to a query, or None when it adds nothing.
///
/// Only printable ASCII (space included) is accepted, so control keys and
/// everything crossterm reports as a special key stay out of the query and can
/// still navigate.
pub fn search_char(key: KeyEvent) -> Option<char> {
    if key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.contains(KeyModifiers::ALT) {
        return None;
    }
    match key.code {
        KeyCode::Char(c) if (' '..'\u{7f}').contains(&c) => Some(c),
        _ => None,
    }
}

/// What one key does in search mode (see [`search_action`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchAction {
    /// Enter: leave the search, keep the filter
    Keep,
    /// Esc: leave the search and drop the filter
    Clear,
    Backspace,
    Up,
    Down,
    PageUp,
    PageDown,
    First,
    Last,
    /// a printable character, into the query
    Char,
    Ignore,
}

/// What `key` does while the search is open.
pub fn search_action(key: KeyEvent) -> SearchAction {
    match key.code {
        KeyCode::Enter => return SearchAction::Keep,
        KeyCode::Esc => return SearchAction::Clear,
        KeyCode::Backspace => return SearchAction::Backspace,
        KeyCode::Up => return SearchAction::Up,
        KeyCode::Down => return SearchAction::Down,
        KeyCode::PageUp => return SearchAction::PageUp,
        KeyCode::PageDown => return SearchAction::PageDown,
        KeyCode::Home => return SearchAction::First,
        KeyCode::End => return SearchAction::Last,
        _ => {}
    }
    if search_char(key).is_some() {
        return SearchAction::Char;
    }
    SearchAction::Ignore
}

/// What Esc does outside the search (see [`escape_action`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscAction {
    /// cancel the inline edit
    Edit,
    /// leave the search, clearing the filter
    Search,
    /// cancel a pending generate confirmation
    Pending,
    /// clear the filter that is still active
    Filter,
    /// nothing at all - Esc never quits
    None,
}

/// What Esc does, in priority order - and it never quits the program.
///
/// An inline edit is cancelled first, then the search (which drops its filter),
/// then a pending "press w again" confirmation, then the filter that a finished
/// search left on the list. With none of those Esc does nothing at all.
pub fn escape_action(editing: bool, searching: bool, pending: bool, filtered: bool) -> EscAction {
    if editing {
        return EscAction::Edit;
    }
    if searching {
        return EscAction::Search;
    }
    if pending {
        return EscAction::Pending;
    }
    if filtered {
        return EscAction::Filter;
    }
    EscAction::None
}

/// Where `item` sits in `rows`; `fallback` (clamped) when it is gone.
///
/// Identity based on purpose: the very same pad/side is followed from one list
/// to the other (filtered <-> unfiltered).
pub fn restore_index<T: PartialEq>(rows: &[T], item: Option<&T>, fallback: usize) -> usize {
    if let Some(it) = item {
        if let Some(index) = rows.iter().position(|row| row == it) {
            return index;
        }
    }
    fallback.min(rows.len().saturating_sub(1))
}

// --------------------------------------------------------------------------- //
// Pure helpers: rows
// --------------------------------------------------------------------------- //

/// The coordinate text of a pad row, exactly as it is displayed.
pub fn pad_position(pad: &Pad) -> String {
    format!("x={:8.3} y={:8.3}", pad.x, pad.y)
}

/// The board size text of a side row, exactly as it is displayed.
pub fn board_text(side: &Side) -> String {
    format!(
        "{:7.1} x {:7.1} mm",
        side.board_width(),
        side.board_height()
    )
}

/// Everything of a pad the search looks at.
///
/// A pad that already has a paste opening also answers to `paste` (and to
/// `closed` once its opening was removed).
pub fn pad_fields(pad: &Pad) -> Vec<String> {
    let extra = if pad.has_paste {
        if pad.is_closed() {
            "paste closed"
        } else {
            "paste"
        }
    } else {
        ""
    };
    vec![
        pad.label(),
        pad.ref_.clone(),
        pad.pin.clone(),
        pad.project.clone(),
        pad.side.clone(),
        pad.function.clone(),
        pad.shape.clone(),
        pad.state.clone(),
        pad_position(pad),
        extra.to_string(),
    ]
}

/// Everything of a side the search looks at.
pub fn side_fields(side: &Side) -> Vec<String> {
    vec![
        side.project_name.clone(),
        side.name.clone(),
        side.label(),
        if side.mirror { "mirrored" } else { "" }.to_string(),
        if side.enabled { "on" } else { "off" }.to_string(),
        board_text(side),
    ]
}

/// The kind of one chunk of a pad row (see [`row_segments`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegKind {
    Plain,
    /// the `+ open` column, coloured by state
    State,
    /// the REF.pin column, dimmed for a pad that already has paste
    PadLabel,
}

/// Pad row as `(text, kind)` chunks.
///
/// A pad with paste is also marked with a `·` in front of its reference, so a
/// terminal without colours can tell it apart.
pub fn row_segments(pad: &Pad, cursor: bool) -> Vec<(String, SegKind)> {
    let marker = state_marker(&pad.state);
    let state_field = format!("{marker} {:<w$}", pad.state, w = STATE_WIDTH);
    let mark = if pad.has_paste {
        PASTE_MARK.to_string()
    } else {
        " ".repeat(PASTE_MARK.chars().count())
    };
    let pad_field = format!("{mark}{}", col(&pad.label(), COL_LABEL));
    let rest = [
        col(&pad.project, COL_PROJECT),
        col(&pad.side, COL_SIDE),
        col(dash(&pad.function), COL_FUNCTION),
        col(dash(&pad.shape), COL_SHAPE),
        pad_position(pad),
    ]
    .join(" ");
    vec![
        (if cursor { "→ " } else { "  " }.to_string(), SegKind::Plain),
        (format!("{state_field} "), SegKind::State),
        (format!("{pad_field} "), SegKind::PadLabel),
        (rest, SegKind::Plain),
    ]
}

fn dash(text: &str) -> &str {
    if text.is_empty() {
        "-"
    } else {
        text
    }
}

/// One plain-text pad row (used by the view and by tests).
pub fn format_row(pad: &Pad, cursor: bool, width: Option<usize>) -> String {
    let line: String = row_segments(pad, cursor)
        .into_iter()
        .map(|(text, _)| text)
        .collect();
    match width {
        Some(w) => clip(&line, w),
        None => line,
    }
}

/// Column header of the pads page.
pub fn pads_header() -> String {
    let width = STATE_WIDTH + 2;
    let rest = [
        format!(
            "{}{}",
            " ".repeat(PASTE_MARK.chars().count()),
            col("pad", COL_LABEL)
        ),
        col("project", COL_PROJECT),
        col("side", COL_SIDE),
        col("function", COL_FUNCTION),
        col("shape", COL_SHAPE),
        "position".to_string(),
    ]
    .join(" ");
    format!("  {:<width$} {rest}", "state")
}

/// One row of the sides page.
pub fn side_row(side: &Side, cursor: bool) -> String {
    let mark = if side.enabled { "[x]" } else { "[ ]" };
    let mut name = format!("{} · {}", side.project_name, side.name);
    if side.mirror {
        name.push_str(" (mirrored)");
    }
    let candidates = side.candidates().count();
    let undefined = side
        .candidates()
        .filter(|pad| pad.state == STATE_UNDEFINED)
        .count();
    let board = board_text(side);
    format!(
        "{}{mark} {} {board}   paste {:<6} pads to decide {:<6} undefined {undefined}",
        if cursor { "→ " } else { "  " },
        col(&name, COL_SIDE_NAME),
        side.paste_objects().len(),
        candidates,
    )
}

/// Sheet (width, height) of `size` in `orientation` (size is long side first).
pub fn orientation_size(size: (u32, u32), orientation: &str) -> (u32, u32) {
    let (long, short) = (size.0.max(size.1), size.0.min(size.1));
    if orientation == ORIENTATION_PORTRAIT {
        (short, long)
    } else {
        (long, short)
    }
}

fn fit_word(fits: Option<bool>) -> &'static str {
    match fits {
        None => "?",
        Some(true) => "fits",
        Some(false) => "NO",
    }
}

/// One row of the stencil page: the preset and its fit in both orientations.
/// `fits` is indexed like [`crate::model::ORIENTATIONS`].
pub fn preset_row(
    size: (u32, u32),
    fits: [Option<bool>; 2],
    config: &Config,
    cursor: bool,
) -> String {
    let selected = config.size == size;
    let mark = if selected { "●" } else { " " };
    let (long, short) = (size.0.max(size.1), size.0.min(size.1));
    let (land_w, land_h) = orientation_size(size, ORIENTATIONS[0]);
    let (port_w, port_h) = orientation_size(size, ORIENTATIONS[1]);
    let land = format!("landscape: {land_w} x {land_h}  {}", fit_word(fits[0]));
    let port = format!("portrait: {port_w} x {port_h}  {}", fit_word(fits[1]));
    let current = if selected {
        format!("  ← {}", config.orientation)
    } else {
        String::new()
    };
    format!(
        "{}{mark} {long} x {short} mm   {land:<30}{port:<30}{current}",
        if cursor { "→ " } else { "  " }
    )
}

/// The kind of one chunk of a coloured line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Plain,
    Fits,
    NoFit,
    Header,
    Search,
}

/// The always-visible status line as `(text, kind)` chunks.
pub fn fit_segments(
    config: &Config,
    layout: Option<&Layout>,
    counts: StateCounts,
) -> Vec<(String, Kind)> {
    let mut parts = vec![(
        format!("stencil {} {}", config.size_label(), config.orientation),
        Kind::Plain,
    )];
    match layout {
        None => {
            parts.push((" · block ? · ".to_string(), Kind::Plain));
            parts.push(("NO LAYOUT".to_string(), Kind::NoFit));
        }
        Some(layout) => {
            parts.push((
                format!(
                    " · block {:.1} x {:.1} mm · ",
                    layout.block_width(),
                    layout.block_height()
                ),
                Kind::Plain,
            ));
            if layout.fits {
                parts.push(("FITS".to_string(), Kind::Fits));
            } else {
                parts.push(("DOES NOT FIT".to_string(), Kind::NoFit));
            }
        }
    }
    parts.push((
        format!(
            " · pads: undefined {}  open {}  ignore {}  closed {}",
            counts.undefined, counts.open, counts.ignore, counts.closed
        ),
        Kind::Plain,
    ));
    parts
}

/// Plain text of [`fit_segments`].
pub fn fit_line(config: &Config, layout: Option<&Layout>, counts: StateCounts) -> String {
    fit_segments(config, layout, counts)
        .into_iter()
        .map(|(text, _)| text)
        .collect()
}

// --------------------------------------------------------------------------- //
// Pure helpers: layout page fields
// --------------------------------------------------------------------------- //

/// Which [`LayoutParams`] member a layout row edits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attr {
    Gap,
    Datum,
    HoleDia,
    HoleInset,
    SlotWidth,
    SlotLength,
    SlotOffset,
    SlotPitch,
    SlotWeb,
    PinDia,
    Marker,
    MarkerSize,
    DotDia,
    DotPitch,
    DotLineGap,
    DotClearance,
    HoleGrid,
    OuterBorder,
    Sort,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Length,
    Bool,
    Choice,
}

/// Validity rule of a numeric field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    Any,
    Ge0,
    Gt0,
}

/// One editable row of the layout page.
#[derive(Clone, Copy, Debug)]
pub struct Field {
    pub attr: Attr,
    pub label: &'static str,
    pub kind: FieldKind,
    pub unit: &'static str,
    pub step: f64,
    /// clamp while stepping (None: unbounded)
    pub minimum: Option<f64>,
    pub rule: Rule,
    pub choices: &'static [&'static str],
}

impl Field {
    pub fn numeric(&self) -> bool {
        self.kind == FieldKind::Length
    }
}

static DATUM_CHOICES: [&str; 3] = DATUM_MODES;
static SORT_CHOICES: [&str; 2] = SORT_ORDERS;

const fn length(
    attr: Attr,
    label: &'static str,
    step: f64,
    minimum: Option<f64>,
    rule: Rule,
) -> Field {
    Field {
        attr,
        label,
        kind: FieldKind::Length,
        unit: "mm",
        step,
        minimum,
        rule,
        choices: &[],
    }
}

const fn flag_field(attr: Attr, label: &'static str) -> Field {
    Field {
        attr,
        label,
        kind: FieldKind::Bool,
        unit: "",
        step: 0.5,
        minimum: None,
        rule: Rule::Any,
        choices: &[],
    }
}

const fn choice_field(attr: Attr, label: &'static str, choices: &'static [&'static str]) -> Field {
    Field {
        attr,
        label,
        kind: FieldKind::Choice,
        unit: "",
        step: 0.5,
        minimum: None,
        rule: Rule::Any,
        choices,
    }
}

/// Every row of the layout page, in display order.
pub static LAYOUT_FIELDS: [Field; 19] = [
    length(
        Attr::Gap,
        "spacing (gap between boards)",
        0.5,
        Some(0.0),
        Rule::Ge0,
    ),
    choice_field(Attr::Datum, "datum (alignment features)", &DATUM_CHOICES),
    length(Attr::HoleDia, "hole diameter", 0.5, Some(0.1), Rule::Gt0),
    length(
        Attr::HoleInset,
        "hole inset (dotted line to hole edge)",
        0.5,
        None,
        Rule::Any,
    ),
    length(Attr::SlotWidth, "slot width", 0.5, Some(0.1), Rule::Gt0),
    length(Attr::SlotLength, "slot length", 0.5, Some(0.1), Rule::Gt0),
    length(
        Attr::SlotOffset,
        "slot offset (edge to outer wall)",
        0.5,
        Some(0.0),
        Rule::Ge0,
    ),
    length(
        Attr::SlotPitch,
        "slot pitch (modular jig raster)",
        5.0,
        Some(1.0),
        Rule::Gt0,
    ),
    length(
        Attr::SlotWeb,
        "slot web (to board)",
        0.5,
        Some(0.1),
        Rule::Gt0,
    ),
    length(Attr::PinDia, "pin diameter", 0.5, Some(0.1), Rule::Gt0),
    flag_field(Attr::Marker, "orientation marker"),
    length(Attr::MarkerSize, "marker size", 0.5, Some(0.5), Rule::Gt0),
    length(Attr::DotDia, "dot diameter", 0.5, Some(0.1), Rule::Gt0),
    length(Attr::DotPitch, "dot pitch", 0.5, Some(0.1), Rule::Gt0),
    length(
        Attr::DotLineGap,
        "dotted line gap (between touching cells)",
        0.5,
        Some(0.0),
        Rule::Ge0,
    ),
    length(
        Attr::DotClearance,
        "dot clearance (to slot/hole/marker)",
        0.1,
        Some(0.0),
        Rule::Ge0,
    ),
    length(
        Attr::HoleGrid,
        "hole grid (holes datum, 0 = off)",
        1.0,
        Some(0.0),
        Rule::Ge0,
    ),
    flag_field(Attr::OuterBorder, "outer border"),
    choice_field(Attr::Sort, "sort", &SORT_CHOICES),
];

/// The numeric value of `attr` (0 for non-numeric rows).
pub fn field_number(attr: Attr, params: &LayoutParams) -> f64 {
    match attr {
        Attr::Gap => params.gap,
        Attr::HoleDia => params.hole_dia,
        Attr::HoleInset => params.hole_inset,
        Attr::SlotWidth => params.slot_width,
        Attr::SlotLength => params.slot_length,
        Attr::SlotOffset => params.slot_offset,
        Attr::SlotPitch => params.slot_pitch,
        Attr::SlotWeb => params.slot_web,
        Attr::PinDia => params.pin_dia,
        Attr::MarkerSize => params.marker_size,
        Attr::DotDia => params.dot_dia,
        Attr::DotPitch => params.dot_pitch,
        Attr::DotLineGap => params.dot_line_gap,
        Attr::DotClearance => params.dot_clearance,
        Attr::HoleGrid => params.hole_grid,
        _ => 0.0,
    }
}

fn set_field_number(attr: Attr, params: &mut LayoutParams, value: f64) {
    match attr {
        Attr::Gap => params.gap = value,
        Attr::HoleDia => params.hole_dia = value,
        Attr::HoleInset => params.hole_inset = value,
        Attr::SlotWidth => params.slot_width = value,
        Attr::SlotLength => params.slot_length = value,
        Attr::SlotOffset => params.slot_offset = value,
        Attr::SlotPitch => params.slot_pitch = value,
        Attr::SlotWeb => params.slot_web = value,
        Attr::PinDia => params.pin_dia = value,
        Attr::MarkerSize => params.marker_size = value,
        Attr::DotDia => params.dot_dia = value,
        Attr::DotPitch => params.dot_pitch = value,
        Attr::DotLineGap => params.dot_line_gap = value,
        Attr::DotClearance => params.dot_clearance = value,
        Attr::HoleGrid => params.hole_grid = value,
        _ => {}
    }
}

/// The boolean value of `attr`.
pub fn field_flag(attr: Attr, params: &LayoutParams) -> bool {
    match attr {
        Attr::Marker => params.marker,
        Attr::OuterBorder => params.outer_border,
        _ => false,
    }
}

fn set_field_flag(attr: Attr, params: &mut LayoutParams, value: bool) {
    match attr {
        Attr::Marker => params.marker = value,
        Attr::OuterBorder => params.outer_border = value,
        _ => {}
    }
}

/// The choice value of `attr`.
pub fn field_choice(attr: Attr, params: &LayoutParams) -> &str {
    match attr {
        Attr::Datum => &params.datum,
        Attr::Sort => &params.sort,
        _ => "",
    }
}

fn set_field_choice(attr: Attr, params: &mut LayoutParams, value: &str) {
    match attr {
        Attr::Datum => params.datum = value.to_string(),
        Attr::Sort => params.sort = value.to_string(),
        _ => {}
    }
}

/// Rows that only mean something for one datum; they are dimmed under any other
/// one but stay editable, so a value can be set before switching over.
fn datum_row(attr: Attr) -> Option<&'static str> {
    match attr {
        Attr::HoleDia | Attr::HoleInset => Some(DATUM_HOLES),
        Attr::SlotWidth
        | Attr::SlotLength
        | Attr::SlotOffset
        | Attr::SlotPitch
        | Attr::SlotWeb
        | Attr::PinDia
        | Attr::Marker
        | Attr::MarkerSize => Some(DATUM_SLOTS),
        _ => None,
    }
}

/// Does this row matter for the datum the layout is currently set to?
///
/// A row that does not (the hole rows under `slots`, the slot and marker rows
/// under `holes`, both under `none`) is only drawn dimmed - it can still be
/// stepped and edited.
pub fn field_applies(field: &Field, params: &LayoutParams) -> bool {
    match datum_row(field.attr) {
        None => true,
        Some(want) => want == params.datum,
    }
}

/// Human readable current value of `field`.
pub fn value_text(field: &Field, params: &LayoutParams) -> String {
    match field.kind {
        FieldKind::Bool => if field_flag(field.attr, params) {
            "on"
        } else {
            "off"
        }
        .to_string(),
        FieldKind::Choice => field_choice(field.attr, params).to_string(),
        FieldKind::Length => format!(
            "{} {}",
            fmt_mm(field_number(field.attr, params)),
            field.unit
        )
        .trim()
        .to_string(),
    }
}

/// Is `value` acceptable for `field`?
pub fn valid_value(field: &Field, value: f64) -> bool {
    if !value.is_finite() {
        return false;
    }
    match field.rule {
        Rule::Ge0 => value >= 0.0,
        Rule::Gt0 => value > 0.0,
        Rule::Any => true,
    }
}

/// `value` moved by `direction` steps, clamped to the field minimum.
pub fn step_value(field: &Field, value: f64, direction: i32) -> f64 {
    let mut new = value + f64::from(direction) * field.step;
    if let Some(minimum) = field.minimum {
        new = minimum.max(new);
    }
    new = (new * 1e6).round() / 1e6;
    if new == 0.0 {
        0.0
    } else {
        new
    }
}

/// Parse an inline edit buffer; None when it is not a finite number.
pub fn parse_number(text: &str) -> Option<f64> {
    text.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Space bar: flip a bool, cycle a choice. Numeric fields are untouched.
pub fn toggle_field(field: &Field, params: &mut LayoutParams) {
    match field.kind {
        FieldKind::Bool => {
            let value = field_flag(field.attr, params);
            set_field_flag(field.attr, params, !value);
        }
        FieldKind::Choice => {
            let choices = field.choices;
            if choices.is_empty() {
                return;
            }
            let value = field_choice(field.attr, params).to_string();
            let next = match choices.iter().position(|c| *c == value) {
                Some(index) => choices[(index + 1) % choices.len()],
                None => choices[0],
            };
            set_field_choice(field.attr, params, next);
        }
        FieldKind::Length => {}
    }
}

/// The reverse of [`toggle_field`] for a choice; a bool just flips.
fn toggle_field_back(field: &Field, params: &mut LayoutParams) {
    if field.kind == FieldKind::Choice && !field.choices.is_empty() {
        let choices = field.choices;
        let value = field_choice(field.attr, params).to_string();
        let index = choices.iter().position(|c| *c == value).unwrap_or(0);
        let next = choices[(index + choices.len() - 1) % choices.len()];
        set_field_choice(field.attr, params, next);
        return;
    }
    toggle_field(field, params);
}

/// One row of the layout page (`editing` = the inline edit buffer).
pub fn format_field_row(
    field: &Field,
    params: &LayoutParams,
    cursor: bool,
    editing: Option<&str>,
) -> String {
    let value = match editing {
        Some(buffer) => format!("{buffer}_"),
        None => value_text(field, params),
    };
    format!(
        "{}{} {value}",
        if cursor { "→ " } else { "  " },
        col(field.label, COL_FIELD)
    )
}

/// Apply one key to an inline edit buffer.
///
/// Returns the new buffer, or None when the key is not an editing key.
pub fn edit_buffer(buffer: &str, key: KeyEvent) -> Option<String> {
    if key.code == KeyCode::Backspace {
        let mut out = buffer.to_string();
        out.pop();
        return Some(out);
    }
    if let KeyCode::Char(c) = key.code {
        if c.is_ascii_digit() || c == '.' || c == '-' {
            return Some(format!("{buffer}{c}"));
        }
    }
    None
}

// --------------------------------------------------------------------------- //
// Drawing helpers
// --------------------------------------------------------------------------- //

fn put(buf: &mut Buffer, x: u16, y: u16, text: &str, style: Style) -> u16 {
    let area = buf.area;
    if text.is_empty() || y >= area.bottom() || x >= area.right() {
        return x;
    }
    let max = usize::from(area.right() - x);
    buf.set_stringn(x, y, text, max, style).0
}

fn style_state(state: &str) -> Style {
    match state {
        STATE_OPEN => Style::new().fg(Color::Green).add_modifier(Modifier::BOLD),
        STATE_IGNORE => Style::new().fg(Color::Blue).add_modifier(Modifier::DIM),
        _ => Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
    }
}

fn style_kind(kind: Kind) -> Style {
    match kind {
        Kind::Fits => Style::new().fg(Color::Green).add_modifier(Modifier::BOLD),
        Kind::NoFit => Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        Kind::Header => Style::new().fg(Color::Cyan),
        Kind::Search => Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        Kind::Plain => Style::new(),
    }
}

/// Bold a full match; bold wins over the dim of a pasted/disabled row.
fn match_style(style: Style, full: bool) -> Style {
    if !full {
        return style;
    }
    style
        .remove_modifier(Modifier::DIM)
        .add_modifier(Modifier::BOLD)
}

fn with_cursor(style: Style, cursor: bool) -> Style {
    if cursor {
        style.add_modifier(Modifier::REVERSED)
    } else {
        style
    }
}

/// One line of text, centred in the column band `x0 .. x0 + width`.
fn draw_centered(buf: &mut Buffer, x0: u16, width: u16, height: u16, text: &str) {
    if width == 0 || height == 0 {
        return;
    }
    let text = ellipsis(text, usize::from(width));
    let used = text.chars().count() as u16;
    let x = x0 + width.saturating_sub(used) / 2;
    put(
        buf,
        x,
        height / 2,
        &text,
        Style::new().add_modifier(Modifier::DIM),
    );
}

/// The colour legend under the picture, every colour word in its own colour.
fn draw_legend(buf: &mut Buffer, x0: u16, width: u16, y: u16) {
    let right = x0.saturating_add(width);
    let mut x = x0;
    for (index, (text, class)) in LEGEND.iter().enumerate() {
        if index > 0 {
            x = put(buf, x, y, ITEM_SEP, Style::new());
        }
        let (colour, rest) = text.split_once(' ').unwrap_or((text, ""));
        if x >= right {
            return;
        }
        x = put(
            buf,
            x,
            y,
            &ellipsis(colour, usize::from(right - x)),
            Style::new().fg(ascii::class_color(*class)),
        );
        if x >= right {
            return;
        }
        x = put(
            buf,
            x,
            y,
            &ellipsis(&format!(" {rest}"), usize::from(right - x)),
            Style::new().add_modifier(Modifier::DIM),
        );
    }
}

/// The right half: the caption, the half-block picture and the legend.
fn draw_panel(buf: &mut Buffer, x0: u16, width: u16, height: u16, raster: &Raster) {
    if width == 0 || height == 0 {
        return;
    }
    put(
        buf,
        x0,
        0,
        &ellipsis(&raster.caption, usize::from(width)),
        style_kind(Kind::Header),
    );
    for row in 0..raster.rows() {
        let y = 1 + row as u16;
        if y >= height {
            break;
        }
        for col in 0..raster.cols {
            let x = x0 + col as u16;
            let (symbol, fg, bg) = raster.cell(col, row);
            if symbol == " " && fg == Color::Reset && bg == Color::Reset {
                continue; // leave the terminal background alone
            }
            put(buf, x, y, symbol, Style::new().fg(fg).bg(bg));
        }
    }
    if height >= LEGEND_MIN_HEIGHT {
        draw_legend(buf, x0, width, height - 1);
    }
}

fn ljust(text: &str, width: usize) -> String {
    let n = text.chars().count();
    let mut out = text.to_string();
    out.extend(std::iter::repeat_n(' ', width.saturating_sub(n)));
    out
}

// --------------------------------------------------------------------------- //
// The application
// --------------------------------------------------------------------------- //

/// What [`App::handle_key`] tells the terminal loop to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// keep running
    Continue,
    /// the user confirmed generation (`w`)
    Generate,
    /// the user quit (`q`)
    Quit,
}

/// One row of one page, used to follow the cursor by identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Pad(PadId),
    Side(SideId),
    Size(usize),
    Field(usize),
}

/// The four page front-end: all state, drawing and key dispatch.
///
/// `'p` covers the borrowed model, `'h` the caller's callbacks.
pub struct App<'p, 'h> {
    projects: &'p mut [Project],
    sides: &'p [SideId],
    config: &'p mut Config,
    hooks: TuiHooks<'h>,
    title: String,
    /// every pad of the relevant sides, already sorted
    pads: Vec<PadId>,
    page: usize,
    cursor: [usize; 4],
    top: [usize; 4],
    /// `*`: list the pads with paste too
    show_all: bool,
    visible: Vec<PadId>,
    side_signature: Option<(bool, Vec<SideId>)>,
    layout: Option<Layout>,
    presets: Option<Vec<[Option<bool>; 2]>>,
    status: String,
    pending: Option<char>,
    /// inline edit buffer
    editing: Option<String>,
    /// the buffer still holds the untouched prefill
    edit_fresh: bool,
    /// search query (None: not searching)
    search: Option<String>,
    /// the filter a finished search left behind, per page
    filters: [Option<String>; 4],
    filtered_pads: Option<Vec<PadId>>,
    filtered_sides: Option<Vec<SideId>>,
    counts_pads: Option<Vec<usize>>,
    counts_sides: Option<Vec<usize>>,
    /// row focused when the search started
    anchor: Option<Row>,
    /// `p` asked for a preview; run it after the "rendering…" frame
    preview_request: Option<Option<PadId>>,
    view_h: u16,
    /// `v`: the terminal preview takes the right half of the screen
    split: bool,
    /// bumped by every change the right half has to follow
    generation: u64,
    /// `object_geometry` per side, built lazily and kept (the expensive part)
    geom_cache: GeomCache,
    /// the last picture, keyed by (pad, panel size, generation)
    panel_cache: Option<(PanelKey, Option<Raster>)>,
}

/// What a cached right-half picture was drawn for.
type PanelKey = (Option<PadId>, u16, u16, u64);

impl<'p, 'h> App<'p, 'h> {
    /// Build the app. `pads` are *all* pads of `sides`, already sorted.
    pub fn new(
        projects: &'p mut [Project],
        sides: &'p [SideId],
        pads: Vec<PadId>,
        config: &'p mut Config,
        hooks: TuiHooks<'h>,
        title: &str,
    ) -> Self {
        let has_candidates = pads.iter().any(|id| pad_of(projects, *id).is_candidate());
        let page = if has_candidates || sides.is_empty() {
            PAGE_PADS
        } else {
            PAGE_SIDES
        };
        let mut app = Self {
            projects,
            sides,
            config,
            hooks,
            title: title.to_string(),
            pads,
            page,
            cursor: [0; 4],
            top: [0; 4],
            show_all: false,
            visible: Vec::new(),
            side_signature: None,
            layout: None,
            presets: None,
            status: String::new(),
            pending: None,
            editing: None,
            edit_fresh: false,
            search: None,
            filters: [None, None, None, None],
            filtered_pads: None,
            filtered_sides: None,
            counts_pads: None,
            counts_sides: None,
            anchor: None,
            preview_request: None,
            view_h: 24,
            split: false,
            generation: 0,
            geom_cache: GeomCache::new(),
            panel_cache: None,
        };
        app.sync_visible(None);
        app
    }

    // -- accessors --------------------------------------------------------- //

    pub fn page(&self) -> usize {
        self.page
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn show_all(&self) -> bool {
        self.show_all
    }

    /// Is the split view on (`v`)?
    pub fn split(&self) -> bool {
        self.split
    }

    /// Counts every change the right half has to follow.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn editing(&self) -> Option<&str> {
        self.editing.as_deref()
    }

    pub fn search(&self) -> Option<&str> {
        self.search.as_deref()
    }

    /// The filter the current page kept after a finished search.
    pub fn active_filter(&self) -> Option<&str> {
        self.filters[self.page].as_deref()
    }

    pub fn pending(&self) -> Option<char> {
        self.pending
    }

    pub fn layout(&self) -> Option<&Layout> {
        self.layout.as_ref()
    }

    /// The query the current page is filtered by (None: no filter).
    ///
    /// While the search is open that is the query being typed; afterwards it is
    /// what Enter kept.
    pub fn query(&self) -> Option<&str> {
        match &self.search {
            Some(q) => Some(q.as_str()),
            None => self.filters[self.page].as_deref(),
        }
    }

    fn match_counts(&self) -> Option<&[usize]> {
        match self.page {
            PAGE_PADS => self.counts_pads.as_deref(),
            PAGE_SIDES => self.counts_sides.as_deref(),
            _ => None,
        }
    }

    fn pad(&self, id: PadId) -> &Pad {
        pad_of(self.projects, id)
    }

    fn side(&self, id: SideId) -> &Side {
        id.get(self.projects)
    }

    // -- rows -------------------------------------------------------------- //

    /// The pads the Pads page shows: the filtered ones when it has a filter.
    fn page_pads(&self) -> &[PadId] {
        self.filtered_pads.as_deref().unwrap_or(&self.visible)
    }

    /// The sides the Sides page shows.
    fn page_sides(&self) -> &[SideId] {
        self.filtered_sides.as_deref().unwrap_or(self.sides)
    }

    fn base_row_count(&self, page: usize) -> usize {
        match page {
            PAGE_PADS => self.visible.len(),
            PAGE_SIDES => self.sides.len(),
            PAGE_STENCIL => STENCIL_SIZES.len(),
            _ => LAYOUT_FIELDS.len(),
        }
    }

    fn row_count(&self, page: usize) -> usize {
        match page {
            PAGE_PADS => self.page_pads().len(),
            PAGE_SIDES => self.page_sides().len(),
            PAGE_STENCIL => STENCIL_SIZES.len(),
            _ => LAYOUT_FIELDS.len(),
        }
    }

    fn rows(&self) -> usize {
        self.row_count(self.page)
    }

    /// The cursor of the current page, clamped into the list.
    pub fn index(&self) -> usize {
        self.cursor[self.page].min(self.rows().saturating_sub(1))
    }

    fn set_index(&mut self, value: usize) {
        self.cursor[self.page] = value.min(self.rows().saturating_sub(1));
    }

    /// The row under the cursor (of the filtered list while searching).
    pub fn current_row(&self) -> Option<Row> {
        let index = self.index();
        if self.rows() == 0 {
            return None;
        }
        match self.page {
            PAGE_PADS => self.page_pads().get(index).map(|id| Row::Pad(*id)),
            PAGE_SIDES => self.page_sides().get(index).map(|id| Row::Side(*id)),
            PAGE_STENCIL => Some(Row::Size(index)),
            _ => Some(Row::Field(index)),
        }
    }

    /// The rows of the current page, in display order.
    pub fn rows_list(&self) -> Vec<Row> {
        match self.page {
            PAGE_PADS => self.page_pads().iter().map(|id| Row::Pad(*id)).collect(),
            PAGE_SIDES => self.page_sides().iter().map(|id| Row::Side(*id)).collect(),
            PAGE_STENCIL => (0..STENCIL_SIZES.len()).map(Row::Size).collect(),
            _ => (0..LAYOUT_FIELDS.len()).map(Row::Field).collect(),
        }
    }

    fn current_pad(&self) -> Option<PadId> {
        if self.page != PAGE_PADS {
            return None;
        }
        match self.current_row() {
            Some(Row::Pad(id)) => Some(id),
            _ => None,
        }
    }

    /// Where `item` is on `page` now; `fallback` (clamped) when it is gone.
    fn locate(&self, page: usize, item: Option<Row>, fallback: usize) -> usize {
        match page {
            PAGE_PADS => {
                let want = match item {
                    Some(Row::Pad(id)) => Some(id),
                    _ => None,
                };
                restore_index(self.page_pads(), want.as_ref(), fallback)
            }
            PAGE_SIDES => {
                let want = match item {
                    Some(Row::Side(id)) => Some(id),
                    _ => None,
                };
                restore_index(self.page_sides(), want.as_ref(), fallback)
            }
            _ => {
                let index = match item {
                    Some(Row::Size(i)) | Some(Row::Field(i)) => i,
                    _ => fallback,
                };
                index.min(self.row_count(page).saturating_sub(1))
            }
        }
    }

    // -- state ------------------------------------------------------------- //

    /// Every pad of the enabled sides, the ones with paste included.
    fn enabled_pads(&self) -> Vec<PadId> {
        visible_pads(self.projects, &self.pads, self.sides, true)
    }

    /// Recompute the visible pad list, keeping the cursor on the same pad.
    ///
    /// An active filter is re-applied to the new base list, so toggling a side
    /// or `*` filters the pads that are listed now; the cursor follows its pad
    /// by identity into whatever list it ends up in.
    fn sync_visible(&mut self, keep: Option<PadId>) {
        if self.search.is_some() {
            // while searching the cursor indexes the filtered list
            return;
        }
        let enabled: Vec<SideId> = self
            .sides
            .iter()
            .copied()
            .filter(|s| s.get(self.projects).enabled)
            .collect();
        let signature = (self.show_all, enabled);
        if self.side_signature.as_ref() == Some(&signature) && !self.visible.is_empty() {
            return;
        }
        let mut current = keep;
        if current.is_none() {
            current = self.page_pads().get(self.cursor[PAGE_PADS]).copied();
        }
        self.side_signature = Some(signature);
        self.visible = visible_pads(self.projects, &self.pads, self.sides, self.show_all);
        self.filter_page(PAGE_PADS, None, false);
        let fallback = if current.is_some() {
            self.cursor[PAGE_PADS]
        } else {
            0
        };
        self.cursor[PAGE_PADS] = restore_index(self.page_pads(), current.as_ref(), fallback);
    }

    fn refresh_layout(&mut self) {
        let compute = &mut *self.hooks.compute_layout;
        self.layout = Some(compute(self.projects, self.config));
    }

    fn refresh_presets(&mut self) {
        let mut presets = Vec::with_capacity(STENCIL_SIZES.len());
        for size in STENCIL_SIZES {
            let mut row = [None, None];
            for (i, orientation) in ORIENTATIONS.iter().enumerate() {
                let mut cfg = self.config.clone();
                cfg.size = size;
                cfg.orientation = (*orientation).to_string();
                let compute = &mut *self.hooks.compute_layout;
                row[i] = Some(compute(self.projects, &cfg).fits);
            }
            presets.push(row);
        }
        self.presets = Some(presets);
    }

    /// Something that influences the layout changed.
    fn invalidate(&mut self) {
        self.refresh_layout();
        self.presets = None;
        self.touch();
    }

    /// Compute the first layout (and the fit table when the Stencil page opens
    /// first). Call once before the first [`App::draw`].
    pub fn refresh(&mut self) {
        self.refresh_layout();
        if self.page == PAGE_STENCIL {
            self.refresh_presets();
        }
    }

    /// What the terminal loop does before every frame.
    pub fn before_draw(&mut self) {
        self.sync_visible(None);
        if self.page == PAGE_STENCIL && self.presets.is_none() {
            self.refresh_presets();
        }
    }

    /// Run the preview `p` asked for (after the "rendering…" frame): it renders
    /// the PNG *and* opens it in the image viewer. Returns true when one ran,
    /// so the caller redraws.
    pub fn run_preview(&mut self) -> bool {
        let Some(pad) = self.preview_request.take() else {
            return false;
        };
        let preview = &mut *self.hooks.on_preview;
        let path = preview(self.projects, self.config, pad, true);
        self.status = format!("preview opened: {path}");
        true
    }

    // -- search state ------------------------------------------------------ //

    /// How many distinct words the query has (0 without a filter).
    fn query_word_count(&self) -> usize {
        self.query().map(|q| query_words(q).len()).unwrap_or(0)
    }

    /// How many filtered rows match every word of the query.
    fn full_count(&self) -> usize {
        let (Some(query), Some(counts)) = (self.query(), self.match_counts()) else {
            return 0;
        };
        if counts.is_empty() {
            return 0;
        }
        full_matches(counts, query)
    }

    /// Does the filtered row at `index` match every word of the query?
    fn is_full(&self, index: usize) -> bool {
        let words = self.query_word_count();
        if words == 0 {
            return false;
        }
        match self.match_counts() {
            Some(counts) => counts.get(index).is_some_and(|c| *c >= words),
            None => false,
        }
    }

    // -- drawing ----------------------------------------------------------- //

    /// Render the whole screen: the four pages, and the split view on top.
    ///
    /// The terminal size is read from the frame on every draw, so a resize
    /// needs nothing but a repaint. In the split view the app is drawn into a
    /// buffer of its own width and blitted over the left half, which keeps
    /// every page's own drawing code unaware of the split.
    pub fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let (width, height) = (area.width, area.height);
        if width == 0 || height == 0 {
            return;
        }
        self.view_h = height;
        if !self.split {
            self.draw_app(frame.buffer_mut());
            return;
        }
        let plan = split_plan(width);
        // The app, into a buffer exactly as wide as its half.
        let left = (plan.left > 0).then(|| {
            let mut sub = Buffer::empty(Rect::new(0, 0, plan.left, height));
            self.draw_app(&mut sub);
            sub
        });
        // The picture, re-rasterised only when something it shows changed. A
        // panel with no room for a single row of it counts as "too small".
        let rows = panel_rows(height);
        let drawable = plan.preview && rows > 0;
        let fallback = self.panel_fallback();
        let panel = if drawable {
            self.panel(plan.right_w, rows)
        } else {
            None
        };
        let buf = frame.buffer_mut();
        if let Some(sub) = &left {
            for y in 0..height {
                for x in 0..plan.left {
                    buf[(x, y)] = sub[(x, y)].clone();
                }
            }
            for y in 0..height {
                put(buf, plan.left, y, SEPARATOR, Style::new());
            }
        }
        if !drawable {
            draw_centered(buf, plan.right_x, plan.right_w, height, SPLIT_PROMPT);
            return;
        }
        match panel {
            None => draw_centered(buf, plan.right_x, plan.right_w, height, &fallback),
            Some(raster) => draw_panel(buf, plan.right_x, plan.right_w, height, raster),
        }
    }

    /// Draw the four pages into `buf`; its area is the app's own half.
    fn draw_app(&mut self, buf: &mut Buffer) {
        let (width, height) = (usize::from(buf.area.width), usize::from(buf.area.height));
        if width == 0 || height == 0 {
            return;
        }
        let head_n = if height >= 7 {
            2
        } else if height >= 3 {
            1
        } else {
            0
        };
        let fit_n = usize::from(height >= 5);
        // The key help takes one or two lines; the list area shrinks by as much.
        let mut help = self.footer_lines(width, FOOTER_LINES);
        let mut help_n = 0usize;
        if height >= 2 && !help.is_empty() {
            help_n = help
                .len()
                .min(height.saturating_sub(head_n + fit_n + 1).max(1));
            if help_n < help.len() {
                // no room: re-wrap into what is left
                help = self.footer_lines(width, help_n);
                help_n = help_n.min(help.len());
            }
        }
        let body_h = height.saturating_sub(head_n + fit_n + help_n);

        self.scroll(body_h);

        if head_n >= 1 {
            self.draw_tabs(buf, width);
        }
        if head_n >= 2 {
            put(
                buf,
                0,
                1,
                &clip(&self.subheader(), width),
                style_kind(Kind::Header).add_modifier(Modifier::BOLD),
            );
        }
        if body_h > 0 {
            self.draw_body(buf, head_n as u16, body_h, width);
        }
        if fit_n == 1 {
            let y = (height - help_n - 1) as u16;
            self.draw_fit(buf, y);
        }
        for (row, text) in help.iter().take(help_n).enumerate() {
            let y = (height - help_n + row) as u16;
            self.draw_footer(buf, y, text, width, row == 0);
        }
    }

    // -- the split view ---------------------------------------------------- //

    /// The pad the right half draws: the one the *Pads page* cursor is on,
    /// whatever page is in front.
    pub fn panel_pad(&self) -> Option<PadId> {
        let rows = self.page_pads();
        let index = self.cursor[PAGE_PADS].min(rows.len().checked_sub(1)?);
        rows.get(index).copied()
    }

    /// What the right half says when there is no picture to draw.
    fn panel_fallback(&self) -> String {
        match self.panel_pad() {
            None => NO_PAD.to_string(),
            Some(id) => format!("{} is not placed on the sheet", self.pad(id).label()),
        }
    }

    /// The picture of the selected pad, rasterised only when it has to be.
    ///
    /// The cache is keyed by the pad, the size of the right half and a counter
    /// every mutation bumps, so moving the cursor, changing a pad state, a
    /// side, the configuration or the terminal size all redraw it and nothing
    /// else does.
    fn panel(&mut self, cols: u16, rows: u16) -> Option<&Raster> {
        let key: PanelKey = (self.panel_pad(), cols, rows, self.generation);
        if self.panel_cache.as_ref().map(|(k, _)| *k) != Some(key) {
            let mut raster = None;
            if let (Some(id), Some(layout)) = (key.0, self.layout.as_ref()) {
                raster = ascii::render_pad(
                    self.projects,
                    layout,
                    id,
                    &mut self.geom_cache,
                    usize::from(cols),
                    usize::from(rows),
                );
            }
            self.panel_cache = Some((key, raster));
        }
        self.panel_cache.as_ref().and_then(|(_, r)| r.as_ref())
    }

    /// `v`: show or hide the terminal preview.
    fn toggle_split(&mut self) {
        self.split = !self.split;
        self.status = if self.split {
            "split view on — the pad under the cursor is drawn on the right".to_string()
        } else {
            "split view off".to_string()
        };
    }

    /// Something the right half shows changed.
    fn touch(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// The `/query_` prompt of the footer while searching.
    fn search_prompt(&self) -> String {
        format!("/{}_", self.search.clone().unwrap_or_default())
    }

    /// The key-help items of the footer as it is right now.
    fn help_items(&self) -> Vec<String> {
        if self.search.is_some() {
            let mut items = vec![self.search_prompt()];
            items.extend(SEARCH_ITEMS.iter().map(|s| (*s).to_string()));
            return items;
        }
        if self.editing.is_some() {
            return EDIT_ITEMS.iter().map(|s| (*s).to_string()).collect();
        }
        page_items(self.page, self.filters[self.page].is_some())
    }

    /// The footer text laid out: a status message, or the key help.
    fn footer_lines(&self, width: usize, max_lines: usize) -> Vec<String> {
        if self.search.is_none() && !self.status.is_empty() {
            return wrap_text(&self.status, width, max_lines);
        }
        wrap_items(&self.help_items(), width, max_lines)
    }

    /// One footer line; the search prompt of the first line is highlighted.
    fn draw_footer(&self, buf: &mut Buffer, y: u16, text: &str, width: usize, first: bool) {
        let mut x = 0u16;
        let mut rest = text.to_string();
        if first && self.search.is_some() {
            let prompt = self.search_prompt();
            if let Some(tail) = text.strip_prefix(prompt.as_str()) {
                x = put(
                    buf,
                    0,
                    y,
                    &prompt,
                    style_kind(Kind::Search).add_modifier(Modifier::REVERSED),
                );
                rest = tail.to_string();
            }
        }
        let pad_to = width.saturating_sub(usize::from(x));
        put(
            buf,
            x,
            y,
            &ljust(&rest, pad_to),
            Style::new().add_modifier(Modifier::REVERSED),
        );
    }

    fn draw_tabs(&self, buf: &mut Buffer, width: usize) {
        let mut x = 0u16;
        if !self.title.is_empty() {
            x = put(
                buf,
                0,
                0,
                &clip(&format!("{}   ", self.title), width / 2),
                style_kind(Kind::Header).add_modifier(Modifier::BOLD),
            );
        }
        for (i, name) in PAGE_NAMES.iter().enumerate() {
            let style = if i == self.page {
                Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD)
            } else {
                Style::new()
            };
            x = put(buf, x, 0, &format!(" {} {name} ", i + 1), style);
            x = put(buf, x, 0, "  ", Style::new());
        }
    }

    fn subheader(&self) -> String {
        if let Some(query) = self.query() {
            let tail = match_summary(
                self.rows(),
                self.base_row_count(self.page),
                self.full_count(),
                self.query_word_count(),
            );
            return format!("filter: {query}  {tail}");
        }
        match self.page {
            PAGE_PADS => {
                let total = self.visible.len();
                let position = if total > 0 {
                    format!("{}/{total}", self.index() + 1)
                } else {
                    "0/0".to_string()
                };
                let (what, hint, hidden) = if self.show_all {
                    (
                        format!(
                            "all pads ({total}) — pads with paste are dimmed; ignore closes their opening"
                        ),
                        "* = back to pads to decide",
                        self.pads.len() as isize - total as isize,
                    )
                } else {
                    let candidates = self
                        .pads
                        .iter()
                        .filter(|id| self.pad(**id).is_candidate())
                        .count();
                    (
                        format!("pads to decide ({total})"),
                        "* = show all pads",
                        candidates as isize - total as isize,
                    )
                };
                let extra = if hidden > 0 {
                    format!("   ({hidden} hidden on disabled sides)")
                } else {
                    String::new()
                };
                format!("pads {position}   {what}   {hint}{extra}")
            }
            PAGE_SIDES => {
                let enabled = self.sides.iter().filter(|s| self.side(**s).enabled).count();
                format!("sides: {enabled}/{} enabled", self.sides.len())
            }
            PAGE_STENCIL => {
                let (w, h) = self.config.sheet_size();
                let mut text = format!(
                    "sheet {w:.1} x {h:.1} mm   orientation {}",
                    self.config.orientation
                );
                if let Some(layout) = &self.layout {
                    text.push_str(&format!(
                        "   block {:.1} x {:.1} mm",
                        layout.block_width(),
                        layout.block_height()
                    ));
                }
                text
            }
            _ => "layout parameters".to_string(),
        }
    }

    fn draw_body(&self, buf: &mut Buffer, y0: u16, body_h: usize, width: usize) {
        match self.page {
            PAGE_PADS => self.draw_pads(buf, y0, body_h, width),
            PAGE_SIDES => self.draw_sides(buf, y0, body_h, width),
            PAGE_STENCIL => self.draw_stencil(buf, y0, body_h),
            _ => self.draw_layout(buf, y0, body_h),
        }
    }

    fn filtering(&self) -> bool {
        matches!(self.query(), Some(q) if !q.is_empty())
    }

    fn draw_pads(&self, buf: &mut Buffer, mut y0: u16, body_h: usize, width: usize) {
        let rows_list = self.page_pads();
        if rows_list.is_empty() {
            let message = if self.filtering() {
                "no pad matches the filter"
            } else if self.show_all {
                if self.pads.is_empty() {
                    "no pads at all"
                } else {
                    "no pads on the enabled sides"
                }
            } else if self.pads.iter().any(|id| self.pad(*id).is_candidate()) {
                "no pads to decide on the enabled sides"
            } else {
                "no candidate pads at all"
            };
            put(
                buf,
                2,
                y0,
                message,
                Style::new().add_modifier(Modifier::DIM),
            );
            return;
        }
        let mut rows = body_h;
        if rows >= 3 {
            put(
                buf,
                0,
                y0,
                &pads_header(),
                Style::new().add_modifier(Modifier::DIM | Modifier::UNDERLINED),
            );
            y0 += 1;
            rows -= 1;
        }
        for row in 0..rows {
            let index = self.top[self.page] + row;
            let Some(id) = rows_list.get(index) else {
                break;
            };
            let pad = self.pad(*id);
            let cursor = index == self.index();
            // matches every word: drawn bold
            let full = self.is_full(index);
            let base = with_cursor(Style::new(), cursor);
            let mut x = 0u16;
            for (text, kind) in row_segments(pad, cursor) {
                let style = match kind {
                    SegKind::State => with_cursor(style_state(&pad.state), cursor),
                    SegKind::PadLabel if pad.has_paste => base.add_modifier(Modifier::DIM),
                    _ => base,
                };
                x = put(buf, x, y0 + row as u16, &text, match_style(style, full));
            }
            if cursor && usize::from(x) < width {
                put(
                    buf,
                    x,
                    y0 + row as u16,
                    &" ".repeat(width - usize::from(x)),
                    match_style(base, full),
                );
            }
        }
    }

    fn draw_sides(&self, buf: &mut Buffer, y0: u16, body_h: usize, width: usize) {
        let rows_list = self.page_sides();
        if rows_list.is_empty() {
            let message = if self.filtering() {
                "no side matches the filter"
            } else {
                "no sides"
            };
            put(
                buf,
                2,
                y0,
                message,
                Style::new().add_modifier(Modifier::DIM),
            );
            return;
        }
        for row in 0..body_h {
            let index = self.top[self.page] + row;
            let Some(id) = rows_list.get(index) else {
                break;
            };
            let side = self.side(*id);
            let cursor = index == self.index();
            let mut style = with_cursor(Style::new(), cursor);
            if !side.enabled {
                style = style.add_modifier(Modifier::DIM);
            }
            let style = match_style(style, self.is_full(index));
            let text = side_row(side, cursor);
            put(buf, 0, y0 + row as u16, &ljust(&text, width), style);
        }
    }

    fn draw_stencil(&self, buf: &mut Buffer, y0: u16, body_h: usize) {
        for row in 0..body_h {
            let index = self.top[self.page] + row;
            if index >= STENCIL_SIZES.len() {
                break;
            }
            let size = STENCIL_SIZES[index];
            let fits = match &self.presets {
                Some(presets) => presets[index],
                None => [None, None],
            };
            let cursor = index == self.index();
            let mut style = with_cursor(Style::new(), cursor);
            if size == self.config.size {
                style = style.add_modifier(Modifier::BOLD);
            }
            put(
                buf,
                0,
                y0 + row as u16,
                &preset_row(size, fits, self.config, cursor),
                style,
            );
        }
    }

    fn draw_layout(&self, buf: &mut Buffer, y0: u16, body_h: usize) {
        for row in 0..body_h {
            let index = self.top[self.page] + row;
            if index >= LAYOUT_FIELDS.len() {
                break;
            }
            let field = &LAYOUT_FIELDS[index];
            let cursor = index == self.index();
            let editing = if cursor {
                self.editing.as_deref()
            } else {
                None
            };
            let mut style = with_cursor(Style::new(), cursor);
            if !field_applies(field, &self.config.layout) {
                style = style.add_modifier(Modifier::DIM);
            }
            put(
                buf,
                0,
                y0 + row as u16,
                &format_field_row(field, &self.config.layout, cursor, editing),
                style,
            );
        }
    }

    fn draw_fit(&self, buf: &mut Buffer, y: u16) {
        let mut x = 0u16;
        // Count every pad of the enabled sides, not only the listed ones, so
        // the "closed" number stays visible in the pads-to-decide view.
        let ids = self.enabled_pads();
        let counts = count_states(ids.iter().map(|id| self.pad(*id)));
        for (text, kind) in fit_segments(self.config, self.layout.as_ref(), counts) {
            x = put(buf, x, y, &text, style_kind(kind));
        }
    }

    /// Keep the cursor row inside the visible window.
    fn scroll(&mut self, body_h: usize) {
        let rows = self.rows();
        let mut list_h = body_h;
        if self.page == PAGE_PADS && body_h >= 3 {
            list_h = body_h - 1; // column header
        }
        let index = self.index();
        let mut top = self.top[self.page];
        if list_h == 0 {
            self.top[self.page] = index;
            return;
        }
        if index < top {
            top = index;
        } else if index >= top + list_h {
            top = index + 1 - list_h;
        }
        self.top[self.page] = top.min(rows.saturating_sub(list_h));
    }

    // -- actions ----------------------------------------------------------- //

    fn move_by(&mut self, delta: isize) {
        let index = (self.index() as isize + delta).max(0) as usize;
        self.set_index(index);
        self.status.clear();
    }

    fn page_step(&self) -> isize {
        (self.view_h as isize - 5).max(1)
    }

    fn goto(&mut self, page: usize) {
        self.page = page.min(3);
        self.status.clear();
        if self.page == PAGE_PADS {
            self.sync_visible(None);
        }
        // Every page keeps its own filter; the new one is re-applied (its rows
        // may have changed while another page was in front) without moving its
        // cursor off the row it was left on.
        self.apply_filter(None, false);
        if self.page == PAGE_STENCIL && self.presets.is_none() {
            self.refresh_presets();
        }
    }

    fn jump_undefined(&mut self, forward: bool) {
        let rows = self.page_pads().to_vec();
        match find_undefined(self.projects, &rows, self.index(), forward) {
            None => self.status = "no undefined pads left".to_string(),
            Some(index) => {
                self.set_index(index);
                self.status = format!("undefined pad {}/{}", index + 1, rows.len());
            }
        }
    }

    fn set_state(&mut self, state: &str) {
        let Some(id) = self.current_pad() else { return };
        let (label, allowed) = {
            let pad = self.pad(id);
            (pad.label(), allowed_state(pad, state))
        };
        if !allowed {
            // a pasted pad is never undefined
            self.status = format!("{label} has paste: it is only open or ignore");
            return;
        }
        let pad = pad_of_mut(self.projects, id);
        pad.state = state.to_string();
        let note = if pad.has_paste {
            if pad.state == STATE_IGNORE {
                " — paste opening closed"
            } else {
                " — paste opening kept"
            }
        } else {
            ""
        };
        let (project, side) = (pad.project.clone(), pad.side.clone());
        self.status = format!("{label} ({project} {side}) -> {state}{note}");
        self.touch();
    }

    fn apply_to_component(&mut self) {
        let Some(id) = self.current_pad() else { return };
        let rows = self.visible.clone();
        let changed = apply_component(self.projects, &rows, id, None);
        let (ref_, state) = {
            let pad = self.pad(id);
            (pad.ref_.clone(), pad.state.clone())
        };
        self.status = format!("{ref_}: {changed} pad(s) -> {state}");
        self.touch();
    }

    /// `*`: list every pad of the enabled sides, or only the candidates.
    fn toggle_show_all(&mut self) {
        let keep = self.current_pad();
        self.show_all = !self.show_all;
        self.sync_visible(keep);
        self.status = if self.show_all {
            "showing all pads — the ones with paste are dimmed; set one to ignore to close its opening"
                .to_string()
        } else {
            "showing the pads to decide".to_string()
        };
    }

    fn toggle_side(&mut self, id: SideId) {
        let side = id.get_mut(self.projects);
        side.enabled = !side.enabled;
        let (label, enabled) = (side.label(), side.enabled);
        self.status = format!("{label}: {}", if enabled { "enabled" } else { "disabled" });
        self.sync_visible(None);
        self.invalidate();
    }

    fn set_all_sides(&mut self, enabled: bool) {
        for id in self.sides {
            id.get_mut(self.projects).enabled = enabled;
        }
        self.status = format!("all sides {}", if enabled { "enabled" } else { "disabled" });
        self.sync_visible(None);
        self.invalidate();
    }

    fn select_size(&mut self, size: (u32, u32)) {
        self.config.size = size;
        self.status = format!("stencil size {}", self.config.size_label());
        self.invalidate();
        self.refresh_presets();
    }

    fn toggle_orientation(&mut self) {
        let index = ORIENTATIONS
            .iter()
            .position(|o| *o == self.config.orientation);
        let next = match index {
            Some(i) => ORIENTATIONS[(i + 1) % ORIENTATIONS.len()],
            None => ORIENTATIONS[0],
        };
        self.config.orientation = next.to_string();
        self.status = format!("orientation {next}");
        self.invalidate();
        if self.page == PAGE_STENCIL {
            self.refresh_presets();
        }
    }

    fn step_field(&mut self, field: &Field, direction: i32) {
        if !field.numeric() {
            if direction > 0 || field.kind == FieldKind::Bool {
                toggle_field(field, &mut self.config.layout);
            } else {
                toggle_field_back(field, &mut self.config.layout);
            }
            self.status = format!(
                "{}: {}",
                field.label,
                value_text(field, &self.config.layout)
            );
            self.invalidate();
            return;
        }
        let value = field_number(field.attr, &self.config.layout);
        let new = step_value(field, value, direction);
        if !valid_value(field, new) {
            self.status = format!("{}: {} is not allowed", field.label, fmt_mm(new));
            return;
        }
        set_field_number(field.attr, &mut self.config.layout, new);
        self.status = format!(
            "{}: {}",
            field.label,
            value_text(field, &self.config.layout)
        );
        self.invalidate();
    }

    fn toggle_layout_field(&mut self, field: &Field) {
        if field.numeric() {
            self.start_edit(field);
            return;
        }
        toggle_field(field, &mut self.config.layout);
        self.status = format!(
            "{}: {}",
            field.label,
            value_text(field, &self.config.layout)
        );
        self.invalidate();
    }

    fn start_edit(&mut self, field: &Field) {
        if !field.numeric() {
            self.toggle_layout_field(field);
            return;
        }
        self.editing = Some(fmt_mm(field_number(field.attr, &self.config.layout)));
        // the first typed character replaces the prefilled value
        self.edit_fresh = true;
        self.status = format!(
            "editing {} — type replaces the value, backspace edits, enter accepts, esc cancels",
            field.label
        );
    }

    fn edit_key(&mut self, key: KeyEvent) {
        let field = LAYOUT_FIELDS[self.index()];
        if key.code == KeyCode::Enter {
            let buffer = self.editing.clone().unwrap_or_default();
            match parse_number(&buffer).filter(|v| valid_value(&field, *v)) {
                None => {
                    self.status = format!("invalid value {buffer:?} for {}", field.label);
                }
                Some(value) => {
                    let value = (value * 1e6).round() / 1e6;
                    set_field_number(field.attr, &mut self.config.layout, value);
                    self.editing = None;
                    self.status = format!(
                        "{}: {}",
                        field.label,
                        value_text(&field, &self.config.layout)
                    );
                    self.invalidate();
                }
            }
            return;
        }
        if key.code == KeyCode::Esc {
            // cancels the edit, nothing else
            self.editing = None;
            self.status = "edit cancelled".to_string();
            return;
        }
        let mut base = self.editing.clone().unwrap_or_default();
        if self.edit_fresh && key.code != KeyCode::Backspace {
            base.clear();
        }
        if let Some(buffer) = edit_buffer(&base, key) {
            self.edit_fresh = false;
            self.editing = Some(buffer);
            self.status.clear();
        }
    }

    // -- search ------------------------------------------------------------ //

    /// Only the two list pages can be searched.
    fn searchable(&self) -> bool {
        self.page == PAGE_PADS || self.page == PAGE_SIDES
    }

    /// `/`: a fresh search with an empty (everything passes) query.
    ///
    /// A filter that is still on the list is dropped right away, so typing
    /// replaces it live instead of narrowing it further.
    fn start_search(&mut self) {
        let anchor = self.current_row();
        self.search = Some(String::new());
        self.filters[self.page] = None;
        self.anchor = anchor;
        self.status.clear();
        self.apply_filter(anchor, true);
    }

    /// Re-filter `page` for its query; `move_row` puts its cursor back on
    /// `keep` (the first - the best - match when it is gone), otherwise the
    /// cursor only stays in range.
    fn filter_page(&mut self, page: usize, keep: Option<Row>, move_row: bool) {
        let query = if page == self.page {
            self.search.clone()
        } else {
            None
        };
        let query = query.or_else(|| self.filters[page].clone());
        match (query, page) {
            (Some(query), PAGE_PADS) => {
                let rows = self.visible.clone();
                let projects: &[Project] = self.projects;
                let pairs = filter_counts(&rows, |id| pad_fields(pad_of(projects, *id)), &query);
                self.filtered_pads = Some(pairs.iter().map(|(row, _)| *row).collect());
                self.counts_pads = Some(pairs.iter().map(|(_, count)| *count).collect());
            }
            (Some(query), PAGE_SIDES) => {
                let rows = self.sides.to_vec();
                let projects: &[Project] = self.projects;
                let pairs = filter_counts(&rows, |id| side_fields(id.get(projects)), &query);
                self.filtered_sides = Some(pairs.iter().map(|(row, _)| *row).collect());
                self.counts_sides = Some(pairs.iter().map(|(_, count)| *count).collect());
            }
            (_, PAGE_PADS) => {
                self.filtered_pads = None;
                self.counts_pads = None;
            }
            (_, PAGE_SIDES) => {
                self.filtered_sides = None;
                self.counts_sides = None;
            }
            _ => {}
        }
        if move_row {
            self.cursor[page] = self.locate(page, keep, 0);
        } else {
            self.cursor[page] = self.cursor[page].min(self.row_count(page).saturating_sub(1));
        }
    }

    /// Re-filter the current page, staying on `keep`.
    fn apply_filter(&mut self, keep: Option<Row>, move_row: bool) {
        self.filter_page(self.page, keep, move_row);
    }

    /// Leave the search: Enter keeps the filter, Esc drops it.
    ///
    /// Either way the cursor stays on the row it was focused on - on the row
    /// the search started from when nothing matched.
    fn end_search(&mut self, keep_filter: bool) {
        let target = self.current_row().or(self.anchor);
        let fallback = self.cursor[self.page];
        let query = self.search.take();
        let keep = match &query {
            Some(q) if keep_filter && !query_words(q).is_empty() => query.clone(),
            _ => None,
        };
        self.filters[self.page] = keep;
        if self.filters[self.page].is_none() {
            self.anchor = None;
        }
        // else the anchor is kept: it is where the cursor goes back to when a
        // filter that matched nothing is cleared again.
        self.apply_filter(None, false);
        self.cursor[self.page] = self.locate(self.page, target, fallback);
        self.status.clear();
    }

    /// Esc outside the search: drop the filter, staying on the same row.
    fn clear_filter(&mut self) {
        let target = self.current_row().or(self.anchor);
        let fallback = self.cursor[self.page];
        self.anchor = None;
        self.filters[self.page] = None;
        self.apply_filter(None, false);
        self.cursor[self.page] = self.locate(self.page, target, fallback);
        self.status = "filter cleared".to_string();
    }

    /// One key while searching: navigate, edit the query or leave.
    fn search_key(&mut self, key: KeyEvent) {
        match search_action(key) {
            SearchAction::Keep => self.end_search(true),
            SearchAction::Clear => self.end_search(false),
            SearchAction::Backspace => {
                if self.search.as_deref().is_some_and(|q| !q.is_empty()) {
                    let current = self.current_row();
                    if let Some(query) = self.search.as_mut() {
                        query.pop();
                    }
                    self.apply_filter(current, true);
                }
            }
            SearchAction::Up => self.move_by(-1),
            SearchAction::Down => self.move_by(1),
            SearchAction::PageUp => self.move_by(-self.page_step()),
            SearchAction::PageDown => self.move_by(self.page_step()),
            SearchAction::First => self.set_index(0),
            SearchAction::Last => self.set_index(self.rows().saturating_sub(1)),
            SearchAction::Char => {
                let Some(c) = search_char(key) else { return };
                let current = self.current_row();
                if let Some(query) = self.search.as_mut() {
                    query.push(c);
                }
                self.apply_filter(current, true);
            }
            SearchAction::Ignore => {}
        }
    }

    /// `p`: render the preview PNG and open it in the image viewer.
    fn preview(&mut self) {
        let pad = if self.page == PAGE_PADS {
            self.current_pad()
        } else {
            None
        };
        self.status = "rendering…".to_string();
        self.preview_request = Some(pad);
    }

    /// `Generate` to generate, `Continue` when a second confirmation is needed.
    fn confirm(&mut self, keyname: char, pending: Option<char>) -> Action {
        let no_fit = self.layout.as_ref().is_some_and(|l| !l.fits);
        if no_fit && pending != Some(keyname) {
            self.pending = Some(keyname);
            self.status = NOFIT_WARNING.to_string();
            return Action::Continue;
        }
        Action::Generate
    }

    /// Esc outside the search and the inline edit - it never quits.
    ///
    /// `pending` is the confirmation the previous key left behind; it has
    /// already been cleared by the dispatcher, so cancelling it is only a
    /// message. With nothing pending Esc clears the filter, and with no filter
    /// either it does nothing at all.
    fn escape(&mut self, pending: Option<char>) {
        match escape_action(
            false,
            false,
            pending.is_some(),
            self.filters[self.page].is_some(),
        ) {
            EscAction::Pending => self.status = "generate cancelled".to_string(),
            EscAction::Filter => self.clear_filter(),
            _ => {}
        }
    }

    // -- key dispatch ------------------------------------------------------ //

    /// Handle one key press.
    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        if self.editing.is_some() {
            self.edit_key(key);
            return Action::Continue;
        }
        if self.search.is_some() {
            self.search_key(key);
            return Action::Continue;
        }

        let pending = self.pending.take();

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            // Ctrl-C leaves the program like curses' KeyboardInterrupt did.
            if key.code == KeyCode::Char('c') {
                return Action::Quit;
            }
            return Action::Continue;
        }

        if key.code == KeyCode::Esc {
            // never quits
            self.escape(pending);
            return Action::Continue;
        }

        match key.code {
            KeyCode::Char('/') if self.searchable() => {
                self.start_search();
                return Action::Continue;
            }
            KeyCode::Char(c @ '1'..='4') => {
                self.goto(c as usize - '1' as usize);
                return Action::Continue;
            }
            KeyCode::Tab if !key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.goto((self.page + 1) % PAGE_NAMES.len());
                return Action::Continue;
            }
            KeyCode::BackTab | KeyCode::Tab => {
                self.goto((self.page + PAGE_NAMES.len() - 1) % PAGE_NAMES.len());
                return Action::Continue;
            }
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Char('p') => {
                self.preview();
                return Action::Continue;
            }
            KeyCode::Char('v') => {
                self.toggle_split();
                return Action::Continue;
            }
            // 'w' generates everywhere
            KeyCode::Char('w') => return self.confirm('w', pending),
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_by(-1);
                return Action::Continue;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_by(1);
                return Action::Continue;
            }
            KeyCode::PageUp => {
                self.move_by(-self.page_step());
                return Action::Continue;
            }
            KeyCode::PageDown => {
                self.move_by(self.page_step());
                return Action::Continue;
            }
            // vim's gg
            KeyCode::Home | KeyCode::Char('g') => {
                self.set_index(0);
                self.status.clear();
                return Action::Continue;
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.set_index(self.rows().saturating_sub(1));
                self.status.clear();
                return Action::Continue;
            }
            _ => {}
        }

        // Nothing below generates or leaves the loop any more - 'w' is the only
        // key that does, and it was handled above.
        match self.page {
            PAGE_PADS => self.handle_pads(key),
            PAGE_SIDES => self.handle_sides(key),
            PAGE_STENCIL => self.handle_stencil(key),
            _ => self.handle_layout(key),
        }
        Action::Continue
    }

    fn handle_pads(&mut self, key: KeyEvent) {
        // Enter does nothing here: 'w' generates.
        match key.code {
            KeyCode::Char('*') => self.toggle_show_all(),
            KeyCode::Char(' ') => {
                if let Some(id) = self.current_pad() {
                    let state = next_state(self.pad(id));
                    self.set_state(state);
                }
            }
            KeyCode::Char('o') => self.set_state(STATE_OPEN),
            KeyCode::Char('i') => self.set_state(STATE_IGNORE),
            KeyCode::Char('a') => self.apply_to_component(),
            KeyCode::Char('n') => self.jump_undefined(true),
            KeyCode::Char('N') => self.jump_undefined(false),
            _ => {}
        }
    }

    fn handle_sides(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char(' ') | KeyCode::Enter => {
                // the row of the filtered list, if any
                if let Some(Row::Side(id)) = self.current_row() {
                    self.toggle_side(id);
                }
            }
            KeyCode::Char('A') => self.set_all_sides(true),
            KeyCode::Char('N') => self.set_all_sides(false),
            _ => {}
        }
    }

    fn handle_stencil(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char(' ') | KeyCode::Enter => {
                self.select_size(STENCIL_SIZES[self.index()]);
            }
            KeyCode::Char('o') => self.toggle_orientation(),
            _ => {}
        }
    }

    fn handle_layout(&mut self, key: KeyEvent) {
        let field = LAYOUT_FIELDS[self.index()];
        match key.code {
            KeyCode::Char('+' | '=') | KeyCode::Right => self.step_field(&field, 1),
            KeyCode::Char('-' | '_') | KeyCode::Left => self.step_field(&field, -1),
            KeyCode::Char(' ') => {
                if field.numeric() {
                    self.status = format!("{} is a number — press e or enter to edit", field.label);
                } else {
                    self.toggle_layout_field(&field);
                }
            }
            KeyCode::Char('e') | KeyCode::Enter => self.start_edit(&field),
            _ => {}
        }
    }
}

// --------------------------------------------------------------------------- //
// The terminal loop
// --------------------------------------------------------------------------- //

/// Restores the terminal whatever happens, panics included (the panic hook
/// `ratatui::try_init` installs covers the panic case).
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

/// Run the TUI. Mutates side.enabled, pad.state and `config` in place.
/// Returns true when the user confirmed generation (`w`), false on quit (`q`).
pub fn run_tui(
    projects: &mut [Project],
    sides: &[SideId],
    config: &mut Config,
    hooks: TuiHooks<'_>,
    title: &str,
) -> anyhow::Result<bool> {
    // With neither pads nor sides there is nothing to decide, so the call
    // succeeds immediately without touching the terminal.
    let pads = if sides.is_empty() {
        Vec::new()
    } else {
        crate::pads::sorted_pads(projects, sides, true)
    };
    if pads.is_empty() && sides.is_empty() {
        return Ok(true);
    }

    let mut app = App::new(projects, sides, pads, config, hooks, title);

    let mut terminal = ratatui::try_init()?;
    let _guard = TerminalGuard;
    terminal.hide_cursor()?;
    app.refresh();

    let result = event_loop(&mut app, &mut terminal);
    terminal.show_cursor().ok();
    result
}

fn event_loop<B: ratatui::backend::Backend>(
    app: &mut App<'_, '_>,
    terminal: &mut ratatui::Terminal<B>,
) -> anyhow::Result<bool>
where
    B::Error: std::error::Error + Send + Sync + 'static,
{
    use ratatui::crossterm::event::{self, Event, KeyEventKind};

    loop {
        app.before_draw();
        terminal.draw(|frame| app.draw(frame))?;
        // The preview blocks: show the "rendering…" frame above first.
        if app.run_preview() {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => match app.handle_key(key) {
                Action::Continue => {}
                Action::Generate => return Ok(true),
                Action::Quit => return Ok(false),
            },
            // `Terminal::draw` resizes on its own; just repaint.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;
    use std::rc::Rc;

    use geo::{LineString, MultiPolygon, Polygon};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use crate::gerber::{Aperture, Flash, GerberFile, GraphicObject};
    use crate::model::{all_sides, Bounds, Geom, DATUM_HOLES, DATUM_NONE, SIDE_BOTTOM, SIDE_TOP};

    // ---------------------------------------------------------------- //
    // fixtures
    // ---------------------------------------------------------------- //

    const A_TOP: SideId = SideId {
        project: 0,
        side: 0,
    };
    const A_BOT: SideId = SideId {
        project: 0,
        side: 1,
    };
    const B_TOP: SideId = SideId {
        project: 1,
        side: 0,
    };

    fn aperture() -> Rc<Aperture> {
        Rc::new(Aperture::new(
            10,
            "R",
            vec![1.0, 0.5],
            BTreeMap::new(),
            None,
        ))
    }

    fn flash(x: f64, y: f64) -> Flash {
        Flash {
            x,
            y,
            aperture: aperture(),
            attrs: BTreeMap::new(),
            dark: true,
        }
    }

    /// A pad shape built by hand: `Aperture::geometry()` is not needed here.
    fn rect(x: f64, y: f64) -> Geom {
        MultiPolygon::new(vec![Polygon::new(
            LineString::from(vec![
                (x - 0.5, y - 0.25),
                (x + 0.5, y - 0.25),
                (x + 0.5, y + 0.25),
                (x - 0.5, y + 0.25),
                (x - 0.5, y - 0.25),
            ]),
            vec![],
        )])
    }

    struct PadSpec {
        ref_: &'static str,
        pin: &'static str,
        x: f64,
        y: f64,
        function: &'static str,
        shape: &'static str,
        has_paste: bool,
        state: &'static str,
    }

    fn make_pad(project: &str, side: &str, spec: &PadSpec) -> Pad {
        Pad {
            key: crate::pads::pad_key(project, side, spec.ref_, spec.pin, spec.x, spec.y),
            project: project.to_string(),
            side: side.to_string(),
            ref_: spec.ref_.to_string(),
            pin: spec.pin.to_string(),
            x: spec.x,
            y: spec.y,
            function: spec.function.to_string(),
            shape: spec.shape.to_string(),
            flash: flash(spec.x, spec.y),
            geom: rect(spec.x, spec.y),
            has_paste: spec.has_paste,
            state: spec.state.to_string(),
            paste_indices: if spec.has_paste { vec![0] } else { vec![] },
        }
    }

    fn make_side(
        project: &str,
        name: &str,
        bbox: Bounds,
        mirror: bool,
        enabled: bool,
        specs: &[PadSpec],
        paste_objects: usize,
    ) -> Side {
        let paste = (paste_objects > 0).then(|| {
            let mut file = GerberFile::default();
            for i in 0..paste_objects {
                file.objects
                    .push(GraphicObject::Flash(flash(i as f64, 0.0)));
            }
            file
        });
        Side {
            project_name: project.to_string(),
            board_bbox: bbox,
            name: name.to_string(),
            copper: None,
            paste,
            mirror,
            enabled,
            pads: specs.iter().map(|s| make_pad(project, name, s)).collect(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    const fn spec(
        ref_: &'static str,
        pin: &'static str,
        x: f64,
        y: f64,
        function: &'static str,
        shape: &'static str,
        has_paste: bool,
        state: &'static str,
    ) -> PadSpec {
        PadSpec {
            ref_,
            pin,
            x,
            y,
            function,
            shape,
            has_paste,
            state,
        }
    }

    /// Two projects, three sides, nine pads; `alpha/bottom` is disabled.
    fn fixture() -> Vec<Project> {
        let alpha_top = make_side(
            "alpha",
            SIDE_TOP,
            (0.0, 0.0, 40.0, 30.0),
            false,
            true,
            &[
                spec(
                    "U1",
                    "1",
                    1.0,
                    1.0,
                    "SMDPad",
                    "R 1.00x0.50",
                    false,
                    STATE_UNDEFINED,
                ),
                spec(
                    "U1",
                    "2",
                    2.0,
                    1.0,
                    "SMDPad",
                    "R 1.00x0.50",
                    false,
                    STATE_UNDEFINED,
                ),
                spec(
                    "R2",
                    "1",
                    3.0,
                    2.0,
                    "SMDPad",
                    "C ⌀0.50",
                    false,
                    STATE_IGNORE,
                ),
                spec(
                    "TP1",
                    "1",
                    4.0,
                    2.0,
                    "TestPad",
                    "C ⌀1.00",
                    false,
                    STATE_IGNORE,
                ),
                spec(
                    "C3",
                    "1",
                    5.0,
                    3.0,
                    "SMDPad",
                    "R 0.80x0.60",
                    true,
                    STATE_OPEN,
                ),
                spec(
                    "C3",
                    "2",
                    6.0,
                    3.0,
                    "SMDPad",
                    "R 0.80x0.60",
                    true,
                    STATE_IGNORE,
                ),
            ],
            2,
        );
        let alpha_bottom = make_side(
            "alpha",
            SIDE_BOTTOM,
            (0.0, 0.0, 40.0, 30.0),
            true,
            false,
            &[spec(
                "J1",
                "1",
                7.0,
                4.0,
                "ConnectorPad",
                "R 2.00x1.00",
                false,
                STATE_UNDEFINED,
            )],
            1,
        );
        let beta_top = make_side(
            "beta",
            SIDE_TOP,
            (0.0, 0.0, 20.0, 15.0),
            false,
            true,
            &[
                spec(
                    "U1",
                    "1",
                    8.0,
                    5.0,
                    "SMDPad",
                    "R 1.00x0.50",
                    false,
                    STATE_UNDEFINED,
                ),
                spec("Q9", "3", 9.0, 6.0, "SMDPad", "C ⌀0.30", true, STATE_OPEN),
            ],
            1,
        );
        vec![
            Project {
                name: "alpha".to_string(),
                source: "alpha.zip".to_string(),
                outline: None,
                bbox: (0.0, 0.0, 40.0, 30.0),
                sides: vec![alpha_top, alpha_bottom],
            },
            Project {
                name: "beta".to_string(),
                source: "beta.zip".to_string(),
                outline: None,
                bbox: (0.0, 0.0, 20.0, 15.0),
                sides: vec![beta_top],
            },
        ]
    }

    fn pad_ids(projects: &[Project], sides: &[SideId]) -> Vec<PadId> {
        let mut out = Vec::new();
        for side in sides {
            for index in 0..side.get(projects).pads.len() {
                out.push((*side, index));
            }
        }
        out
    }

    /// A 400 x 300 mm block: it needs a sheet of at least that size.
    fn fake_layout(config: &Config) -> Layout {
        let (width, height) = config.sheet_size();
        let (bw, bh) = (400.0, 300.0);
        Layout {
            params: config.layout.clone(),
            areas: Vec::new(),
            width,
            height,
            block: (0.0, 0.0, bw, bh),
            fits: bw <= width && bh <= height,
            dots: Vec::new(),
            dividers: Vec::new(),
            heuristic: "test".to_string(),
            overflow: 0,
            dots_dropped: 0,
        }
    }

    macro_rules! make_app {
        ($app:ident) => {
            make_app!($app, |_cfg: &mut Config| {});
        };
        ($app:ident, $prep:expr) => {
            let mut projects = fixture();
            let sides = all_sides(&projects);
            let order = pad_ids(&projects, &sides);
            let mut config = Config::default();
            ($prep)(&mut config);
            let mut on_preview =
                |_: &[Project], _: &Config, sel: Option<PadId>, open: bool| -> String {
                    format!(
                        "/tmp/preview-{}-{}.png",
                        if open { "open" } else { "quiet" },
                        sel.map(|s| s.1.to_string())
                            .unwrap_or_else(|| "none".into())
                    )
                };
            let mut compute_layout = |_: &[Project], cfg: &Config| -> Layout { fake_layout(cfg) };
            let hooks = TuiHooks {
                on_preview: &mut on_preview,
                compute_layout: &mut compute_layout,
            };
            #[allow(unused_mut)]
            let mut $app = App::new(&mut projects, &sides, order, &mut config, hooks, "stencil");
            $app.refresh();
        };
    }

    /// Like [`make_app`], but with the real packer behind `compute_layout` so
    /// the split view has `Area`s (and therefore sheet coordinates) to work in.
    macro_rules! make_packed_app {
        ($app:ident) => {
            let mut projects = fixture();
            let sides = all_sides(&projects);
            let order = pad_ids(&projects, &sides);
            let mut config = Config::default();
            config.size = (600, 600);
            let mut on_preview = |_: &[Project], _: &Config, _: Option<PadId>, _: bool| -> String {
                "/tmp/preview.png".to_string()
            };
            let mut compute_layout = |ps: &[Project], cfg: &Config| -> Layout {
                let on: Vec<SideId> = all_sides(ps)
                    .into_iter()
                    .filter(|id| id.get(ps).enabled)
                    .collect();
                crate::layout::pack(ps, &on, cfg)
            };
            let hooks = TuiHooks {
                on_preview: &mut on_preview,
                compute_layout: &mut compute_layout,
            };
            #[allow(unused_mut)]
            let mut $app = App::new(&mut projects, &sides, order, &mut config, hooks, "stencil");
            $app.refresh();
        };
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ch(c: char) -> KeyEvent {
        key(KeyCode::Char(c))
    }

    fn send(app: &mut App<'_, '_>, keys: &str) {
        for c in keys.chars() {
            app.handle_key(ch(c));
        }
    }

    fn render(app: &mut App<'_, '_>, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        app.before_draw();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn line(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    fn modifier(buf: &Buffer, x: u16, y: u16) -> Modifier {
        buf[(x, y)].modifier
    }

    /// Columns `x0 .. x1` of row `y`, trailing blanks trimmed.
    fn band(buf: &Buffer, y: u16, x0: u16, x1: u16) -> String {
        (x0..x1.min(buf.area.width))
            .map(|x| buf[(x, y)].symbol())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    /// `(symbol, fg, bg)` of one cell.
    fn cell(buf: &Buffer, x: u16, y: u16) -> (String, Color, Color) {
        let c = &buf[(x, y)];
        (c.symbol().to_string(), c.fg, c.bg)
    }

    /// How many *picture* cells of the band carry `class` as foreground or
    /// background (the caption and the legend rows are left out).
    fn colour_count(buf: &Buffer, x0: u16, x1: u16, class: Class) -> usize {
        let want = ascii::class_color(class);
        let last = buf
            .area
            .height
            .saturating_sub(u16::from(buf.area.height >= LEGEND_MIN_HEIGHT));
        let mut n = 0;
        for y in 1..last {
            for x in x0..x1.min(buf.area.width) {
                let c = &buf[(x, y)];
                if c.symbol() != " " && (c.fg == want || c.bg == want) {
                    n += 1;
                }
            }
        }
        n
    }

    /// The pad label of every row of the (filtered) pads page.
    fn pad_labels(app: &App<'_, '_>) -> Vec<String> {
        app.page_pads()
            .iter()
            .map(|id| app.pad(*id).label())
            .collect()
    }

    fn side_labels(app: &App<'_, '_>) -> Vec<String> {
        app.page_sides()
            .iter()
            .map(|id| app.side(*id).label())
            .collect()
    }

    // ---------------------------------------------------------------- //
    // pure helpers
    // ---------------------------------------------------------------- //

    #[test]
    fn states_cycle_and_are_checked() {
        assert_eq!(cycle_state(STATE_UNDEFINED), STATE_OPEN);
        assert_eq!(cycle_state(STATE_OPEN), STATE_IGNORE);
        assert_eq!(cycle_state(STATE_IGNORE), STATE_OPEN);

        let projects = fixture();
        let candidate = pad_of(&projects, (A_TOP, 0));
        let pasted_open = pad_of(&projects, (A_TOP, 4));
        let pasted_closed = pad_of(&projects, (A_TOP, 5));
        assert_eq!(next_state(candidate), STATE_OPEN);
        assert_eq!(next_state(pasted_open), STATE_IGNORE);
        assert_eq!(next_state(pasted_closed), STATE_OPEN);
        assert!(allowed_state(candidate, STATE_UNDEFINED));
        assert!(!allowed_state(pasted_open, STATE_UNDEFINED));
        assert!(allowed_state(pasted_open, STATE_IGNORE));
        assert!(pasted_closed.is_closed());
    }

    #[test]
    fn counts_separate_candidates_from_pasted_pads() {
        let projects = fixture();
        let sides = all_sides(&projects);
        let all = pad_ids(&projects, &sides);
        let enabled = visible_pads(&projects, &all, &sides, true);
        let counts = count_states(enabled.iter().map(|id| pad_of(&projects, *id)));
        assert_eq!(
            counts,
            StateCounts {
                undefined: 3,
                open: 0,
                ignore: 2,
                closed: 1
            }
        );
    }

    #[test]
    fn visible_pads_follow_the_enabled_sides() {
        let projects = fixture();
        let sides = all_sides(&projects);
        let all = pad_ids(&projects, &sides);
        assert_eq!(all.len(), 9);
        let candidates = visible_pads(&projects, &all, &sides, false);
        assert_eq!(
            candidates,
            vec![(A_TOP, 0), (A_TOP, 1), (A_TOP, 2), (A_TOP, 3), (B_TOP, 0)]
        );
        // the disabled alpha/bottom pad is never listed
        assert!(!candidates.contains(&(A_BOT, 0)));
        assert_eq!(visible_pads(&projects, &all, &sides, true).len(), 8);
    }

    #[test]
    fn footer_help_wraps_onto_two_lines_at_80_and_one_at_150() {
        let items = page_items(PAGE_PADS, false);
        let wide = wrap_items(&items, 150, FOOTER_LINES);
        assert_eq!(wide.len(), 1);
        assert_eq!(wide[0], items.join(ITEM_SEP));

        let narrow = wrap_items(&items, 80, FOOTER_LINES);
        assert_eq!(narrow.len(), 2);
        for row in &narrow {
            assert!(row.chars().count() <= 80, "{row:?}");
        }
        assert!(narrow[0].starts_with("↑↓/jk move  / search"));
        assert!(narrow[1].ends_with("q quit"));
        // nothing is lost
        let joined = format!("{}{ITEM_SEP}{}", narrow[0], narrow[1]);
        assert_eq!(joined, items.join(ITEM_SEP));
    }

    #[test]
    fn footer_help_truncates_when_two_lines_are_not_enough() {
        let items = page_items(PAGE_PADS, false);
        let tiny = wrap_items(&items, 24, FOOTER_LINES);
        assert_eq!(tiny.len(), 2);
        assert!(tiny[1].ends_with('…'), "{tiny:?}");
        for row in &tiny {
            assert!(row.chars().count() <= 24);
        }
        assert!(wrap_items(&items, 0, 2).is_empty());
        assert_eq!(wrap_text("a status message", 6, 2), vec!["a", "statu…"]);
    }

    #[test]
    fn filter_item_sits_behind_the_search_item() {
        let items = page_items(PAGE_PADS, true);
        let index = items.iter().position(|i| i == FILTER_ITEM).unwrap();
        assert_eq!(items[index - 1], "/ search");
        // a page without a search item gets it appended
        let stencil = page_items(PAGE_STENCIL, true);
        assert_eq!(stencil.last().map(String::as_str), Some(FILTER_ITEM));
    }

    #[test]
    fn search_words_are_deduplicated_and_or_ed() {
        assert_eq!(query_words("U1 u1  Top"), vec!["u1", "top"]);
        let projects = fixture();
        let fields = pad_fields(pad_of(&projects, (A_TOP, 0)));
        assert_eq!(match_count(&fields, "u1 alpha"), 2);
        assert_eq!(match_count(&fields, "u1 nothing"), 1);
        assert_eq!(match_count(&fields, ""), 0);
        assert!(matches(&fields, ""));
        assert!(matches(&fields, "alpha zzz"));
        assert!(!matches(&fields, "zzz"));
        // a pasted pad answers to "paste" and, once closed, to "closed"
        assert!(matches(&pad_fields(pad_of(&projects, (A_TOP, 4))), "paste"));
        assert!(matches(
            &pad_fields(pad_of(&projects, (A_TOP, 5))),
            "closed"
        ));
    }

    #[test]
    fn filtering_ranks_the_best_matches_first() {
        let projects = fixture();
        let sides = all_sides(&projects);
        let rows = visible_pads(&projects, &pad_ids(&projects, &sides), &sides, false);
        let pairs = filter_counts(&rows, |id| pad_fields(pad_of(&projects, *id)), "u1 alpha");
        let labels: Vec<String> = pairs
            .iter()
            .map(|(id, _)| pad_of(&projects, *id).label())
            .collect();
        assert_eq!(labels, vec!["U1.1", "U1.2", "R2.1", "TP1.1", "U1.1"]);
        let counts: Vec<usize> = pairs.iter().map(|(_, c)| *c).collect();
        assert_eq!(counts, vec![2, 2, 1, 1, 1]);
        assert_eq!(full_matches(&counts, "u1 alpha"), 2);
        assert_eq!(full_matches(&counts, "u1"), 5);
        assert_eq!(match_summary(5, 5, 2, 2), "(5 of 5, 2 full)");
        assert_eq!(match_summary(5, 5, 5, 1), "(5 of 5)");
        assert_eq!(match_summary(0, 5, 0, 1), "no match");
        // a blank query keeps everything, in order
        assert_eq!(
            filter_rows(&rows, |id| pad_fields(pad_of(&projects, *id)), "  "),
            rows
        );
    }

    #[test]
    fn search_and_escape_actions() {
        assert_eq!(search_action(key(KeyCode::Enter)), SearchAction::Keep);
        assert_eq!(search_action(key(KeyCode::Esc)), SearchAction::Clear);
        assert_eq!(
            search_action(key(KeyCode::Backspace)),
            SearchAction::Backspace
        );
        assert_eq!(search_action(key(KeyCode::Up)), SearchAction::Up);
        assert_eq!(search_action(ch('j')), SearchAction::Char);
        assert_eq!(search_action(ch(' ')), SearchAction::Char);
        assert_eq!(search_action(key(KeyCode::F(3))), SearchAction::Ignore);
        assert_eq!(search_char(ch('/')), Some('/'));
        assert_eq!(search_char(key(KeyCode::Tab)), None);

        assert_eq!(escape_action(true, true, true, true), EscAction::Edit);
        assert_eq!(escape_action(false, true, true, true), EscAction::Search);
        assert_eq!(escape_action(false, false, true, true), EscAction::Pending);
        assert_eq!(escape_action(false, false, false, true), EscAction::Filter);
        assert_eq!(escape_action(false, false, false, false), EscAction::None);
    }

    #[test]
    fn restore_index_follows_identity() {
        let rows = vec![(A_TOP, 0), (A_TOP, 1), (B_TOP, 0)];
        assert_eq!(restore_index(&rows, Some(&(B_TOP, 0)), 0), 2);
        assert_eq!(restore_index(&rows, Some(&(A_BOT, 0)), 1), 1);
        assert_eq!(restore_index(&rows, Some(&(A_BOT, 0)), 99), 2);
        assert_eq!(restore_index::<PadId>(&[], Some(&(A_TOP, 0)), 7), 0);
    }

    #[test]
    fn pad_rows_have_the_python_columns() {
        let projects = fixture();
        let pad = pad_of(&projects, (A_TOP, 0));
        let expected = format!(
            "  ? {:<9} {}{:<12} {:<20} {:<7} {:<13} {:<16} x={:8.3} y={:8.3}",
            "undefined", "  ", "U1.1", "alpha", "top", "SMDPad", "R 1.00x0.50", 1.0, 1.0
        );
        assert_eq!(format_row(pad, false, None), expected);
        assert_eq!(format_row(pad, true, None), format!("→ {}", &expected[2..]));
        assert_eq!(format_row(pad, false, Some(10)), "  ? undefi");

        // a pad that already has paste is marked with "· " and toggles only
        let pasted = pad_of(&projects, (A_TOP, 4));
        let expected = format!(
            "  + {:<9} {}{:<12} {:<20} {:<7} {:<13} {:<16} x={:8.3} y={:8.3}",
            "open", PASTE_MARK, "C3.1", "alpha", "top", "SMDPad", "R 0.80x0.60", 5.0, 3.0
        );
        assert_eq!(format_row(pasted, false, None), expected);
        let kinds: Vec<SegKind> = row_segments(pasted, false)
            .iter()
            .map(|(_, k)| *k)
            .collect();
        assert_eq!(
            kinds,
            vec![
                SegKind::Plain,
                SegKind::State,
                SegKind::PadLabel,
                SegKind::Plain
            ]
        );

        assert_eq!(
            pads_header(),
            format!(
                "  {:<11} {}{:<12} {:<20} {:<7} {:<13} {:<16} position",
                "state", "  ", "pad", "project", "side", "function", "shape"
            )
        );
        // an empty function/shape becomes "-"
        let mut bare = pad.clone();
        bare.function.clear();
        bare.shape.clear();
        assert!(format_row(&bare, false, None).contains("- "));
    }

    #[test]
    fn side_rows_and_presets() {
        let projects = fixture();
        let side = A_TOP.get(&projects);
        assert_eq!(
            side_row(side, false),
            format!(
                "  [x] {:<30} {:7.1} x {:7.1} mm   paste {:<6} pads to decide {:<6} undefined {}",
                "alpha · top", 40.0, 30.0, 2, 4, 2
            )
        );
        let disabled = A_BOT.get(&projects);
        assert!(side_row(disabled, true).starts_with("→ [ ] alpha · bottom (mirrored)"));

        let config = Config::default();
        let row = preset_row((380, 280), [Some(true), Some(false)], &config, false);
        assert_eq!(
            row,
            format!(
                "  ● 380 x 280 mm   {:<30}{:<30}  ← landscape",
                "landscape: 380 x 280  fits", "portrait: 280 x 380  NO"
            )
        );
        let other = preset_row((600, 600), [None, None], &config, true);
        assert!(other.starts_with("→   600 x 600 mm"));
        assert!(other.contains("landscape: 600 x 600  ?"));
        assert_eq!(orientation_size((380, 280), "portrait"), (280, 380));
    }

    #[test]
    fn the_fit_line_reads_like_the_python_one() {
        let config = Config::default();
        let layout = fake_layout(&config);
        let counts = StateCounts {
            undefined: 3,
            open: 0,
            ignore: 2,
            closed: 1,
        };
        assert_eq!(
            fit_line(&config, Some(&layout), counts),
            "stencil 380x280 landscape · block 400.0 x 300.0 mm · DOES NOT FIT \
             · pads: undefined 3  open 0  ignore 2  closed 1"
        );
        let kinds: Vec<Kind> = fit_segments(&config, Some(&layout), counts)
            .iter()
            .map(|(_, k)| *k)
            .collect();
        assert_eq!(
            kinds,
            vec![Kind::Plain, Kind::Plain, Kind::NoFit, Kind::Plain]
        );
        assert!(fit_line(&config, None, counts).contains("block ? · NO LAYOUT"));

        let big = Config {
            size: (600, 600),
            ..Config::default()
        };
        let layout = fake_layout(&big);
        assert!(fit_line(&big, Some(&layout), counts).contains("· FITS ·"));
    }

    #[test]
    fn layout_fields_cover_every_row_including_dot_clearance() {
        let labels: Vec<&str> = LAYOUT_FIELDS.iter().map(|f| f.label).collect();
        assert_eq!(labels.len(), 19);
        assert_eq!(labels[0], "spacing (gap between boards)");
        assert_eq!(labels[1], "datum (alignment features)");
        assert_eq!(labels[14], "dotted line gap (between touching cells)");
        assert_eq!(labels[15], "dot clearance (to slot/hole/marker)");
        assert_eq!(labels[16], "hole grid (holes datum, 0 = off)");
        assert_eq!(labels[18], "sort");
        let clearance = LAYOUT_FIELDS[15];
        assert_eq!(clearance.attr, Attr::DotClearance);
        assert_eq!(clearance.step, 0.1);
        assert_eq!(clearance.minimum, Some(0.0));
        assert_eq!(clearance.rule, Rule::Ge0);
    }

    #[test]
    fn fields_step_toggle_and_validate() {
        let mut params = LayoutParams::default();
        let gap = LAYOUT_FIELDS[0];
        assert_eq!(step_value(&gap, 30.0, 1), 30.5);
        assert_eq!(step_value(&gap, 0.2, -1), 0.0); // clamped to the minimum
        assert_eq!(value_text(&gap, &params), "30 mm");

        let clearance = LAYOUT_FIELDS[15];
        assert_eq!(step_value(&clearance, 0.5, 1), 0.6);
        assert_eq!(step_value(&clearance, 0.05, -1), 0.0);
        assert!(valid_value(&clearance, 0.0));
        assert!(!valid_value(&clearance, -0.1));
        assert!(!valid_value(&clearance, f64::NAN));

        let dia = LAYOUT_FIELDS[12];
        assert!(!valid_value(&dia, 0.0));

        let datum = LAYOUT_FIELDS[1];
        toggle_field(&datum, &mut params);
        assert_eq!(params.datum, DATUM_HOLES);
        toggle_field(&datum, &mut params);
        assert_eq!(params.datum, DATUM_NONE);
        toggle_field_back(&datum, &mut params);
        assert_eq!(params.datum, DATUM_HOLES);

        let marker = LAYOUT_FIELDS[10];
        assert_eq!(value_text(&marker, &params), "on");
        toggle_field(&marker, &mut params);
        assert_eq!(value_text(&marker, &params), "off");

        assert_eq!(parse_number(" 2.5 "), Some(2.5));
        assert_eq!(parse_number("2,5"), None);
        assert_eq!(parse_number("inf"), None);

        assert_eq!(edit_buffer("1", ch('2')).as_deref(), Some("12"));
        assert_eq!(
            edit_buffer("12", key(KeyCode::Backspace)).as_deref(),
            Some("1")
        );
        assert_eq!(edit_buffer("1", ch('x')), None);
        assert_eq!(edit_buffer("1", ch('.')).as_deref(), Some("1."));

        assert_eq!(
            format_field_row(&gap, &params, true, Some("3.")),
            format!("→ {:<38} 3._", gap.label)
        );
    }

    #[test]
    fn datum_rows_dim_but_stay_editable() {
        let mut params = LayoutParams::default(); // slots
        assert!(field_applies(&LAYOUT_FIELDS[4], &params)); // slot width
        assert!(!field_applies(&LAYOUT_FIELDS[2], &params)); // hole diameter
        params.datum = DATUM_HOLES.to_string();
        assert!(!field_applies(&LAYOUT_FIELDS[4], &params));
        assert!(field_applies(&LAYOUT_FIELDS[2], &params));
        params.datum = DATUM_NONE.to_string();
        assert!(!field_applies(&LAYOUT_FIELDS[4], &params));
        assert!(!field_applies(&LAYOUT_FIELDS[2], &params));
        // rows that belong to no datum always apply
        assert!(field_applies(&LAYOUT_FIELDS[15], &params));
    }

    // ---------------------------------------------------------------- //
    // the app: navigation
    // ---------------------------------------------------------------- //

    #[test]
    fn the_app_opens_on_the_pads_page() {
        make_app!(app);
        assert_eq!(app.page(), PAGE_PADS);
        assert_eq!(
            pad_labels(&app),
            vec!["U1.1", "U1.2", "R2.1", "TP1.1", "U1.1"]
        );
        assert_eq!(app.index(), 0);
    }

    #[test]
    fn movement_keys_walk_every_page() {
        make_app!(app);
        render(&mut app, 120, 20); // fixes the page step at 15
        send(&mut app, "jj");
        assert_eq!(app.index(), 2);
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.index(), 1);
        send(&mut app, "G");
        assert_eq!(app.index(), 4);
        send(&mut app, "g");
        assert_eq!(app.index(), 0);
        app.handle_key(key(KeyCode::End));
        assert_eq!(app.index(), 4);
        app.handle_key(key(KeyCode::Home));
        assert_eq!(app.index(), 0);
        app.handle_key(key(KeyCode::PageDown));
        assert_eq!(app.index(), 4); // clamped
        app.handle_key(key(KeyCode::PageUp));
        assert_eq!(app.index(), 0);

        // every page: 1-4, Tab and Shift-Tab
        send(&mut app, "4");
        assert_eq!(app.page(), PAGE_LAYOUT);
        send(&mut app, "G");
        assert_eq!(app.index(), LAYOUT_FIELDS.len() - 1);
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.page(), PAGE_PADS);
        app.handle_key(key(KeyCode::BackTab));
        assert_eq!(app.page(), PAGE_LAYOUT);
        // some terminals report Shift-Tab as Tab + SHIFT
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT));
        assert_eq!(app.page(), PAGE_STENCIL);
        app.handle_key(key(KeyCode::Tab));
        assert_eq!(app.page(), PAGE_LAYOUT);
        send(&mut app, "3");
        assert_eq!(app.page(), PAGE_STENCIL);
        send(&mut app, "G");
        assert_eq!(app.index(), STENCIL_SIZES.len() - 1);
        send(&mut app, "2");
        assert_eq!(app.page(), PAGE_SIDES);
        send(&mut app, "G");
        assert_eq!(app.index(), 2);
    }

    // ---------------------------------------------------------------- //
    // the app: pad states
    // ---------------------------------------------------------------- //

    #[test]
    fn space_cycles_a_candidate_and_o_i_set_it() {
        make_app!(app);
        send(&mut app, " ");
        assert_eq!(app.pad((A_TOP, 0)).state, STATE_OPEN);
        send(&mut app, " ");
        assert_eq!(app.pad((A_TOP, 0)).state, STATE_IGNORE);
        send(&mut app, " ");
        assert_eq!(app.pad((A_TOP, 0)).state, STATE_OPEN);
        send(&mut app, "i");
        assert_eq!(app.pad((A_TOP, 0)).state, STATE_IGNORE);
        send(&mut app, "o");
        assert_eq!(app.pad((A_TOP, 0)).state, STATE_OPEN);
        assert!(app.status().starts_with("U1.1 (alpha top) -> open"));
    }

    #[test]
    fn a_applies_the_state_to_the_whole_component() {
        make_app!(app);
        send(&mut app, "o"); // U1.1 -> open
        send(&mut app, "a");
        assert_eq!(app.pad((A_TOP, 0)).state, STATE_OPEN);
        assert_eq!(app.pad((A_TOP, 1)).state, STATE_OPEN); // U1.2, same ref
        assert_eq!(app.pad((B_TOP, 0)).state, STATE_UNDEFINED); // other project
        assert_eq!(app.status(), "U1: 2 pad(s) -> open");
    }

    #[test]
    fn n_and_shift_n_walk_the_undefined_candidates() {
        make_app!(app);
        // rows: U1.1 undef, U1.2 undef, R2.1 ignore, TP1.1 ignore, U1.1(beta) undef
        send(&mut app, "n");
        assert_eq!(app.index(), 1);
        send(&mut app, "n");
        assert_eq!(app.index(), 4);
        send(&mut app, "n"); // wraps
        assert_eq!(app.index(), 0);
        send(&mut app, "N");
        assert_eq!(app.index(), 4);
        assert_eq!(app.status(), "undefined pad 5/5");
        // once nothing is undefined any more
        send(&mut app, "goaGoa");
        send(&mut app, "jjo");
        send(&mut app, "n");
        assert_eq!(app.status(), "no undefined pads left");
    }

    #[test]
    fn pasted_pads_only_toggle_open_and_ignore() {
        make_app!(app);
        send(&mut app, "*");
        assert!(app.show_all());
        assert_eq!(
            app.subheader(),
            "pads 1/8   all pads (8) — pads with paste are dimmed; ignore closes their opening   \
             * = back to pads to decide   (1 hidden on disabled sides)"
        );
        // all pads: U1.1 U1.2 R2.1 TP1.1 C3.1 C3.2 U1.1(beta) Q9.3
        assert_eq!(app.page_pads().len(), 8);
        send(&mut app, "jjjj"); // C3.1, open + paste
        assert_eq!(app.current_pad(), Some((A_TOP, 4)));
        send(&mut app, " ");
        assert_eq!(app.pad((A_TOP, 4)).state, STATE_IGNORE);
        assert!(app.status().ends_with("paste opening closed"));
        send(&mut app, " ");
        assert_eq!(app.pad((A_TOP, 4)).state, STATE_OPEN);
        assert!(app.status().ends_with("paste opening kept"));
        // it may never become undefined
        app.set_state(STATE_UNDEFINED);
        assert_eq!(app.pad((A_TOP, 4)).state, STATE_OPEN);
        assert_eq!(app.status(), "C3.1 has paste: it is only open or ignore");
    }

    #[test]
    fn star_keeps_the_cursor_on_its_pad() {
        make_app!(app);
        send(&mut app, "jjj"); // TP1.1
        let before = app.current_pad();
        assert_eq!(before, Some((A_TOP, 3)));
        send(&mut app, "*");
        assert_eq!(app.current_pad(), before);
        assert_eq!(app.index(), 3);
        send(&mut app, "*");
        assert_eq!(app.current_pad(), before);
        assert_eq!(app.index(), 3);
    }

    // ---------------------------------------------------------------- //
    // the app: sides
    // ---------------------------------------------------------------- //

    #[test]
    fn sides_toggle_and_the_pad_list_follows() {
        make_app!(app);
        send(&mut app, "2");
        assert_eq!(
            side_labels(&app),
            vec!["alpha top", "alpha bottom", "beta top"]
        );
        send(&mut app, " "); // alpha/top off
        assert!(!app.side(A_TOP).enabled);
        send(&mut app, "1");
        assert_eq!(pad_labels(&app), vec!["U1.1"]); // only beta/top is left
        send(&mut app, "2A");
        assert!(app.side(A_BOT).enabled);
        send(&mut app, "1");
        assert_eq!(pad_labels(&app).len(), 6);
        send(&mut app, "2N");
        assert!(!app.side(B_TOP).enabled);
        send(&mut app, "1");
        assert!(pad_labels(&app).is_empty());
    }

    #[test]
    fn enter_toggles_the_side_under_the_cursor_even_when_filtered() {
        make_app!(app);
        send(&mut app, "2/");
        send(&mut app, "beta");
        app.handle_key(key(KeyCode::Enter)); // keep the filter
        assert_eq!(side_labels(&app), vec!["beta top"]);
        assert_eq!(app.index(), 0);
        // index 0 of the *filtered* list is beta/top, not alpha/top
        app.handle_key(key(KeyCode::Enter));
        assert!(!app.side(B_TOP).enabled);
        assert!(app.side(A_TOP).enabled);
        assert_eq!(app.status(), "beta top: disabled");
    }

    // ---------------------------------------------------------------- //
    // the app: search and filters
    // ---------------------------------------------------------------- //

    #[test]
    fn search_filters_live_and_keeps_the_cursor() {
        make_app!(app);
        send(&mut app, "jjjj"); // U1.1 of beta
        assert_eq!(app.current_pad(), Some((B_TOP, 0)));
        send(&mut app, "/");
        assert_eq!(app.search(), Some(""));
        send(&mut app, "u1");
        assert_eq!(pad_labels(&app), vec!["U1.1", "U1.2", "U1.1"]);
        assert_eq!(app.current_pad(), Some((B_TOP, 0))); // followed by identity
        assert_eq!(app.index(), 2);
        assert_eq!(app.subheader(), "filter: u1  (3 of 5)");

        // backspace widens the filter again
        app.handle_key(key(KeyCode::Backspace));
        app.handle_key(key(KeyCode::Backspace));
        assert_eq!(app.search(), Some(""));
        assert_eq!(pad_labels(&app).len(), 5);
        send(&mut app, "u1");
        assert_eq!(pad_labels(&app), vec!["U1.1", "U1.2", "U1.1"]);

        // Enter keeps the filter and normal keys work on the rows that are left
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.search(), None);
        assert_eq!(app.active_filter(), Some("u1"));
        assert_eq!(app.current_pad(), Some((B_TOP, 0)));
        send(&mut app, "o");
        assert_eq!(app.pad((B_TOP, 0)).state, STATE_OPEN);

        // Esc clears it and the cursor goes back to the same pad
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.active_filter(), None);
        assert_eq!(app.status(), "filter cleared");
        assert_eq!(app.current_pad(), Some((B_TOP, 0)));
        assert_eq!(app.index(), 4);
    }

    #[test]
    fn the_search_ranks_and_reports_full_matches() {
        make_app!(app);
        send(&mut app, "/");
        send(&mut app, "u1 alpha");
        assert_eq!(
            pad_labels(&app),
            vec!["U1.1", "U1.2", "R2.1", "TP1.1", "U1.1"]
        );
        assert_eq!(app.subheader(), "filter: u1 alpha  (5 of 5, 2 full)");
        assert!(app.is_full(0));
        assert!(app.is_full(1));
        assert!(!app.is_full(2));

        // the words are OR-ed: one more never narrows the list
        send(&mut app, " zzz");
        assert_eq!(pad_labels(&app).len(), 5);
        assert_eq!(app.subheader(), "filter: u1 alpha zzz  (5 of 5, 0 full)");

        // a query nothing matches at all
        app.handle_key(key(KeyCode::Esc));
        send(&mut app, "/");
        send(&mut app, "zzz");
        assert!(pad_labels(&app).is_empty());
        assert_eq!(app.subheader(), "filter: zzz  no match");
    }

    #[test]
    fn escape_out_of_a_search_that_matched_nothing_returns_to_the_anchor() {
        make_app!(app);
        send(&mut app, "jj"); // R2.1
        send(&mut app, "/");
        send(&mut app, "zzz");
        assert!(pad_labels(&app).is_empty());
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.active_filter(), None);
        assert_eq!(app.current_pad(), Some((A_TOP, 2)));
    }

    #[test]
    fn a_new_search_starts_from_scratch() {
        make_app!(app);
        send(&mut app, "/");
        send(&mut app, "u1");
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.active_filter(), Some("u1"));
        send(&mut app, "/"); // fresh: the old filter is dropped at once
        assert_eq!(app.search(), Some(""));
        assert_eq!(app.active_filter(), None);
        assert_eq!(pad_labels(&app).len(), 5);
    }

    #[test]
    fn every_page_keeps_its_own_filter() {
        make_app!(app);
        send(&mut app, "/");
        send(&mut app, "tp");
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(pad_labels(&app), vec!["TP1.1"]);

        send(&mut app, "2/");
        send(&mut app, "bottom");
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(side_labels(&app), vec!["alpha bottom"]);

        send(&mut app, "1");
        assert_eq!(app.active_filter(), Some("tp"));
        assert_eq!(pad_labels(&app), vec!["TP1.1"]);
        send(&mut app, "2");
        assert_eq!(app.active_filter(), Some("bottom"));
        assert_eq!(side_labels(&app), vec!["alpha bottom"]);
        // the layout page cannot be searched at all
        send(&mut app, "4/");
        assert_eq!(app.search(), None);
        assert_eq!(app.index(), 0);
    }

    #[test]
    fn a_filter_survives_star_and_a_side_toggle() {
        make_app!(app);
        send(&mut app, "/");
        send(&mut app, "c3");
        app.handle_key(key(KeyCode::Enter));
        assert!(pad_labels(&app).is_empty()); // C3 has paste: not a candidate
        send(&mut app, "*");
        assert_eq!(pad_labels(&app), vec!["C3.1", "C3.2"]);
        assert_eq!(app.active_filter(), Some("c3"));

        send(&mut app, "2 "); // alpha/top off
        send(&mut app, "1");
        assert!(pad_labels(&app).is_empty());
        assert_eq!(app.active_filter(), Some("c3"));
    }

    // ---------------------------------------------------------------- //
    // the app: generate, escape, preview, quit
    // ---------------------------------------------------------------- //

    #[test]
    fn w_needs_a_second_press_when_the_layout_does_not_fit() {
        make_app!(app);
        assert!(!app.layout().unwrap().fits);
        assert_eq!(app.handle_key(ch('w')), Action::Continue);
        assert_eq!(app.status(), NOFIT_WARNING);
        assert_eq!(app.pending(), Some('w'));
        assert_eq!(app.handle_key(ch('w')), Action::Generate);

        // any other key cancels the pending confirmation
        assert_eq!(app.handle_key(ch('w')), Action::Continue);
        assert_eq!(app.handle_key(ch('j')), Action::Continue);
        assert_eq!(app.pending(), None);
        assert_eq!(app.handle_key(ch('w')), Action::Continue);
        assert_eq!(app.status(), NOFIT_WARNING);
    }

    #[test]
    fn w_generates_at_once_when_the_layout_fits() {
        make_app!(app, |cfg: &mut Config| cfg.size = (600, 600));
        assert!(app.layout().unwrap().fits);
        assert_eq!(app.handle_key(ch('w')), Action::Generate);
        // and on every page
        send(&mut app, "4");
        assert_eq!(app.handle_key(ch('w')), Action::Generate);
        send(&mut app, "2");
        assert_eq!(app.handle_key(ch('w')), Action::Generate);
    }

    #[test]
    fn escape_cancels_in_order_and_never_quits() {
        make_app!(app);
        // 1. nothing at all
        assert_eq!(app.handle_key(key(KeyCode::Esc)), Action::Continue);
        assert_eq!(app.status(), "");

        // 2. a pending generate
        app.handle_key(ch('w'));
        assert_eq!(app.pending(), Some('w'));
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.status(), "generate cancelled");
        assert_eq!(app.pending(), None);

        // 3. an active filter
        send(&mut app, "/");
        send(&mut app, "u1");
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.active_filter(), Some("u1"));
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.active_filter(), None);

        // 4. an inline edit wins over everything
        send(&mut app, "4e");
        assert_eq!(app.editing(), Some("30"));
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.editing(), None);
        assert_eq!(app.status(), "edit cancelled");
        assert_eq!(app.handle_key(key(KeyCode::Esc)), Action::Continue);
    }

    #[test]
    fn q_quits_and_ctrl_c_too() {
        make_app!(app);
        assert_eq!(app.handle_key(ch('q')), Action::Quit);
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::Quit
        );
    }

    #[test]
    fn p_renders_and_opens_the_preview_while_v_does_not() {
        make_app!(app);
        send(&mut app, "jj"); // R2.1
        app.handle_key(ch('p'));
        assert_eq!(app.status(), "rendering…");
        assert!(app.run_preview());
        // `p` always opens the viewer now; there is no render-only key left
        assert_eq!(app.status(), "preview opened: /tmp/preview-open-2.png");
        assert!(!app.run_preview());

        // `v` is the split view and never calls the hook
        app.handle_key(ch('v'));
        assert!(app.split());
        assert!(!app.run_preview());
        app.handle_key(ch('v'));
        assert!(!app.split());

        // no pad is highlighted from another page
        send(&mut app, "3p");
        app.run_preview();
        assert_eq!(app.status(), "preview opened: /tmp/preview-open-none.png");
    }

    // ---------------------------------------------------------------- //
    // the app: stencil and layout pages
    // ---------------------------------------------------------------- //

    #[test]
    fn the_stencil_page_picks_a_size_and_flips_the_orientation() {
        make_app!(app);
        send(&mut app, "3");
        app.before_draw();
        assert!(app.presets.is_some());
        send(&mut app, "G "); // the biggest preset
        assert_eq!(app.config.size, (700, 600));
        assert!(app.layout().unwrap().fits);
        send(&mut app, "o");
        assert_eq!(app.config.orientation, "portrait");
        send(&mut app, "o");
        assert_eq!(app.config.orientation, "landscape");
        assert!(app
            .subheader()
            .starts_with("sheet 700.0 x 600.0 mm   orientation landscape"));
    }

    #[test]
    fn layout_rows_step_toggle_and_edit() {
        make_app!(app);
        send(&mut app, "4");
        assert_eq!(app.config.layout.gap, 30.0);
        send(&mut app, "+");
        assert_eq!(app.config.layout.gap, 30.5);
        send(&mut app, "-");
        assert_eq!(app.config.layout.gap, 30.0);
        app.handle_key(key(KeyCode::Right));
        assert_eq!(app.config.layout.gap, 30.5);
        app.handle_key(key(KeyCode::Left));
        assert_eq!(app.config.layout.gap, 30.0);
        send(&mut app, " ");
        assert!(app.status().ends_with("press e or enter to edit"));

        // the datum is a choice: space cycles it, '-' goes back
        send(&mut app, "j ");
        assert_eq!(app.config.layout.datum, DATUM_HOLES);
        send(&mut app, "-");
        assert_eq!(app.config.layout.datum, DATUM_SLOTS);

        // dot clearance steps by 0.1 and stops at 0
        send(&mut app, "G");
        for _ in 0..3 {
            app.handle_key(key(KeyCode::Up));
        }
        assert_eq!(LAYOUT_FIELDS[app.index()].attr, Attr::DotClearance);
        send(&mut app, "+");
        assert_eq!(app.config.layout.dot_clearance, 0.6);
        send(&mut app, "-----------");
        assert_eq!(app.config.layout.dot_clearance, 0.0);
    }

    #[test]
    fn inline_editing_replaces_the_prefill_and_validates() {
        make_app!(app);
        send(&mut app, "4e");
        assert_eq!(app.editing(), Some("30")); // prefilled
        send(&mut app, "7"); // the first keystroke replaces it
        assert_eq!(app.editing(), Some("7"));
        send(&mut app, ".5");
        assert_eq!(app.editing(), Some("7.5"));
        app.handle_key(key(KeyCode::Backspace));
        assert_eq!(app.editing(), Some("7."));
        send(&mut app, "25");
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.editing(), None);
        assert_eq!(app.config.layout.gap, 7.25);
        assert_eq!(app.status(), "spacing (gap between boards): 7.25 mm");

        // Esc keeps the old value
        app.handle_key(key(KeyCode::Enter)); // Enter starts an edit on the layout page
        assert_eq!(app.editing(), Some("7.25"));
        send(&mut app, "9");
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.config.layout.gap, 7.25);

        // an invalid value is refused and the buffer stays open
        send(&mut app, "je"); // datum is a choice: 'e' toggles instead
        assert_eq!(app.editing(), None);
        send(&mut app, "kke"); // back to the gap
        send(&mut app, "-");
        assert_eq!(app.editing(), Some("-"));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(
            app.status(),
            "invalid value \"-\" for spacing (gap between boards)"
        );
        assert_eq!(app.editing(), Some("-")); // the buffer stays open
        send(&mut app, "5");
        app.handle_key(key(KeyCode::Enter)); // -5 breaks the >= 0 rule
        assert_eq!(app.editing(), Some("-5"));
        assert_eq!(app.config.layout.gap, 7.25);
        // stepping below the minimum is refused with its own message
        app.handle_key(key(KeyCode::Esc));
        send(&mut app, "jjjj"); // slot width, minimum 0.1
        assert_eq!(LAYOUT_FIELDS[app.index()].attr, Attr::SlotWidth);
        send(&mut app, "----------");
        assert_eq!(app.config.layout.slot_width, 0.1);
        assert_eq!(app.status(), "slot width: 0.1 mm");
    }

    #[test]
    fn a_backspace_on_a_fresh_edit_trims_the_prefill() {
        make_app!(app);
        send(&mut app, "4e");
        app.handle_key(key(KeyCode::Backspace));
        assert_eq!(app.editing(), Some("3"));
    }

    // ---------------------------------------------------------------- //
    // rendering
    // ---------------------------------------------------------------- //

    #[test]
    fn the_pads_page_renders_header_rows_fit_line_and_footer() {
        make_app!(app);
        let buf = render(&mut app, 150, 20);
        assert!(line(&buf, 0).starts_with("stencil    1 Pads"));
        assert_eq!(
            line(&buf, 1),
            "pads 1/5   pads to decide (5)   * = show all pads   \
             (1 hidden on disabled sides)"
        );
        assert_eq!(line(&buf, 2), pads_header().trim_end());
        assert_eq!(
            line(&buf, 3),
            format_row(app.pad((A_TOP, 0)), true, None).trim_end()
        );
        assert_eq!(
            line(&buf, 4),
            format_row(app.pad((A_TOP, 1)), false, None).trim_end()
        );

        // the tab of the current page is reversed
        let tabs = line(&buf, 0);
        let x = tabs.find("1 Pads").unwrap() as u16;
        assert!(modifier(&buf, x, 0).contains(Modifier::REVERSED));

        // the cursor row is reverse video all the way to the right edge
        assert!(modifier(&buf, 0, 3).contains(Modifier::REVERSED));
        assert!(modifier(&buf, 149, 3).contains(Modifier::REVERSED));
        assert!(!modifier(&buf, 149, 4).contains(Modifier::REVERSED));

        // the fit line sits above the footer
        let counts = StateCounts {
            undefined: 3,
            open: 0,
            ignore: 2,
            closed: 1,
        };
        let expected = fit_line(app.config, app.layout(), counts);
        assert_eq!(line(&buf, 18), clip(&expected, 150).trim_end());

        let help = wrap_items(&page_items(PAGE_PADS, false), 150, FOOTER_LINES);
        assert_eq!(help.len(), 1);
        assert_eq!(line(&buf, 19), help[0]);
        assert!(modifier(&buf, 0, 19).contains(Modifier::REVERSED));
    }

    #[test]
    fn pad_colours_and_the_paste_mark() {
        make_app!(app);
        send(&mut app, "*");
        let buf = render(&mut app, 120, 20);
        // row 4 of the all-pads list is C3.1: dimmed label, "· " in front
        let y = 3 + 4;
        assert!(line(&buf, y).contains("· C3.1"));
        // x = 2 + 12 = the state column, x = 14 the paste mark, x = 16 the label
        assert!(modifier(&buf, 16, y).contains(Modifier::DIM));
        assert!(!modifier(&buf, 30, y).contains(Modifier::DIM));
        // an "ignore" candidate is dimmed in its state column only
        let y = 3 + 2; // R2.1, ignore
        assert!(modifier(&buf, 2, y).contains(Modifier::DIM));
        assert!(modifier(&buf, 2, 3).contains(Modifier::BOLD)); // undefined
    }

    #[test]
    fn full_matches_are_bold_and_bold_beats_dim() {
        make_app!(app);
        send(&mut app, "/");
        send(&mut app, "u1 alpha");
        let buf = render(&mut app, 120, 20);
        assert_eq!(line(&buf, 1), "filter: u1 alpha  (5 of 5, 2 full)");
        // row 1 (U1.2, a full match, not the cursor) is bold
        assert!(modifier(&buf, 40, 4).contains(Modifier::BOLD));
        // row 2 (R2.1, one word only) is not
        assert!(!modifier(&buf, 40, 5).contains(Modifier::BOLD));

        // a pasted pad that matches fully is bold and no longer dim
        app.handle_key(key(KeyCode::Esc));
        send(&mut app, "*/");
        send(&mut app, "c3");
        let buf = render(&mut app, 120, 20);
        assert_eq!(pad_labels(&app), vec!["C3.1", "C3.2"]);
        let y = 4; // the second row, not the cursor
        assert!(modifier(&buf, 16, y).contains(Modifier::BOLD));
        assert!(!modifier(&buf, 16, y).contains(Modifier::DIM));
    }

    #[test]
    fn the_sides_page_dims_the_disabled_rows() {
        make_app!(app);
        send(&mut app, "2");
        let buf = render(&mut app, 120, 20);
        assert_eq!(line(&buf, 1), "sides: 2/3 enabled");
        assert_eq!(line(&buf, 2), side_row(app.side(A_TOP), true).trim_end());
        assert!(line(&buf, 3).contains("[ ] alpha · bottom (mirrored)"));
        assert!(modifier(&buf, 2, 3).contains(Modifier::DIM));
        assert!(!modifier(&buf, 2, 4).contains(Modifier::DIM));
    }

    #[test]
    fn the_stencil_page_shows_a_fit_verdict_for_every_preset() {
        make_app!(app);
        send(&mut app, "3");
        let buf = render(&mut app, 120, 20);
        let rows: Vec<String> = (2..2 + STENCIL_SIZES.len() as u16)
            .map(|y| line(&buf, y))
            .collect();
        assert_eq!(rows.len(), 8);
        assert!(rows[0].contains("270 x 270 mm"));
        assert!(rows[0].contains("landscape: 270 x 270  NO"));
        assert!(rows[1].contains("● 380 x 280 mm")); // the current one
        assert!(rows[1].contains("← landscape"));
        assert!(rows[2].contains("landscape: 420 x 320  fits"));
        assert!(rows[2].contains("portrait: 320 x 420  NO"));
        assert!(rows[6].contains("landscape: 600 x 600  fits"));
        assert!(rows[6].contains("portrait: 600 x 600  fits"));
        // the selected preset is bold
        assert!(modifier(&buf, 4, 3).contains(Modifier::BOLD));
    }

    #[test]
    fn the_layout_page_dims_the_rows_of_the_other_datum() {
        make_app!(app);
        send(&mut app, "4");
        let buf = render(&mut app, 120, 24);
        assert_eq!(line(&buf, 1), "layout parameters");
        // row 2 is "hole diameter": the datum is slots, so it is dimmed
        assert!(line(&buf, 4).contains("hole diameter"));
        assert!(modifier(&buf, 4, 4).contains(Modifier::DIM));
        // row 4 is "slot width": it applies
        assert!(line(&buf, 6).contains("slot width"));
        assert!(!modifier(&buf, 4, 6).contains(Modifier::DIM));
        // and the new dot clearance row is there, never dimmed
        assert!(line(&buf, 17).contains("dot clearance"));
        assert!(line(&buf, 17).ends_with("0.5 mm"));
        assert!(!modifier(&buf, 4, 17).contains(Modifier::DIM));

        // switching the datum swaps the dimming
        send(&mut app, "g");
        send(&mut app, "j ");
        let buf = render(&mut app, 120, 24);
        assert!(!modifier(&buf, 4, 4).contains(Modifier::DIM));
        assert!(modifier(&buf, 4, 6).contains(Modifier::DIM));
    }

    #[test]
    fn the_footer_wraps_at_80_and_fits_on_one_line_at_150() {
        make_app!(app);
        let buf = render(&mut app, 150, 20);
        let one = wrap_items(&page_items(PAGE_PADS, false), 150, FOOTER_LINES);
        assert_eq!(one.len(), 1);
        assert_eq!(line(&buf, 19), one[0]);
        assert!(!line(&buf, 18).is_empty()); // the fit line, not more help

        let buf = render(&mut app, 80, 20);
        let two = wrap_items(&page_items(PAGE_PADS, false), 80, FOOTER_LINES);
        assert_eq!(two.len(), 2);
        assert_eq!(line(&buf, 18), two[0]);
        assert_eq!(line(&buf, 19), two[1]);
    }

    #[test]
    fn the_footer_shows_the_search_prompt_and_the_filter_key() {
        make_app!(app);
        send(&mut app, "/");
        send(&mut app, "u1");
        let buf = render(&mut app, 120, 20);
        assert!(line(&buf, 19).starts_with("/u1_  type to filter"));
        assert!(modifier(&buf, 0, 19).contains(Modifier::REVERSED));

        app.handle_key(key(KeyCode::Enter));
        let buf = render(&mut app, 120, 20);
        let footer = format!("{}{}", line(&buf, 18), line(&buf, 19));
        assert!(footer.contains(FILTER_ITEM), "{footer:?}");
    }

    #[test]
    fn a_status_message_takes_the_footer_over() {
        make_app!(app);
        send(&mut app, "o");
        let buf = render(&mut app, 120, 20);
        assert_eq!(line(&buf, 19), "U1.1 (alpha top) -> open");
    }

    #[test]
    fn the_editing_footer_replaces_the_page_help() {
        make_app!(app);
        send(&mut app, "4e");
        let buf = render(&mut app, 120, 24);
        assert!(line(&buf, 2).ends_with("30_"));
        // the status message of the edit wins over the key help
        assert!(
            line(&buf, 23).starts_with("editing spacing"),
            "{:?}",
            line(&buf, 23)
        );
        // and the edit help is shown once the message is gone
        app.status.clear();
        let buf = render(&mut app, 120, 24);
        assert_eq!(line(&buf, 23), EDIT_ITEMS.join(ITEM_SEP));
    }

    #[test]
    fn an_empty_list_says_so() {
        make_app!(app);
        send(&mut app, "2N1"); // every side off
        let buf = render(&mut app, 120, 20);
        assert_eq!(
            line(&buf, 2).trim(),
            "no pads to decide on the enabled sides"
        );
        send(&mut app, "/");
        send(&mut app, "zz");
        let buf = render(&mut app, 120, 20);
        assert_eq!(line(&buf, 2).trim(), "no pad matches the filter");
    }

    #[test]
    fn a_tiny_terminal_does_not_panic() {
        make_app!(app);
        for (w, h) in [(2u16, 8u16), (1, 1), (3, 2), (80, 1), (5, 5), (20, 7)] {
            let buf = render(&mut app, w, h);
            assert_eq!(buf.area.width, w);
            assert_eq!(buf.area.height, h);
        }
        // and on every page, with a search open
        send(&mut app, "/");
        send(&mut app, "u");
        for page in ['1', '2', '3', '4'] {
            app.handle_key(key(KeyCode::Esc));
            send(&mut app, &page.to_string());
            render(&mut app, 2, 8);
            render(&mut app, 120, 20);
        }
    }

    #[test]
    fn scrolling_keeps_the_cursor_visible() {
        make_app!(app);
        send(&mut app, "4G");
        // 10 rows high: 2 header + 1 fit + 1 help -> 6 body rows
        let buf = render(&mut app, 120, 10);
        assert_eq!(app.index(), LAYOUT_FIELDS.len() - 1);
        assert!(line(&buf, 7).contains("sort"));
        send(&mut app, "g");
        let buf = render(&mut app, 120, 10);
        assert!(line(&buf, 2).contains("spacing"));
    }

    // ---------------------------------------------------------------- //
    // the split view
    // ---------------------------------------------------------------- //

    #[test]
    fn the_split_plan_follows_the_terminal_width() {
        assert_eq!(
            split_plan(150),
            SplitPlan {
                left: 90,
                right_x: 91,
                right_w: 59,
                preview: true
            }
        );
        assert_eq!(split_plan(400).left, 90);
        assert_eq!(split_plan(400).right_w, 400 - 91);
        // below 150 only the prompt, and the app keeps at least 60 columns
        let narrow = split_plan(120);
        assert!(!narrow.preview);
        assert_eq!(narrow.left, SPLIT_LEFT_MIN);
        assert_eq!(narrow.right_w, SPLIT_PROMPT.chars().count() as u16);
        assert_eq!(split_plan(149).left, 149 - 1 - narrow.right_w);
        // 119 cannot host both: the prompt takes the whole width
        assert_eq!(
            split_plan(119),
            SplitPlan {
                left: 0,
                right_x: 0,
                right_w: 119,
                preview: false
            }
        );
        // the caption takes one row, the legend one more when there is room
        assert_eq!(panel_rows(40), 38);
        assert_eq!(panel_rows(5), 3);
        assert_eq!(panel_rows(4), 3);
        assert_eq!(panel_rows(0), 0);
    }

    #[test]
    fn v_splits_the_screen_and_draws_the_selected_pad() {
        make_packed_app!(app);
        assert!(!app.split());
        send(&mut app, "v");
        assert!(app.split());
        assert_eq!(
            app.status(),
            "split view on — the pad under the cursor is drawn on the right"
        );
        app.status.clear();

        let buf = render(&mut app, 150, 40);
        // the separator runs the whole height, the app keeps 90 columns
        for y in 0..40 {
            assert_eq!(buf[(90, y)].symbol(), SEPARATOR, "row {y}");
        }
        assert!(band(&buf, 0, 0, 90).contains("1 Pads"));
        let help = wrap_items(&page_items(PAGE_PADS, false), 90, FOOTER_LINES);
        assert_eq!(band(&buf, 39, 0, 90), help[help.len() - 1]);

        // the caption names the pad, the window and the zoom
        let caption = band(&buf, 0, 91, 150);
        assert!(
            caption.starts_with("U1.1 · alpha top · window 4.00 x 5.15 mm · 1 col = 0.07 mm"),
            "{caption:?}"
        );
        // the legend closes the panel
        assert!(band(&buf, 39, 91, 150).starts_with("red opening  yellow undefined"));

        // the window is centred on the pad: its middle cell is the pad itself
        let (symbol, fg, _) = cell(&buf, 91 + 29, 1 + 18);
        assert_eq!(symbol, ascii::FULL_BLOCK);
        assert_eq!(fg, ascii::class_color(Class::Selected));
        // and the picture carries every class of the fixture
        assert!(colour_count(&buf, 91, 150, Class::Selected) > 20);
        assert!(colour_count(&buf, 91, 150, Class::Undefined) > 20); // U1.2
        assert!(colour_count(&buf, 91, 150, Class::Opening) > 10); // live paste
        assert!(colour_count(&buf, 91, 150, Class::Ignored) > 10); // closed paste
        assert!(colour_count(&buf, 91, 150, Class::Outline) > 0); // the board edge
                                                                  // nothing of the picture leaks into the app's half
        assert_eq!(colour_count(&buf, 0, 90, Class::Selected), 0);

        // v again and the app is alone on the screen
        send(&mut app, "v");
        let buf = render(&mut app, 150, 40);
        assert_ne!(buf[(90, 0)].symbol(), SEPARATOR);
    }

    #[test]
    fn a_narrow_terminal_only_asks_for_a_wider_one() {
        make_packed_app!(app);
        send(&mut app, "v");
        let buf = render(&mut app, 120, 40);
        // 60 for the app, 1 for the line, 59 for the prompt
        for y in 0..40 {
            assert_eq!(buf[(60, y)].symbol(), SEPARATOR, "row {y}");
        }
        assert_eq!(band(&buf, 20, 61, 120), SPLIT_PROMPT);
        for y in 0..40 {
            if y != 20 {
                assert_eq!(band(&buf, y, 61, 120), "", "row {y}");
            }
        }
        // the left half is the live app and still takes keys
        assert!(band(&buf, 0, 0, 60).contains("1 Pads"));
        assert_eq!(app.index(), 0);
        send(&mut app, "j");
        assert_eq!(app.index(), 1);
        let buf = render(&mut app, 120, 40);
        assert!(band(&buf, 1, 0, 60).starts_with("pads 2/5"));
    }

    #[test]
    fn the_picture_follows_the_cursor_and_the_pad_states() {
        make_packed_app!(app);
        send(&mut app, "v");
        let buf = render(&mut app, 150, 40);
        assert!(band(&buf, 0, 91, 150).starts_with("U1.1 · alpha top"));
        let undefined = colour_count(&buf, 91, 150, Class::Undefined);
        assert!(undefined > 0);

        // the cursor moves: another pad is drawn
        send(&mut app, "j");
        let buf = render(&mut app, 150, 40);
        assert!(band(&buf, 0, 91, 150).starts_with("U1.2 · alpha top"));

        // opening U1.2 turns it red; back on U1.1 nothing is yellow any more
        send(&mut app, "ok");
        let buf = render(&mut app, 150, 40);
        assert!(band(&buf, 0, 91, 150).starts_with("U1.1 · alpha top"));
        assert_eq!(colour_count(&buf, 91, 150, Class::Undefined), 0);
        assert!(colour_count(&buf, 91, 150, Class::Opening) > undefined);
    }

    #[test]
    fn the_panel_says_so_when_there_is_no_pad() {
        make_packed_app!(app);
        send(&mut app, "v");
        send(&mut app, "2N"); // every side off: nothing is left to draw
        let buf = render(&mut app, 150, 40);
        assert_eq!(band(&buf, 20, 91, 150).trim(), NO_PAD);
        assert_eq!(colour_count(&buf, 91, 150, Class::Selected), 0);
    }

    #[test]
    fn the_footer_lists_the_preview_and_the_split_keys() {
        for page in 0..PAGE_NAMES.len() {
            let items = page_items(page, false);
            assert!(items.iter().any(|i| i == "p preview"), "page {page}");
            assert!(items.iter().any(|i| i == "v split view"), "page {page}");
            assert!(!items.iter().any(|i| i.contains("p/v")), "page {page}");
        }
    }

    #[test]
    fn the_split_view_survives_every_terminal_size() {
        make_packed_app!(app);
        send(&mut app, "v");
        for (w, h) in [
            (2u16, 8u16),
            (60, 10),
            (149, 40),
            (400, 100),
            (150, 40),
            (1, 1),
            (91, 2),
            (150, 4),
        ] {
            let buf = render(&mut app, w, h);
            assert_eq!(buf.area.width, w);
            assert_eq!(buf.area.height, h);
        }
        // and on every page, with a search open
        send(&mut app, "/");
        send(&mut app, "u");
        for page in ['1', '2', '3', '4'] {
            app.handle_key(key(KeyCode::Esc));
            send(&mut app, &page.to_string());
            render(&mut app, 2, 8);
            render(&mut app, 150, 40);
            render(&mut app, 120, 40);
        }
    }

    #[test]
    fn nothing_to_show_returns_without_a_terminal() {
        let mut config = Config::default();
        let mut on_preview =
            |_: &[Project], _: &Config, _: Option<PadId>, _: bool| -> String { String::new() };
        let mut compute_layout = |_: &[Project], cfg: &Config| -> Layout { fake_layout(cfg) };
        let hooks = TuiHooks {
            on_preview: &mut on_preview,
            compute_layout: &mut compute_layout,
        };
        assert!(run_tui(&mut [], &[], &mut config, hooks, "empty").unwrap());
    }
}
