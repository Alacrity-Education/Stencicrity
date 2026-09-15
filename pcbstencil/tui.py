"""Curses front-end: a four page TUI to set everything up before generating.

Pages (tabs in the header, switched with ``1``-``4`` or Tab / Shift-Tab):

1. **Pads** - every candidate pad (copper without paste) of the *enabled*
   sides; each one is marked ``open`` (cut an opening), ``ignore`` (leave it
   closed) or left ``undefined`` (treated as closed).  ``*`` toggles the view
   to *all* pads: the pads that already have a paste opening are shown dimmed
   and can be set to ``ignore``, which removes their opening from the stencil
   (they are ``open`` by default and never ``undefined``).
2. **Sides** - which project sides get a cell on the stencil.
3. **Stencil** - the orderable sheet sizes and the orientation, with a
   fits / does not fit verdict for every preset.
4. **Layout** - the layout parameters: gap, the ``datum`` (which alignment
   features every cell gets: ``slots``, ``holes`` or ``none``), the numbers of
   both datums - the modular jig raster of the slots, the hole grid of the
   holes - divider dots and the gap between the two dotted lines, outer
   border, cell order.  The rows of the datum that is not selected are drawn
   dimmed but stay editable.

On the two list pages (Pads and Sides) ``/`` starts a vim like search: every
printable character is appended to the query and the list is filtered while
you type (a row survives when any whitespace separated word of the query is a
substring of any of its fields).  The matches are ordered by how many
distinct words of the query they match - the best matches first - and a row
that matches *every* word is drawn bold.  Enter or Esc leaves the search,
clears the filter and keeps the cursor on the item it was on.

Everything above :class:`_App` is pure and unit testable: state cycling, row
formatting, field stepping/validation, the search matching/filtering, the
footer key-help wrapping and the fit line do not touch curses.  The curses
class only draws and dispatches keys.

The live objects are mutated in place: ``pad.state``, ``side.enabled``,
``config.size``, ``config.orientation`` and ``config.layout.*``.  The
``config.sides`` / ``config.pads`` dictionaries are left alone - the caller
collects them from the objects.
"""
from __future__ import annotations

import curses
from dataclasses import dataclass
from typing import Callable, Optional, Sequence, TypeVar

from .model import (
    DATUM_HOLES,
    DATUM_MODES,
    DATUM_SLOTS,
    ORIENTATION_LANDSCAPE,
    ORIENTATION_PORTRAIT,
    ORIENTATIONS,
    SORT_ORDERS,
    STATE_IGNORE,
    STATE_OPEN,
    STATE_UNDEFINED,
    STENCIL_SIZES,
    Config,
    Layout,
    Pad,
    Side,
)

# Callback: (pad or None, open_in_viewer) -> path of the rendered preview.
PreviewFunc = Callable[[Optional[Pad], bool], str]
# Callback: (config or None -> use the live one) -> Layout for that config.
LayoutFunc = Callable[[Optional[Config]], Layout]

STATE_MARKERS = {STATE_UNDEFINED: "?", STATE_OPEN: "+", STATE_IGNORE: "-"}
# Not a pad state: the counts line calls a pasted pad set to "ignore" closed.
STATE_CLOSED = "closed"
PASTE_MARK = "· "                    # in front of a pad that already has paste

PAGE_PADS = 0
PAGE_SIDES = 1
PAGE_STENCIL = 2
PAGE_LAYOUT = 3
PAGE_NAMES = ("Pads", "Sides", "Stencil", "Layout")

_COL_LABEL = 12
_COL_PROJECT = 20
_COL_SIDE = 7
_COL_FUNCTION = 13
_COL_SHAPE = 16
_STATE_WIDTH = 9
_COL_FIELD = 38
_COL_SIDE_NAME = 30

# The footer help of every page as a list of "key description" items; they are
# laid out on one or two lines by wrap_items() so nothing is lost on a narrow
# terminal.
PAGE_ITEMS: tuple[tuple[str, ...], ...] = (
    ("↑↓/jk move", "/ search", "space cycle", "o open", "i ignore", "a component",
     "n/N undef", "* all pads", "p/v preview", "enter generate",
     "1-4/tab page", "q quit"),
    ("↑↓ move", "/ search", "space/enter toggle side", "A all", "N none",
     "g generate", "p/v preview", "1-4/tab page", "q quit"),
    ("↑↓ move", "space/enter pick size", "o orientation", "g generate",
     "p/v preview", "1-4/tab page", "q quit"),
    ("↑↓ move", "+/- step", "space toggle", "e/enter edit", "g generate",
     "p/v preview", "1-4/tab page", "q quit"),
)
EDIT_ITEMS = ("type a number", "backspace", "enter accept", "esc cancel")
SEARCH_ITEMS = ("type to filter", "backspace", "↑↓ move", "enter/esc leave the search")

ITEM_SEP = "  "                      # between two key-help items on one line
FOOTER_LINES = 2                     # the key help never grows past two lines

# The same help as one (possibly very long) line; kept for tests and callers.
PAGE_KEYS = tuple(ITEM_SEP.join(items) for items in PAGE_ITEMS)
EDIT_KEYS = ITEM_SEP.join(EDIT_ITEMS)
SEARCH_KEYS = ITEM_SEP.join(SEARCH_ITEMS)
NOFIT_WARNING = "layout does not fit the stencil — press again to generate anyway"

# Colour pair numbers (only used when the terminal has colours).
_PAIR_UNDEFINED = 1
_PAIR_OPEN = 2
_PAIR_IGNORE = 3
_PAIR_HEADER = 4
_PAIR_FITS = 5
_PAIR_NOFIT = 6
_PAIR_SEARCH = 7


# --------------------------------------------------------------------------- #
# Pure helpers: pad states
# --------------------------------------------------------------------------- #
def cycle_state(state: str) -> str:
    """Next state for the space bar: undefined -> open -> ignore -> open ...

    ``undefined`` is only ever a starting point; once a pad has been touched
    it toggles between ``open`` and ``ignore``.
    """
    return STATE_IGNORE if state == STATE_OPEN else STATE_OPEN


def next_state(pad: Pad) -> str:
    """The state the space bar gives ``pad``.

    Candidates cycle undefined -> open -> ignore -> open …; a pad that already
    has a paste opening only toggles open <-> ignore and is never
    ``undefined`` (``ignore`` closes its opening).
    """
    if pad.has_paste and pad.state not in (STATE_OPEN, STATE_IGNORE):
        return STATE_OPEN
    return cycle_state(pad.state)


def allowed_state(pad: Pad, state: str) -> bool:
    """May ``pad`` be put in ``state``? Pasted pads are open or ignore only."""
    if pad.has_paste:
        return state in (STATE_OPEN, STATE_IGNORE)
    return state in (STATE_UNDEFINED, STATE_OPEN, STATE_IGNORE)


def count_states(pads: Sequence[Pad]) -> dict[str, int]:
    """Count pads per state, candidates and pasted pads apart.

    ``undefined`` / ``open`` / ``ignore`` count the candidates (same shape as
    :func:`pcbstencil.pads.state_counts`); ``closed`` counts the pads that have
    a paste opening the user removed (state ``ignore``).
    """
    counts = {STATE_UNDEFINED: 0, STATE_OPEN: 0, STATE_IGNORE: 0, STATE_CLOSED: 0}
    for pad in pads:
        if pad.has_paste:
            if pad.is_closed:
                counts[STATE_CLOSED] += 1
        elif pad.state in counts:
            counts[pad.state] += 1
    return counts


def component_pads(pads: Sequence[Pad], pad: Pad) -> list[Pad]:
    """Every pad of the same component (same project, side and reference)."""
    return [p for p in pads
            if p.project == pad.project and p.side == pad.side and p.ref == pad.ref]


def apply_component(pads: Sequence[Pad], pad: Pad, state: Optional[str] = None) -> int:
    """Set ``state`` (default: the pad's own state) on the whole component.

    Pads that may not take the state are skipped (a pad with paste is never
    ``undefined``).  Returns the number of pads changed, the cursor pad
    included.
    """
    target = pad.state if state is None else state
    changed = 0
    for other in component_pads(pads, pad):
        if not allowed_state(other, target):
            continue
        other.state = target
        changed += 1
    return changed


def find_undefined(pads: Sequence[Pad], start: int, forward: bool = True) -> Optional[int]:
    """Index of the next/previous undefined *candidate*, wrapping around ``start``.

    Pads that already have paste are never undefined, so ``n``/``N`` walk the
    candidates only, even in the all-pads view.
    """
    total = len(pads)
    if total == 0:
        return None
    step = 1 if forward else -1
    for offset in range(1, total + 1):
        index = (start + step * offset) % total
        pad = pads[index]
        if pad.is_candidate and pad.state == STATE_UNDEFINED:
            return index
    return None


def visible_pads(pads: Sequence[Pad], sides: Sequence[Side],
                 show_all: bool = False) -> list[Pad]:
    """The pads of the enabled sides the Pads page shows (order preserved).

    By default only the candidates (pads without a paste opening) are listed;
    with ``show_all`` the pads that already have paste are listed too, so the
    user can close their opening.  With no sides at all nothing can be filtered
    by side, so every pad passes that step.
    """
    if sides:
        keys = {side.key for side in sides if side.enabled}
        rows = [pad for pad in pads if pad.side_key in keys]
    else:
        rows = list(pads)
    if show_all:
        return rows
    return [pad for pad in rows if pad.is_candidate]


# --------------------------------------------------------------------------- #
# Pure helpers: text
# --------------------------------------------------------------------------- #
def clip(text: str, width: int) -> str:
    """Truncate ``text`` so it always fits in ``width`` columns."""
    if width <= 0:
        return ""
    return text[:width]


def _col(text: object, width: int) -> str:
    """Pad or truncate ``text`` to exactly ``width`` characters."""
    text = "" if text is None else str(text)
    if len(text) > width:
        return text[:max(0, width - 1)] + "…"
    return text.ljust(width)


def fmt_mm(value: float) -> str:
    """Compact millimetre number: 30.0 -> '30', 12.5 -> '12.5'."""
    text = f"{float(value):.3f}".rstrip("0").rstrip(".")
    return text if text not in ("", "-") else "0"


def ellipsis(text: str, width: int) -> str:
    """``text`` cut to ``width`` columns, marking the cut with ``…``."""
    if width <= 0:
        return ""
    if len(text) <= width:
        return text
    if width == 1:
        return "…"
    return text[:width - 1] + "…"


def _wrap(items: Sequence[str], width: int, max_lines: int, sep: str) -> list[str]:
    """Greedy line breaking of ``items`` joined by ``sep``; at most ``max_lines``."""
    items = [str(item) for item in items if str(item) != ""]
    if not items or width <= 0 or max_lines <= 0:
        return []
    lines: list[str] = []
    current = ""
    index = 0
    while index < len(items) and len(lines) < max_lines:
        item = items[index]
        candidate = item if not current else current + sep + item
        if len(candidate) <= width:
            current, index = candidate, index + 1
            continue
        if not current:                    # a single item wider than one line
            current, index = item, index + 1
        lines.append(current)
        current = ""
    if current and len(lines) < max_lines:
        lines.append(current)
        current = ""
    left = index < len(items) or bool(current)
    lines = [ellipsis(line, width) for line in lines]
    if left and lines:                     # say that something was dropped
        lines[-1] = ellipsis(lines[-1] + sep + "…", width)
    return lines


def wrap_items(items: list[str], width: int, max_lines: int = 2) -> list[str]:
    """Lay ``key description`` items out on at most ``max_lines`` lines.

    Items are separated by two spaces and never split: as many as fit go on a
    line, the rest start the next one.  When even ``max_lines`` lines are not
    enough the last one is truncated with ``…``, so the result is always at
    most ``max_lines`` lines of at most ``width`` columns.
    """
    return _wrap(items, width, max_lines, ITEM_SEP)


def wrap_text(text: str, width: int, max_lines: int = 2) -> list[str]:
    """Same as :func:`wrap_items` for a sentence (status messages)."""
    return _wrap(text.split(), width, max_lines, " ")


# --------------------------------------------------------------------------- #
# Pure helpers: search
# --------------------------------------------------------------------------- #
_Row = TypeVar("_Row")


def query_words(query: str) -> list[str]:
    """The distinct lower case words of ``query``, in the order they appear.

    The query is split on whitespace and a word typed twice only counts once,
    so ``"u1 u1"`` is the very same one word query as ``"u1"``.
    """
    return list(dict.fromkeys(str(query).lower().split()))


def match_count(fields: list[str], query: str) -> int:
    """How many distinct words of ``query`` match these ``fields``?

    A word matches when it is a case-insensitive substring of *any* field.
    The words are OR-ed for the filter (a row is kept as soon as the count is
    >= 1) and the count itself orders the result: the rows that match every
    word - the *full* matches - come first.  A blank query has no words at
    all, so it counts 0 for every row and nothing is a full match.
    """
    words = query_words(query)
    if not words:
        return 0
    haystack = [str(field).lower() for field in fields if field]
    return sum(1 for word in words if any(word in text for text in haystack))


def matches(fields: list[str], query: str) -> bool:
    """Does ``query`` match a row with these ``fields`` (count >= 1)?

    A blank query matches everything.
    """
    if not query_words(query):
        return True
    return match_count(fields, query) >= 1


def filter_counts(rows: Sequence[_Row], fields_of: Callable[[_Row], list[str]],
                  query: str) -> list[tuple[_Row, int]]:
    """The matching ``(row, match count)`` pairs, the best matches first.

    The pairs are sorted by the number of matched words, descending; rows with
    the same count keep their original order (the sort is stable).  A blank
    query keeps every row, in order, with a count of 0.
    """
    if not query_words(query):
        return [(row, 0) for row in rows]
    pairs = []
    for row in rows:
        count = match_count(fields_of(row), query)
        if count:
            pairs.append((row, count))
    pairs.sort(key=lambda pair: -pair[1])
    return pairs


def filter_rows(rows: Sequence[_Row], fields_of: Callable[[_Row], list[str]],
                query: str) -> list[_Row]:
    """The rows matching ``query``, best (most words matched) first.

    Everything - in the original order - when the query is blank.
    """
    return [row for row, _ in filter_counts(rows, fields_of, query)]


def full_matches(counts: Sequence[int], query: str) -> int:
    """How many of ``counts`` matched *every* distinct word of ``query``."""
    words = len(query_words(query))
    if not words:
        return 0
    return sum(1 for count in counts if count >= words)


def match_summary(shown: int, total: int, full: int, words: int) -> str:
    """The ``(n of N, k full)`` tail of the search sub-header.

    ``, k full`` is left out for a one word query, where every match is a full
    match anyway, and for the blank query that filters nothing.
    """
    if not shown:
        return "no match"
    if words > 1:
        return f"({shown} of {total}, {full} full)"
    return f"({shown} of {total})"


def search_char(key: int) -> Optional[str]:
    """The character ``key`` adds to a query, or None when it adds nothing.

    Only printable ASCII (space included) is accepted, so control keys and
    everything curses reports as a special key (``KEY_*`` is >= 256) stay out
    of the query and can still navigate.
    """
    if 32 <= key < 127:
        return chr(key)
    return None


def restore_index(rows: Sequence[_Row], item: Optional[_Row],
                  fallback: int = 0) -> int:
    """Where ``item`` sits in ``rows``; ``fallback`` (clamped) when it is gone.

    Identity based on purpose: the very same pad/side object is followed from
    one list to the other (filtered <-> unfiltered).
    """
    if item is not None:
        for i, row in enumerate(rows):
            if row is item:
                return i
    return max(0, min(fallback, max(0, len(rows) - 1)))


# --------------------------------------------------------------------------- #
# Pure helpers: rows
# --------------------------------------------------------------------------- #
def pad_position(pad: Pad) -> str:
    """The coordinate text of a pad row, exactly as it is displayed."""
    return f"x={pad.x:8.3f} y={pad.y:8.3f}"


def board_text(side: Side) -> str:
    """The board size text of a side row, exactly as it is displayed."""
    return f"{side.project.width:7.1f} x {side.project.height:7.1f} mm"


def pad_fields(pad: Pad) -> list[str]:
    """Everything of a pad the search looks at.

    A pad that already has a paste opening also answers to ``paste`` (and to
    ``closed`` once its opening was removed).
    """
    extra = ""
    if pad.has_paste:
        extra = "paste closed" if pad.is_closed else "paste"
    return [pad.label, pad.ref, pad.pin, pad.project, pad.side,
            pad.function or "", pad.shape or "", pad.state, pad_position(pad),
            extra]


def side_fields(side: Side) -> list[str]:
    """Everything of a side the search looks at."""
    return [side.project.name, side.name, side.label,
            "mirrored" if side.mirror else "",
            "on" if side.enabled else "off",
            board_text(side)]


def row_segments(pad: Pad, *, cursor: bool = False) -> list[tuple[str, str]]:
    """Pad row as ``(text, kind)`` chunks.

    ``kind`` is ``"state"`` (the ``+ open`` column, coloured by state),
    ``"pad"`` (the REF.pin column, dimmed for a pad that already has paste) or
    ``"plain"``.  A pad with paste is also marked with a ``·`` in front of its
    reference, so a terminal without colours can tell it apart.
    """
    marker = STATE_MARKERS.get(pad.state, "?")
    state_field = f"{marker} {pad.state:<{_STATE_WIDTH}}"
    pad_field = (PASTE_MARK if pad.has_paste else " " * len(PASTE_MARK)) \
        + _col(pad.label, _COL_LABEL)
    rest = " ".join((
        _col(pad.project, _COL_PROJECT),
        _col(pad.side, _COL_SIDE),
        _col(pad.function or "-", _COL_FUNCTION),
        _col(pad.shape or "-", _COL_SHAPE),
        pad_position(pad),
    ))
    return [
        ("→ " if cursor else "  ", "plain"),
        (state_field + " ", "state"),
        (pad_field + " ", "pad"),
        (rest, "plain"),
    ]


def format_row(pad: Pad, *, cursor: bool = False, width: Optional[int] = None) -> str:
    """One plain-text pad row (used by the curses view and by tests)."""
    line = "".join(text for text, _ in row_segments(pad, cursor=cursor))
    return clip(line, width) if width is not None else line


def pads_header() -> str:
    """Column header of the pads page."""
    return "  " + f"{'state':<{_STATE_WIDTH + 2}} " + " ".join((
        " " * len(PASTE_MARK) + _col("pad", _COL_LABEL),
        _col("project", _COL_PROJECT),
        _col("side", _COL_SIDE),
        _col("function", _COL_FUNCTION),
        _col("shape", _COL_SHAPE),
        "position",
    ))


def side_row(side: Side, *, cursor: bool = False) -> str:
    """One row of the sides page."""
    mark = "[x]" if side.enabled else "[ ]"
    name = f"{side.project.name} · {side.name}"
    if side.mirror:
        name += " (mirrored)"
    candidates = side.candidates
    undefined = sum(1 for pad in candidates if pad.state == STATE_UNDEFINED)
    board = board_text(side)
    return (f"{'→ ' if cursor else '  '}{mark} {_col(name, _COL_SIDE_NAME)} {board}"
            f"   paste {len(side.paste_objects):<6}"
            f" pads to decide {len(candidates):<6}"
            f" undefined {undefined}")


def orientation_size(size: tuple[int, int], orientation: str) -> tuple[int, int]:
    """Sheet (width, height) of ``size`` in ``orientation`` (size is long side first)."""
    long_side, short_side = max(size), min(size)
    if orientation == ORIENTATION_PORTRAIT:
        return (short_side, long_side)
    return (long_side, short_side)


def _fit_word(fits: Optional[bool]) -> str:
    if fits is None:
        return "?"
    return "fits" if fits else "NO"


def preset_row(size: tuple[int, int], fits: dict[str, Optional[bool]],
               config: Config, *, cursor: bool = False) -> str:
    """One row of the stencil page: the preset and its fit in both orientations."""
    selected = tuple(config.size) == tuple(size)
    mark = "●" if selected else " "
    long_side, short_side = max(size), min(size)
    land_w, land_h = orientation_size(size, ORIENTATION_LANDSCAPE)
    port_w, port_h = orientation_size(size, ORIENTATION_PORTRAIT)
    land = (f"landscape: {land_w} x {land_h}  "
            f"{_fit_word(fits.get(ORIENTATION_LANDSCAPE))}")
    port = (f"portrait: {port_w} x {port_h}  "
            f"{_fit_word(fits.get(ORIENTATION_PORTRAIT))}")
    current = ""
    if selected:
        current = f"  ← {config.orientation}"
    return (f"{'→ ' if cursor else '  '}{mark} {long_side} x {short_side} mm   "
            f"{land:<30}{port:<30}{current}")


def fit_segments(config: Config, layout: Optional[Layout],
                 pads: Sequence[Pad]) -> list[tuple[str, str]]:
    """The always-visible status line as ``(text, kind)`` chunks.

    ``kind`` is ``"plain"``, ``"fits"`` or ``"nofit"``.
    """
    parts: list[tuple[str, str]] = [
        (f"stencil {config.size_label} {config.orientation}", "plain"),
    ]
    if layout is None:
        parts.append((" · block ? · ", "plain"))
        parts.append(("NO LAYOUT", "nofit"))
    else:
        parts.append((f" · block {layout.block_width:.1f} x {layout.block_height:.1f} mm · ",
                      "plain"))
        parts.append(("FITS" if layout.fits else "DOES NOT FIT",
                      "fits" if layout.fits else "nofit"))
    counts = count_states(pads)
    parts.append((f" · pads: undefined {counts[STATE_UNDEFINED]}  "
                  f"open {counts[STATE_OPEN]}  ignore {counts[STATE_IGNORE]}  "
                  f"closed {counts[STATE_CLOSED]}", "plain"))
    return parts


def fit_line(config: Config, layout: Optional[Layout], pads: Sequence[Pad]) -> str:
    """Plain text of :func:`fit_segments`."""
    return "".join(text for text, _ in fit_segments(config, layout, pads))


# --------------------------------------------------------------------------- #
# Pure helpers: layout page fields
# --------------------------------------------------------------------------- #
@dataclass(frozen=True)
class Field:
    """One editable row of the layout page."""
    attr: str
    label: str
    kind: str                            # "length" | "bool" | "choice"
    unit: str = ""
    step: float = 0.5
    minimum: Optional[float] = None      # clamp while stepping (None: unbounded)
    rule: str = "any"                    # "any" | "ge0" | "gt0"
    choices: tuple[str, ...] = ()

    @property
    def numeric(self) -> bool:
        return self.kind == "length"


LAYOUT_FIELDS: tuple[Field, ...] = (
    Field("gap", "spacing (gap between boards)", "length", "mm", 0.5, 0.0, "ge0"),
    Field("datum", "datum (alignment features)", "choice",
          choices=tuple(DATUM_MODES)),
    Field("hole_dia", "hole diameter", "length", "mm", 0.5, 0.1, "gt0"),
    Field("hole_inset", "hole inset (dotted line to hole edge)", "length", "mm", 0.5),
    Field("slot_width", "slot width", "length", "mm", 0.5, 0.1, "gt0"),
    Field("slot_length", "slot length", "length", "mm", 0.5, 0.1, "gt0"),
    Field("slot_offset", "slot offset (edge to outer wall)", "length", "mm",
          0.5, 0.0, "ge0"),
    Field("slot_pitch", "slot pitch (modular jig raster)", "length", "mm",
          5.0, 1.0, "gt0"),
    Field("slot_web", "slot web (to board)", "length", "mm", 0.5, 0.1, "gt0"),
    Field("pin_dia", "pin diameter", "length", "mm", 0.5, 0.1, "gt0"),
    Field("dot_dia", "dot diameter", "length", "mm", 0.5, 0.1, "gt0"),
    Field("dot_pitch", "dot pitch", "length", "mm", 0.5, 0.1, "gt0"),
    Field("dot_line_gap", "dotted line gap (between touching cells)", "length", "mm",
          0.5, 0.0, "ge0"),
    Field("hole_grid", "hole grid (holes datum, 0 = off)", "length", "mm",
          1.0, 0.0, "ge0"),
    Field("outer_border", "outer border", "bool"),
    Field("sort", "sort", "choice", choices=tuple(SORT_ORDERS)),
)

#: Rows that only mean something for one datum; they are dimmed under any
#: other one but stay editable, so a value can be set before switching over.
DATUM_ROWS: dict[str, str] = {
    "hole_dia": DATUM_HOLES,
    "hole_inset": DATUM_HOLES,
    "slot_width": DATUM_SLOTS,
    "slot_length": DATUM_SLOTS,
    "slot_offset": DATUM_SLOTS,
    "slot_pitch": DATUM_SLOTS,
    "slot_web": DATUM_SLOTS,
    "pin_dia": DATUM_SLOTS,
}


def field_applies(field: Field, params) -> bool:
    """Does this row matter for the datum the layout is currently set to?

    A row that does not (the hole rows under ``slots``, the slot rows under
    ``holes``, both under ``none``) is only drawn dimmed - it can still be
    stepped and edited.
    """
    want = DATUM_ROWS.get(field.attr)
    return want is None or want == getattr(params, "datum", None)


def value_text(field: Field, params) -> str:
    """Human readable current value of ``field``."""
    value = getattr(params, field.attr)
    if field.kind == "bool":
        return "on" if value else "off"
    if field.kind == "choice":
        return str(value)
    return (fmt_mm(value) + " " + field.unit).strip()


def valid_value(field: Field, value: float) -> bool:
    """Is ``value`` acceptable for ``field``?"""
    try:
        value = float(value)
    except (TypeError, ValueError):
        return False
    if value != value or value in (float("inf"), float("-inf")):
        return False
    if field.rule == "ge0":
        return value >= 0.0
    if field.rule == "gt0":
        return value > 0.0
    return True


def step_value(field: Field, value: float, direction: int) -> float:
    """``value`` moved by ``direction`` steps, clamped to the field minimum."""
    new = float(value) + direction * field.step
    if field.minimum is not None:
        new = max(field.minimum, new)
    new = round(new, 6)
    return 0.0 if new == 0 else new


def parse_number(text: str) -> Optional[float]:
    """Parse an inline edit buffer; None when it is not a finite number."""
    try:
        value = float(text.strip())
    except (TypeError, ValueError):
        return None
    if value != value or value in (float("inf"), float("-inf")):
        return None
    return value


def toggle_field(field: Field, params) -> None:
    """Space bar: flip a bool, cycle a choice. Numeric fields are untouched."""
    value = getattr(params, field.attr)
    if field.kind == "bool":
        setattr(params, field.attr, not value)
    elif field.kind == "choice":
        choices = field.choices or (value,)
        try:
            index = choices.index(value)
        except ValueError:
            index = -1
        setattr(params, field.attr, choices[(index + 1) % len(choices)])


def format_field_row(field: Field, params, *, cursor: bool = False,
                     editing: Optional[str] = None) -> str:
    """One row of the layout page (``editing`` = the inline edit buffer)."""
    value = f"{editing}_" if editing is not None else value_text(field, params)
    return f"{'→ ' if cursor else '  '}{_col(field.label, _COL_FIELD)} {value}"


def edit_buffer(buffer: str, key: int) -> Optional[str]:
    """Apply one key to an inline edit buffer.

    Returns the new buffer, or None when the key is not an editing key.
    """
    if key in (curses.KEY_BACKSPACE, 8, 127):
        return buffer[:-1]
    if 0 <= key < 0x110000:
        char = chr(key)
        if char.isdigit() or char in ".-":
            return buffer + char
    return None


# --------------------------------------------------------------------------- #
# Curses application
# --------------------------------------------------------------------------- #
class _App:
    """The four page curses front-end; all drawing and key dispatch."""

    def __init__(self, pads: list[Pad], sides: list[Side], config: Config,
                 on_preview: PreviewFunc, compute_layout: LayoutFunc,
                 title: str = "") -> None:
        self.pads = list(pads)
        self.sides = list(sides)
        self.config = config
        self.on_preview = on_preview
        self.compute_layout = compute_layout
        self.title = title
        has_candidates = any(pad.is_candidate for pad in self.pads)
        self.page = PAGE_PADS if (has_candidates or not sides) else PAGE_SIDES
        self.cursor = [0, 0, 0, 0]
        self.top = [0, 0, 0, 0]
        self.show_all = False                   # '*': list the pads with paste too
        self.visible: list[Pad] = []
        self._side_signature: Optional[tuple] = None
        self.layout: Optional[Layout] = None
        self.presets: Optional[dict[tuple[tuple[int, int], str], Optional[bool]]] = None
        self.status = ""
        self.pending: Optional[str] = None
        self.editing: Optional[str] = None      # inline edit buffer
        self.edit_fresh = False                 # buffer still holds the untouched prefill
        self.search: Optional[str] = None       # search query (None: not searching)
        self.filtered: Optional[list] = None    # the rows the query keeps
        self.match_counts: Optional[list[int]] = None   # words matched per filtered row
        self.search_anchor = None               # row focused when the search started
        self.colors = False
        self._sync_visible()

    # -- state ------------------------------------------------------------- #
    def _enabled_pads(self) -> list[Pad]:
        """Every pad of the enabled sides, the ones with paste included."""
        return visible_pads(self.pads, self.sides, True)

    def _sync_visible(self, keep: Optional[Pad] = None) -> None:
        """Recompute the visible pad list, keeping the cursor on the same pad."""
        if self.search is not None:
            return          # while searching the cursor indexes the filtered list
        signature = (self.show_all,
                     tuple(side.key for side in self.sides if side.enabled))
        if signature == self._side_signature and self.visible:
            return
        current = keep
        if current is None and 0 <= self.cursor[PAGE_PADS] < len(self.visible):
            current = self.visible[self.cursor[PAGE_PADS]]
        self._side_signature = signature
        self.visible = visible_pads(self.pads, self.sides, self.show_all)
        index = 0
        if current is not None:
            for i, pad in enumerate(self.visible):
                if pad is current:
                    index = i
                    break
            else:
                index = min(self.cursor[PAGE_PADS], max(0, len(self.visible) - 1))
        self.cursor[PAGE_PADS] = max(0, min(index, max(0, len(self.visible) - 1)))

    def _refresh_layout(self) -> None:
        try:
            self.layout = self.compute_layout(None)
        except Exception as exc:                   # noqa: BLE001 - shown to the user
            self.layout = None
            self.status = f"layout failed: {exc}"

    def _refresh_presets(self) -> None:
        presets: dict[tuple[tuple[int, int], str], Optional[bool]] = {}
        for size in STENCIL_SIZES:
            for orientation in ORIENTATIONS:
                cfg = self.config.copy()
                cfg.size = tuple(size)
                cfg.orientation = orientation
                try:
                    presets[(tuple(size), orientation)] = bool(self.compute_layout(cfg).fits)
                except Exception:                  # noqa: BLE001 - unknown fit
                    presets[(tuple(size), orientation)] = None
        self.presets = presets

    def _invalidate(self) -> None:
        """Something that influences the layout changed."""
        self._refresh_layout()
        self.presets = None

    def _base_rows(self) -> Sequence:
        """Every row of the current page, search filter *not* applied."""
        if self.page == PAGE_PADS:
            return self.visible
        if self.page == PAGE_SIDES:
            return self.sides
        if self.page == PAGE_STENCIL:
            return STENCIL_SIZES
        return LAYOUT_FIELDS

    def _rows_list(self) -> Sequence:
        """The rows the user sees: the filtered ones while searching."""
        if self.search is not None and self.filtered is not None:
            return self.filtered
        return self._base_rows()

    def _rows(self) -> int:
        return len(self._rows_list())

    @property
    def index(self) -> int:
        return max(0, min(self.cursor[self.page], max(0, self._rows() - 1)))

    @index.setter
    def index(self, value: int) -> None:
        self.cursor[self.page] = max(0, min(value, max(0, self._rows() - 1)))

    def _current_row(self):
        """The row under the cursor (of the filtered list while searching)."""
        rows = self._rows_list()
        if rows and 0 <= self.index < len(rows):
            return rows[self.index]
        return None

    @property
    def current_pad(self) -> Optional[Pad]:
        if self.page != PAGE_PADS:
            return None
        row = self._current_row()
        return row if isinstance(row, Pad) else None

    # -- colours ----------------------------------------------------------- #
    def _init_colors(self) -> None:
        if not curses.has_colors():
            return
        try:
            curses.start_color()
            curses.use_default_colors()
            curses.init_pair(_PAIR_UNDEFINED, curses.COLOR_YELLOW, -1)
            curses.init_pair(_PAIR_OPEN, curses.COLOR_GREEN, -1)
            curses.init_pair(_PAIR_IGNORE, curses.COLOR_BLUE, -1)
            curses.init_pair(_PAIR_HEADER, curses.COLOR_CYAN, -1)
            curses.init_pair(_PAIR_FITS, curses.COLOR_GREEN, -1)
            curses.init_pair(_PAIR_NOFIT, curses.COLOR_RED, -1)
            curses.init_pair(_PAIR_SEARCH, curses.COLOR_CYAN, -1)
        except curses.error:
            return
        self.colors = True

    def _state_attr(self, state: str) -> int:
        if not self.colors:
            return curses.A_DIM if state == STATE_IGNORE else curses.A_NORMAL
        if state == STATE_OPEN:
            return curses.color_pair(_PAIR_OPEN) | curses.A_BOLD
        if state == STATE_IGNORE:
            return curses.color_pair(_PAIR_IGNORE) | curses.A_DIM
        return curses.color_pair(_PAIR_UNDEFINED) | curses.A_BOLD

    def _kind_attr(self, kind: str) -> int:
        if kind == "fits":
            return (curses.color_pair(_PAIR_FITS) | curses.A_BOLD if self.colors
                    else curses.A_BOLD)
        if kind == "nofit":
            return (curses.color_pair(_PAIR_NOFIT) | curses.A_BOLD if self.colors
                    else curses.A_BOLD | curses.A_REVERSE)
        if kind == "header":
            return curses.color_pair(_PAIR_HEADER) if self.colors else curses.A_NORMAL
        if kind == "search":
            return (curses.color_pair(_PAIR_SEARCH) | curses.A_BOLD if self.colors
                    else curses.A_BOLD)
        return curses.A_NORMAL

    def _match_attr(self, attr: int, full: bool) -> int:
        """Bold a full match; bold wins over the dim of a pasted/disabled row."""
        if not full:
            return attr
        return (attr & ~curses.A_DIM) | curses.A_BOLD

    # -- search state ------------------------------------------------------ #
    def _query_words(self) -> int:
        """How many distinct words the query has (0 when not searching)."""
        return len(query_words(self.search)) if self.search is not None else 0

    def _full_count(self) -> int:
        """How many filtered rows match every word of the query."""
        if self.search is None or not self.match_counts:
            return 0
        return full_matches(self.match_counts, self.search)

    def _is_full(self, index: int) -> bool:
        """Does the filtered row at ``index`` match every word of the query?"""
        words = self._query_words()
        if not words or not self.match_counts:
            return False
        return 0 <= index < len(self.match_counts) \
            and self.match_counts[index] >= words

    # -- drawing ----------------------------------------------------------- #
    @staticmethod
    def _put(win, y: int, x: int, text: str, attr: int = curses.A_NORMAL) -> int:
        """Write ``text`` clipped to the window; never raises. Returns the new x."""
        try:
            height, width = win.getmaxyx()
        except curses.error:
            return x
        if y < 0 or y >= height or x < 0 or x >= width - 1:
            return x
        text = clip(text, max(0, width - 1 - x))
        if not text:
            return x
        try:
            win.addstr(y, x, text, attr)
        except (curses.error, UnicodeError):
            return x
        return x + len(text)

    def _draw(self, win) -> None:
        try:
            height, width = win.getmaxyx()
        except curses.error:
            return
        try:
            win.erase()
        except curses.error:
            pass
        if height <= 0 or width <= 1:
            self._refresh(win)
            return

        head_n = 2 if height >= 7 else (1 if height >= 3 else 0)
        fit_n = 1 if height >= 5 else 0
        foot_w = max(0, width - 1)
        # The key help takes one or two lines; the list area shrinks by as much.
        help_lines = self._footer_lines(foot_w)
        help_n = 0
        if height >= 2 and help_lines:
            help_n = min(len(help_lines), max(1, height - head_n - fit_n - 1))
            if help_n < len(help_lines):        # no room: re-wrap into what is left
                help_lines = self._footer_lines(foot_w, help_n)
        body_h = max(0, height - head_n - fit_n - help_n)

        if head_n >= 1:
            self._draw_tabs(win, width)
        if head_n >= 2:
            self._put(win, 1, 0, clip(self._subheader(), max(0, width - 1)),
                      curses.A_BOLD | self._kind_attr("header"))

        self._scroll(body_h)
        if body_h > 0:
            self._draw_body(win, head_n, body_h, width)

        if fit_n:
            self._draw_fit(win, height - help_n - 1, width)
        for row, text in enumerate(help_lines[:help_n]):
            self._draw_footer(win, height - help_n + row, text, foot_w, first=row == 0)
        self._refresh(win)

    @staticmethod
    def _refresh(win) -> None:
        try:
            win.noutrefresh()
            curses.doupdate()
        except curses.error:
            pass

    def _search_prompt(self) -> str:
        """The ``/query_`` prompt of the footer while searching."""
        return f"/{self.search}_"

    def _help_items(self) -> list[str]:
        """The key-help items of the footer as it is right now."""
        if self.search is not None:
            return [self._search_prompt(), *SEARCH_ITEMS]
        if self.editing is not None:
            return list(EDIT_ITEMS)
        return list(PAGE_ITEMS[self.page])

    def _footer_lines(self, width: int,
                      max_lines: int = FOOTER_LINES) -> list[str]:
        """The footer text laid out: a status message, or the key help.

        The key help is wrapped onto up to ``max_lines`` lines instead of being
        cut off; a status message wraps the same way.
        """
        if self.search is None and self.status:
            return wrap_text(self.status, width, max_lines)
        return wrap_items(self._help_items(), width, max_lines)

    def _draw_footer(self, win, y: int, text: str, width: int,
                     first: bool = False) -> None:
        """One footer line; the search prompt of the first line is highlighted."""
        x = 0
        prompt = self._search_prompt() if (first and self.search is not None) else ""
        if prompt and text.startswith(prompt):
            x = self._put(win, y, 0, prompt,
                          self._kind_attr("search") | curses.A_REVERSE)
            text = text[len(prompt):]
        self._put(win, y, x, text.ljust(max(0, width - x)), curses.A_REVERSE)

    def _draw_tabs(self, win, width: int) -> None:
        x = 0
        if self.title:
            x = self._put(win, 0, 0, clip(self.title + "   ", max(0, width // 2)),
                          curses.A_BOLD | self._kind_attr("header"))
        for i, name in enumerate(PAGE_NAMES):
            attr = curses.A_REVERSE | curses.A_BOLD if i == self.page else curses.A_NORMAL
            x = self._put(win, 0, x, f" {i + 1} {name} ", attr)
            x = self._put(win, 0, x, "  ")

    def _subheader(self) -> str:
        if self.search is not None:
            tail = match_summary(len(self._rows_list()), len(self._base_rows()),
                                 self._full_count(), self._query_words())
            return f"filter: {self.search}  {tail}"
        if self.page == PAGE_PADS:
            total = len(self.visible)
            position = f"{self.index + 1}/{total}" if total else "0/0"
            if self.show_all:
                what = (f"all pads ({total}) — pads with paste are dimmed; "
                        f"ignore closes their opening")
                hint = "* = back to pads to decide"
                hidden = len(self.pads) - total
            else:
                what = f"pads to decide ({total})"
                hint = "* = show all pads"
                hidden = sum(1 for pad in self.pads if pad.is_candidate) - total
            extra = f"   ({hidden} hidden on disabled sides)" if hidden > 0 else ""
            return f"pads {position}   {what}   {hint}{extra}"
        if self.page == PAGE_SIDES:
            enabled = sum(1 for side in self.sides if side.enabled)
            return f"sides: {enabled}/{len(self.sides)} enabled"
        if self.page == PAGE_STENCIL:
            sheet_w, sheet_h = self.config.sheet_size()
            text = (f"sheet {sheet_w:.1f} x {sheet_h:.1f} mm   "
                    f"orientation {self.config.orientation}")
            if self.layout is not None:
                text += (f"   block {self.layout.block_width:.1f} x "
                         f"{self.layout.block_height:.1f} mm")
            return text
        return "layout parameters"

    def _draw_body(self, win, y0: int, body_h: int, width: int) -> None:
        if self.page == PAGE_PADS:
            self._draw_pads(win, y0, body_h, width)
        elif self.page == PAGE_SIDES:
            self._draw_sides(win, y0, body_h, width)
        elif self.page == PAGE_STENCIL:
            self._draw_stencil(win, y0, body_h)
        else:
            self._draw_layout(win, y0, body_h)

    def _draw_pads(self, win, y0: int, body_h: int, width: int) -> None:
        rows_list = self._rows_list()
        if not rows_list:
            if self.search:
                message = "no pad matches the filter"
            elif self.show_all:
                message = ("no pads on the enabled sides"
                           if self.pads else "no pads at all")
            else:
                message = ("no pads to decide on the enabled sides"
                           if any(pad.is_candidate for pad in self.pads)
                           else "no candidate pads at all")
            self._put(win, y0, 2, message, curses.A_DIM)
            return
        rows = body_h
        if rows >= 3:
            self._put(win, y0, 0, pads_header(), curses.A_DIM | curses.A_UNDERLINE)
            y0 += 1
            rows -= 1
        for row in range(rows):
            index = self.top[self.page] + row
            if index >= len(rows_list):
                break
            pad = rows_list[index]
            cursor = index == self.index
            full = self._is_full(index)          # matches every word: drawn bold
            base = curses.A_REVERSE if cursor else curses.A_NORMAL
            x = 0
            for text, kind in row_segments(pad, cursor=cursor):
                attr = base
                if kind == "state":
                    attr = self._state_attr(pad.state) | base
                elif kind == "pad" and pad.has_paste:
                    attr = base | curses.A_DIM   # it already has an opening
                x = self._put(win, y0 + row, x, text, self._match_attr(attr, full))
            if cursor and x < width - 1:
                self._put(win, y0 + row, x, " " * (width - 1 - x),
                          self._match_attr(base, full))

    def _draw_sides(self, win, y0: int, body_h: int, width: int) -> None:
        rows_list = self._rows_list()
        if not rows_list:
            self._put(win, y0, 2,
                      "no side matches the filter" if self.search else "no sides",
                      curses.A_DIM)
            return
        for row in range(body_h):
            index = self.top[self.page] + row
            if index >= len(rows_list):
                break
            side = rows_list[index]
            cursor = index == self.index
            attr = curses.A_REVERSE if cursor else curses.A_NORMAL
            if not side.enabled:
                attr |= curses.A_DIM
            attr = self._match_attr(attr, self._is_full(index))
            text = side_row(side, cursor=cursor)
            self._put(win, y0 + row, 0, text.ljust(max(0, width - 1)), attr)

    def _draw_stencil(self, win, y0: int, body_h: int) -> None:
        presets = self.presets or {}
        for row in range(body_h):
            index = self.top[self.page] + row
            if index >= len(STENCIL_SIZES):
                break
            size = STENCIL_SIZES[index]
            fits = {o: presets.get((tuple(size), o)) for o in ORIENTATIONS}
            cursor = index == self.index
            attr = curses.A_REVERSE if cursor else curses.A_NORMAL
            if tuple(size) == tuple(self.config.size):
                attr |= curses.A_BOLD
            self._put(win, y0 + row, 0, preset_row(size, fits, self.config, cursor=cursor),
                      attr)

    def _draw_layout(self, win, y0: int, body_h: int) -> None:
        for row in range(body_h):
            index = self.top[self.page] + row
            if index >= len(LAYOUT_FIELDS):
                break
            field = LAYOUT_FIELDS[index]
            cursor = index == self.index
            editing = self.editing if (cursor and self.editing is not None) else None
            attr = curses.A_REVERSE if cursor else curses.A_NORMAL
            if not field_applies(field, self.config.layout):
                attr |= curses.A_DIM
            self._put(win, y0 + row, 0,
                      format_field_row(field, self.config.layout, cursor=cursor,
                                       editing=editing), attr)

    def _draw_fit(self, win, y: int, width: int) -> None:
        x = 0
        # Count every pad of the enabled sides, not only the listed ones, so
        # the "closed" number stays visible in the pads-to-decide view.
        for text, kind in fit_segments(self.config, self.layout, self._enabled_pads()):
            x = self._put(win, y, x, text, self._kind_attr(kind))

    def _scroll(self, body_h: int) -> None:
        """Keep the cursor row inside the visible window."""
        rows = self._rows()
        list_h = body_h
        if self.page == PAGE_PADS and body_h >= 3:
            list_h = body_h - 1                    # column header
        index = self.index
        top = self.top[self.page]
        if list_h <= 0:
            self.top[self.page] = index
            return
        if index < top:
            top = index
        elif index >= top + list_h:
            top = index - list_h + 1
        self.top[self.page] = max(0, min(top, max(0, rows - list_h)))

    # -- actions ----------------------------------------------------------- #
    def _move(self, delta: int) -> None:
        self.index = self.index + delta
        self.status = ""

    def _goto(self, page: int) -> None:
        self.page = max(0, min(3, page))
        self.status = ""
        if self.page == PAGE_PADS:
            self._sync_visible()
        if self.page == PAGE_STENCIL and self.presets is None:
            self._refresh_presets()

    def _jump_undefined(self, forward: bool) -> None:
        rows = self._rows_list()
        index = find_undefined(rows, self.index, forward)
        if index is None:
            self.status = "no undefined pads left"
        else:
            self.index = index
            self.status = f"undefined pad {index + 1}/{len(rows)}"

    def _set_state(self, state: str) -> None:
        pad = self.current_pad
        if pad is None:
            return
        if not allowed_state(pad, state):      # a pasted pad is never undefined
            self.status = f"{pad.label} has paste: it is only open or ignore"
            return
        pad.state = state
        note = ""
        if pad.has_paste:
            note = (" — paste opening closed" if pad.state == STATE_IGNORE
                    else " — paste opening kept")
        self.status = f"{pad.label} ({pad.project} {pad.side}) -> {state}{note}"

    def _apply_component(self) -> None:
        pad = self.current_pad
        if pad is None:
            return
        changed = apply_component(self.visible, pad)
        self.status = f"{pad.ref}: {changed} pad(s) -> {pad.state}"

    def _toggle_show_all(self) -> None:
        """``*``: list every pad of the enabled sides, or only the candidates."""
        keep = self.current_pad
        self.show_all = not self.show_all
        self._sync_visible(keep)
        if self.show_all:
            self.status = ("showing all pads — the ones with paste are dimmed; "
                           "set one to ignore to close its opening")
        else:
            self.status = "showing the pads to decide"

    def _toggle_side(self, side: Side) -> None:
        side.enabled = not side.enabled
        self.status = f"{side.label}: {'enabled' if side.enabled else 'disabled'}"
        self._sync_visible()
        self._invalidate()

    def _set_all_sides(self, enabled: bool) -> None:
        for side in self.sides:
            side.enabled = enabled
        self.status = f"all sides {'enabled' if enabled else 'disabled'}"
        self._sync_visible()
        self._invalidate()

    def _select_size(self, size: tuple[int, int]) -> None:
        self.config.size = tuple(size)
        self.status = f"stencil size {self.config.size_label}"
        self._invalidate()
        self._refresh_presets()

    def _toggle_orientation(self) -> None:
        index = ORIENTATIONS.index(self.config.orientation) \
            if self.config.orientation in ORIENTATIONS else -1
        self.config.orientation = ORIENTATIONS[(index + 1) % len(ORIENTATIONS)]
        self.status = f"orientation {self.config.orientation}"
        self._invalidate()
        if self.page == PAGE_STENCIL:
            self._refresh_presets()

    def _step_field(self, field: Field, direction: int) -> None:
        params = self.config.layout
        if not field.numeric:
            if direction > 0 or field.kind == "bool":
                toggle_field(field, params)
            else:
                choices = field.choices or ()
                if field.kind == "choice" and choices:
                    value = getattr(params, field.attr)
                    i = choices.index(value) if value in choices else 0
                    setattr(params, field.attr, choices[(i - 1) % len(choices)])
                else:
                    toggle_field(field, params)
            self.status = f"{field.label}: {value_text(field, params)}"
            self._invalidate()
            return
        new = step_value(field, float(getattr(params, field.attr)), direction)
        if not valid_value(field, new):
            self.status = f"{field.label}: {fmt_mm(new)} is not allowed"
            return
        setattr(params, field.attr, new)
        self.status = f"{field.label}: {value_text(field, params)}"
        self._invalidate()

    def _toggle_field(self, field: Field) -> None:
        if field.numeric:
            self._start_edit(field)
            return
        toggle_field(field, self.config.layout)
        self.status = f"{field.label}: {value_text(field, self.config.layout)}"
        self._invalidate()

    def _start_edit(self, field: Field) -> None:
        if not field.numeric:
            self._toggle_field(field)
            return
        self.editing = fmt_mm(getattr(self.config.layout, field.attr))
        self.edit_fresh = True   # first typed character replaces the prefilled value
        self.status = (f"editing {field.label} — type replaces the value, backspace edits, "
                       "enter accepts, esc cancels")

    def _edit_key(self, key: int) -> None:
        field = LAYOUT_FIELDS[self.index]
        if key in (10, 13, curses.KEY_ENTER):
            value = parse_number(self.editing or "")
            if value is None or not valid_value(field, value):
                self.status = f"invalid value {self.editing!r} for {field.label}"
                return
            setattr(self.config.layout, field.attr, round(value, 6))
            self.editing = None
            self.status = f"{field.label}: {value_text(field, self.config.layout)}"
            self._invalidate()
            return
        if key == 27:
            self.editing = None
            self.status = "edit cancelled"
            return
        base = self.editing or ""
        if self.edit_fresh and key not in (curses.KEY_BACKSPACE, 8, 127):
            base = ""
        buffer = edit_buffer(base, key)
        if buffer is not None:
            self.edit_fresh = False
            self.editing = buffer
            self.status = ""

    # -- search ------------------------------------------------------------ #
    def _searchable(self) -> bool:
        """Only the two list pages can be searched."""
        return self.page in (PAGE_PADS, PAGE_SIDES)

    def _start_search(self) -> None:
        """``/``: enter search mode with an empty (everything passes) query."""
        self.search = ""
        self.search_anchor = self._current_row()
        self.status = ""
        self._apply_filter(self.search_anchor)

    def _apply_filter(self, keep=None) -> None:
        """Re-filter the list for the current query, staying on ``keep``.

        The rows are re-ordered (best match first), so the cursor follows
        ``keep`` by identity wherever it landed; when it dropped out of the
        filter the cursor goes to the first - the best - match.
        """
        if self.search is None:
            self.filtered = None
            self.match_counts = None
            return
        if self.page == PAGE_PADS:
            pairs = filter_counts(self.visible, pad_fields, self.search)
        elif self.page == PAGE_SIDES:
            pairs = filter_counts(self.sides, side_fields, self.search)
        else:                                  # not reachable: / is list only
            self.filtered = None
            self.match_counts = None
            return
        self.filtered = [row for row, _ in pairs]
        self.match_counts = [count for _, count in pairs]
        self.cursor[self.page] = restore_index(self.filtered, keep, 0)

    def _end_search(self) -> None:
        """Enter/Esc: drop the filter but stay on the focused item."""
        target = self._current_row()
        if target is None:                     # nothing matched: the old item
            target = self.search_anchor
        fallback = self.cursor[self.page]
        self.search = None
        self.filtered = None
        self.match_counts = None
        self.search_anchor = None
        self.cursor[self.page] = restore_index(self._rows_list(), target, fallback)
        self.status = ""

    def _search_key(self, win, key: int) -> None:
        """One key while searching: navigate, edit the query or leave."""
        if key in (10, 13, curses.KEY_ENTER, 27):
            self._end_search()
            return
        if key in (curses.KEY_BACKSPACE, 8, 127):
            if self.search:
                current = self._current_row()
                self.search = self.search[:-1]
                self._apply_filter(current)
            return
        if key == curses.KEY_UP:
            self._move(-1)
            return
        if key == curses.KEY_DOWN:
            self._move(1)
            return
        if key == curses.KEY_PPAGE:
            self._move(-max(1, win.getmaxyx()[0] - 5))
            return
        if key == curses.KEY_NPAGE:
            self._move(max(1, win.getmaxyx()[0] - 5))
            return
        if key == curses.KEY_HOME:
            self.index = 0
            return
        if key == curses.KEY_END:
            self.index = self._rows() - 1
            return
        char = search_char(key)
        if char is not None:
            current = self._current_row()
            self.search += char
            self._apply_filter(current)

    def _preview(self, win, do_open: bool) -> None:
        pad = self.current_pad if self.page == PAGE_PADS else None
        self.status = "rendering…"
        self._draw(win)
        try:
            path = self.on_preview(pad, do_open)
        except Exception as exc:                   # noqa: BLE001 - shown to the user
            self.status = f"preview failed: {exc}"
            return
        verb = "preview opened" if do_open else "preview updated"
        self.status = f"{verb}: {path}"

    def _confirm(self, keyname: str, pending: Optional[str]) -> Optional[bool]:
        """True to generate, None when a second confirmation is required."""
        layout = self.layout
        if layout is not None and not layout.fits and pending != keyname:
            self.pending = keyname
            self.status = NOFIT_WARNING
            return None
        return True

    # -- key dispatch ------------------------------------------------------ #
    def _handle(self, win, key: int) -> Optional[bool]:
        """Handle one key; None to keep running, True/False to leave the loop."""
        if self.editing is not None:
            self._edit_key(key)
            return None
        if self.search is not None:
            self._search_key(win, key)
            return None

        pending, self.pending = self.pending, None

        if key == ord("/") and self._searchable():
            self._start_search()
            return None

        if key in (ord("1"), ord("2"), ord("3"), ord("4")):
            self._goto(key - ord("1"))
            return None
        if key == 9:                                   # Tab
            self._goto((self.page + 1) % len(PAGE_NAMES))
            return None
        if key in (curses.KEY_BTAB, 353):              # Shift-Tab
            self._goto((self.page - 1) % len(PAGE_NAMES))
            return None
        if key in (27, ord("q")):
            return False
        if key == ord("p"):
            self._preview(win, False)
            return None
        if key == ord("v"):
            self._preview(win, True)
            return None
        if key == ord("g") and self.page != PAGE_PADS:
            return self._confirm("g", pending)

        if key in (curses.KEY_UP, ord("k")):
            self._move(-1)
            return None
        if key in (curses.KEY_DOWN, ord("j")):
            self._move(1)
            return None
        if key == curses.KEY_PPAGE:
            self._move(-max(1, win.getmaxyx()[0] - 5))
            return None
        if key == curses.KEY_NPAGE:
            self._move(max(1, win.getmaxyx()[0] - 5))
            return None
        if key == curses.KEY_HOME:
            self.index = 0
            self.status = ""
            return None
        if key == curses.KEY_END:
            self.index = self._rows() - 1
            self.status = ""
            return None

        if self.page == PAGE_PADS:
            return self._handle_pads(key, pending)
        if self.page == PAGE_SIDES:
            return self._handle_sides(key, pending)
        if self.page == PAGE_STENCIL:
            return self._handle_stencil(key, pending)
        return self._handle_layout(key, pending)

    def _handle_pads(self, key: int, pending: Optional[str]) -> Optional[bool]:
        if key == ord("g"):                # Home on this page (enter generates)
            self.index = 0
            self.status = ""
        elif key == ord("G"):
            self.index = self._rows() - 1
            self.status = ""
        elif key == ord("*"):
            self._toggle_show_all()
        elif key == ord(" "):
            pad = self.current_pad
            if pad is not None:
                self._set_state(next_state(pad))
        elif key == ord("o"):
            self._set_state(STATE_OPEN)
        elif key == ord("i"):
            self._set_state(STATE_IGNORE)
        elif key == ord("a"):
            self._apply_component()
        elif key == ord("n"):
            self._jump_undefined(True)
        elif key == ord("N"):
            self._jump_undefined(False)
        elif key in (10, 13, curses.KEY_ENTER):
            return self._confirm("enter", pending)
        return None

    def _handle_sides(self, key: int, pending: Optional[str]) -> Optional[bool]:
        if key in (ord(" "), 10, 13, curses.KEY_ENTER):
            if self.sides:
                self._toggle_side(self.sides[self.index])
        elif key == ord("A"):
            self._set_all_sides(True)
        elif key == ord("N"):
            self._set_all_sides(False)
        return None

    def _handle_stencil(self, key: int, pending: Optional[str]) -> Optional[bool]:
        if key in (ord(" "), 10, 13, curses.KEY_ENTER):
            self._select_size(STENCIL_SIZES[self.index])
        elif key == ord("o"):
            self._toggle_orientation()
        return None

    def _handle_layout(self, key: int, pending: Optional[str]) -> Optional[bool]:
        field = LAYOUT_FIELDS[self.index]
        if key in (ord("+"), ord("="), curses.KEY_RIGHT):
            self._step_field(field, 1)
        elif key in (ord("-"), ord("_"), curses.KEY_LEFT):
            self._step_field(field, -1)
        elif key == ord(" "):
            if field.numeric:
                self.status = f"{field.label} is a number — press e or enter to edit"
            else:
                self._toggle_field(field)
        elif key in (ord("e"), 10, 13, curses.KEY_ENTER):
            self._start_edit(field)
        return None

    # -- main loop --------------------------------------------------------- #
    def run(self, win) -> bool:
        """Event loop; True when the user confirmed, False when they quit."""
        try:
            curses.curs_set(0)
        except curses.error:
            pass
        self._init_colors()
        win.keypad(True)
        self._refresh_layout()
        if self.page == PAGE_STENCIL:
            self._refresh_presets()
        while True:
            self._sync_visible()
            if self.page == PAGE_STENCIL and self.presets is None:
                self._refresh_presets()
            self._draw(win)
            try:
                key = win.getch()
            except KeyboardInterrupt:
                return False
            except curses.error:
                continue
            if key == curses.ERR:
                continue
            if key == curses.KEY_RESIZE:
                try:                               # force a full repaint
                    curses.update_lines_cols()
                    win.clear()
                except (curses.error, AttributeError):
                    pass
                continue
            result = self._handle(win, key)
            if result is not None:
                return result


def run_tui(pads: list[Pad], sides: list[Side], config: Config, *,
            on_preview: PreviewFunc,
            compute_layout: LayoutFunc,
            title: str = "") -> bool:
    """Set up the stencil in a curses UI; True when confirmed, False when aborted.

    ``pads`` are *all* pads (already sorted): the candidates are listed by
    default, the ones that already have a paste opening are listed on ``*`` so
    they can be closed.  ``sides`` are all relevant
    sides (enabled or not) and ``config`` the live configuration.  Pad states,
    side switches and the configuration (size, orientation, layout parameters)
    are mutated in place; ``config.sides`` / ``config.pads`` are never touched.

    ``on_preview(pad, open_it)`` re-renders the preview image and returns its
    path.  ``compute_layout(cfg)`` returns the :class:`~pcbstencil.model.Layout`
    for ``cfg`` (or for the live configuration when ``cfg`` is None).

    With neither pads nor sides there is nothing to decide, so the call
    succeeds immediately without touching the terminal.
    """
    if not pads and not sides:
        return True
    app = _App(pads, sides, config, on_preview, compute_layout, title)
    return bool(curses.wrapper(app.run))
