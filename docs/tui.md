# The TUI

`pcbstencil/tui.py` is the curses front-end that runs between the first preview
and the generation step. `run_tui(pads, sides, config, on_preview=...,
compute_layout=..., title=...)` is its only entry point. It mutates the live
objects in place — `pad.state`, `side.enabled`, `config.size`,
`config.orientation` and the fields of `config.layout` — and never touches
`config.sides` or `config.pads`; the caller collects those from the objects
afterwards with `collect_config` (see [pads-and-config.md](pads-and-config.md)).
It returns `True` when the user asked to generate and `False` when they quit,
and with neither pads nor sides it returns `True` immediately without touching
the terminal at all.

## Structure

Everything above `class _App` is pure: no curses call, no global state, no I/O.
That is deliberate — those functions carry the whole behaviour of the UI and can
be exercised without a terminal. `_App` only draws and dispatches keys.

| group | functions | what they do |
| --- | --- | --- |
| pad states | `cycle_state`, `next_state`, `allowed_state`, `count_states`, `component_pads`, `apply_component`, `find_undefined`, `visible_pads` | which state a key gives a pad, which states a pad may take, and which pads are listed |
| text | `clip`, `_col`, `fmt_mm`, `ellipsis`, `_wrap`, `wrap_items`, `wrap_text` | column padding, number formatting and footer line breaking |
| search | `query_words`, `match_count`, `matches`, `filter_counts`, `filter_rows`, `full_matches`, `match_summary`, `search_char`, `restore_index` | the whole filter: matching, ranking, the sub-header tail and cursor preservation |
| rows | `pad_position`, `board_text`, `pad_fields`, `side_fields`, `row_segments`, `format_row`, `pads_header`, `side_row`, `orientation_size`, `preset_row`, `fit_segments`, `fit_line` | the exact text of every row and of the status line |
| layout fields | `Field`, `LAYOUT_FIELDS`, `DATUM_ROWS`, `field_applies`, `value_text`, `valid_value`, `step_value`, `parse_number`, `toggle_field`, `format_field_row`, `edit_buffer` | the editable parameters of the Layout page, their validation and which of them the current datum applies to |

### Draw and dispatch

`_App.run(win)` is the loop:

```
curs_set(0), _init_colors(), keypad(True), _refresh_layout()
loop:
    _sync_visible()                    # the pad list follows the enabled sides
    _refresh_presets() if on the Stencil page and the cache is empty
    _draw(win)
    key = win.getch()                  # ERR -> continue, KEY_RESIZE -> repaint
    result = _handle(win, key)         # None keeps running, True/False leaves
```

`_draw` first works out how much room each band gets, from the outside in:

| band | rows | condition |
| --- | --- | --- |
| tabs | 1 | height >= 3 |
| sub-header | 1 | height >= 7 |
| body | the rest | — |
| fit line | 1 | height >= 5 |
| footer | 1 or 2 | whatever `_footer_lines` produced, capped by what is left |

The footer is wrapped first (`_footer_lines`), and when the result does not fit
into the remaining rows it is wrapped again into fewer lines, so the body never
disappears because the key help is long. `_draw` then calls `_draw_tabs`, writes
the sub-header, calls `_scroll` and delegates the body to `_draw_pads`,
`_draw_sides`, `_draw_stencil` or `_draw_layout`.

`_handle` dispatches the global keys first (search, page switching, quit,
preview, `g`, movement) and hands anything left to `_handle_pads`,
`_handle_sides`, `_handle_stencil` or `_handle_layout`. Inline editing and
search mode short-circuit the whole dispatch at the top: while `self.editing` is
not `None` every key goes to `_edit_key`, and while `self.search` is not `None`
every key goes to `_search_key`.

The cursor and the scroll offset are per page: `self.cursor` and `self.top` are
four-element lists, so switching pages and coming back keeps the position.
`_scroll` moves `top` the least it can to keep the cursor row visible; on the
Pads page it subtracts one row for the column header, but only when the body is
at least 3 rows tall (below that the header is not drawn either).

## The four pages

The session starts on Pads when any pad is a candidate or when there are no
sides at all, otherwise on Sides:

```python
has_candidates = any(pad.is_candidate for pad in self.pads)
self.page = PAGE_PADS if (has_candidates or not sides) else PAGE_SIDES
```

### 1. Pads

Lists the pads of the *enabled* sides: by default only the candidates (copper
without paste), and with `*` every pad, so the ones that already have a paste
opening can be closed. The column header comes from `pads_header()`; a row is
built by `row_segments` as four chunks — the cursor arrow, the state field, the
pad field and the rest — so the state column can be coloured and a pasted pad
dimmed independently. The first row below has paste, the second is a candidate:

```
  state         pad          project              side    function      shape            position
→ + open      · D1.1         RBARF                top     SMDPad        R 0.70x0.70      x= 148.775 y= -96.120
  - ignore      TP1.1        RBARF                bottom  SMDPad        C ⌀1.00          x= 148.082 y= -99.568
```

The state marker is `?` undefined, `+` open, `-` ignore (`STATE_MARKERS`), and
`PASTE_MARK` (`· `) sits in front of the reference of a pad that already has
paste, so a terminal without colours can still tell them apart. The sub-header
shows the position in the list, the mode (`pads to decide (n)` or
`all pads (n) — pads with paste are dimmed; ignore closes their opening`), the
`*` hint and, when some pads are hidden because their side is switched off,
`(n hidden on disabled sides)`.

### 2. Sides

One row per relevant side, `side_row`: a `[x]` / `[ ]` switch, the project and
side name with `(mirrored)` appended, the board size, the number of paste
openings, the number of pads to decide and how many of those are still
undefined. The sub-header counts the enabled sides. Rows of disabled sides are
dimmed.

### 3. Stencil

One row per entry of `STENCIL_SIZES` — currently eight: 270x270, 380x280,
420x320, 450x350, 460x460, 520x420, 600x600 and 700x600 mm, long side first.
(The README still lists seven and omits 270x270; the code is the current state.)
`preset_row` marks the selected size with `●`, prints the sheet size in both
orientations and, for each, `fits` / `NO` / `?` from the preset cache (`?` means
packing raised for that combination and the fit is unknown):

```
→   270 x 270 mm   landscape: 270 x 270  NO      portrait: 270 x 270  NO
  ● 380 x 280 mm   landscape: 380 x 280  NO      portrait: 280 x 380  fits       ← landscape
    420 x 320 mm   landscape: 420 x 320  fits    portrait: 320 x 420  fits
```

(Real output for the eight sample projects with all 13 sides enabled: the
default sheet only takes them in portrait.) 270x270 is square, so its two
orientations describe the same sheet and always agree. The sub-header shows the
current sheet size, the orientation and the block size.

### 4. Layout

One row per `Field` of `LAYOUT_FIELDS`, rendered by `format_field_row` as label
plus value (or the edit buffer with a trailing `_` while editing):

| attr | label | kind | unit | step | minimum | rule |
| --- | --- | --- | --- | --- | --- | --- |
| `gap` | spacing (gap between boards) | length | mm | 0.5 | 0.0 | `ge0` |
| `datum` | datum (alignment features) | choice | — | — | — | slots \| holes \| none |
| `hole_dia` | hole diameter | length | mm | 0.5 | 0.1 | `gt0` |
| `hole_inset` | hole inset (dotted line to hole edge) | length | mm | 0.5 | none | `any` |
| `slot_width` | slot width | length | mm | 0.5 | 0.1 | `gt0` |
| `slot_length` | slot length | length | mm | 0.5 | 0.1 | `gt0` |
| `slot_offset` | slot offset (edge to outer wall) | length | mm | 0.5 | 0.0 | `ge0` |
| `slot_pitch` | slot pitch (modular jig raster) | length | mm | 5.0 | 1.0 | `gt0` |
| `slot_web` | slot web (to board) | length | mm | 0.5 | 0.1 | `gt0` |
| `pin_dia` | pin diameter | length | mm | 0.5 | 0.1 | `gt0` |
| `dot_dia` | dot diameter | length | mm | 0.5 | 0.1 | `gt0` |
| `dot_pitch` | dot pitch | length | mm | 0.5 | 0.1 | `gt0` |
| `dot_line_gap` | dotted line gap (between touching cells) | length | mm | 0.5 | 0.0 | `ge0` |
| `hole_grid` | hole grid (holes datum, 0 = off) | length | mm | 1.0 | 0.0 | `ge0` |
| `outer_border` | outer border | bool | — | — | — | — |
| `sort` | sort | choice | — | — | — | height \| name |

`hole_inset` is the only unbounded number: it may go negative, which puts the
dowel hole onto the dotted line instead of beside it. `slot_pitch` is the only
row that steps by more than 0.5 mm: it moves in 5 mm steps and clamps at 1 mm,
because it is a jig raster, not a fit-and-finish dimension — every cell is
rounded up to a whole number of it, so a small change moves the whole sheet. `Field.numeric` is true
only for `kind == "length"`, which is what decides between stepping/editing and
toggling. See [data-model.md](data-model.md) for what each parameter means.

**The datum row and dimming.** `datum` is an ordinary `choice` field, so space,
`+`/`→`, `e` and Enter all cycle it (`-`/`←` cycles backwards) through
slots → holes → none. It also decides which of the other rows are *relevant*:
`DATUM_ROWS` maps each datum-specific attribute to the mode it belongs to and
`field_applies(field, params)` answers whether a row matters right now — the
two hole rows under `slots`, the six slot rows (`slot_width`, `slot_length`,
`slot_offset`, `slot_pitch`, `slot_web`, `pin_dia`) under `holes`, and both
groups under `none`, are drawn with `curses.A_DIM` added to their attribute
(the cursor's `A_REVERSE` still wins visually). The rows that are shared by
every datum — spacing, the dots, the hole grid, the outer border and the sort
order — are never dimmed; `hole_grid` only bites under `holes`, which its label
says.

Dimming is presentation only. A dimmed row still steps, still edits and still
saves: that is deliberate, so a slot size can be dialled in before switching
the datum over, and so an old configuration's `hole_dia` is not silently lost
while the default `slots` datum is active.

### The status line

Above the footer, on every page, `fit_segments` draws: the stencil size and
orientation, the block size, `FITS` or `DOES NOT FIT` (or `NO LAYOUT` when
packing failed), and the pad counts — undefined, open, ignore and closed. The
counts come from `count_states` over *every* pad of the enabled sides
(`_enabled_pads`), not only the listed ones, so `closed` stays visible in the
default pads-to-decide view where no pasted pad is listed at all.

## Keys

Global, handled before the page dispatch:

| key | action |
| --- | --- |
| `/` | start a search (Pads and Sides pages only) |
| `1` `2` `3` `4` | go to that page |
| Tab | next page; Shift-Tab (`KEY_BTAB` or 353) previous page |
| `q`, Esc | quit without generating |
| `p` | re-render the preview |
| `v` | re-render the preview and open it in the viewer |
| `g` | generate — but only when the page is *not* Pads |
| `↑` `k`, `↓` `j` | move one row |
| PgUp / PgDn | move by window height minus 5 rows |
| Home / End | first / last row |

Pads page:

| key | action |
| --- | --- |
| space | `next_state`: cycle the pad under the cursor |
| `o` / `i` | set it open / ignore |
| `a` | apply the cursor pad's state to every pad of the same component |
| `n` / `N` | next / previous undefined candidate, wrapping around |
| `*` | show all pads, including the ones with paste |
| `g` / `G` | jump to the first / last row (not generate) |
| Enter | generate |

Sides page:

| key | action |
| --- | --- |
| space, Enter | toggle the side under the cursor |
| `A` / `N` | all sides on / all sides off |

Stencil page:

| key | action |
| --- | --- |
| space, Enter | pick the size under the cursor |
| `o` | toggle the orientation |

Layout page:

| key | action |
| --- | --- |
| `+` `=` `→` | step the value up (or toggle / cycle forwards) |
| `-` `_` `←` | step the value down (or toggle / cycle backwards) |
| space | toggle a switch or cycle a choice (`datum`, `sort`); on a number it only prints a hint |
| `e`, Enter | start editing the number inline; on a choice row it cycles instead |

Two of these deviate from the obvious: on the Pads page `g` and `G` are Home and
End because Enter already generates there, and `o` opens a pad on the Pads page
but toggles the orientation on the Stencil page. `N` is "previous undefined" on
Pads and "all sides off" on Sides.

The footer text comes from `PAGE_ITEMS` (one tuple of `"key description"` items
per page), `EDIT_ITEMS` while editing and `SEARCH_ITEMS` while searching.
`PAGE_KEYS`, `EDIT_KEYS` and `SEARCH_KEYS` are the same items joined with two
spaces into a single line, kept for callers and tests that want the help as one
string.

## Search

`/` starts a search, but only on the two list pages (`_searchable`). Search mode
swallows every key: the printable ones extend the query, Backspace shortens it,
the arrows and PgUp/PgDn/Home/End still move the cursor, and Enter or Esc leaves.
Nothing else — you cannot toggle a side or set a pad state while searching.

The matching is word based:

- `query_words(query)` splits on whitespace, lower-cases and drops duplicates
  while keeping the order, so `"u1 u1"` is the same one-word query as `"u1"`.
- `match_count(fields, query)` counts how many *distinct* words are a
  case-insensitive substring of *any* field. Words are OR-ed for the filter.
- `matches(fields, query)` is `match_count >= 1`; a blank query matches
  everything.
- `filter_counts(rows, fields_of, query)` keeps the rows with a count of at
  least 1 and sorts them by descending count. The sort is stable, so rows with
  the same count keep their original order and the best matches come first. A
  blank query returns every row in order with a count of 0.
- `full_matches(counts, query)` counts the rows that matched *every* word.
- `match_summary(shown, total, full, words)` builds the sub-header tail:
  `no match`, `(n of N)` for a one-word query, `(n of N, k full)` for more.

The fields a row exposes are `pad_fields` — label, reference, pin, project,
side, aperture function, shape, state, the position text, and a synthetic field
that is `"paste"` for a pad with a paste opening and `"paste closed"` once it
was closed — and `side_fields` — project name, side name, label, `"mirrored"`,
`"on"`/`"off"` and the board size text.

A row that matched every word is drawn bold: `_is_full(index)` compares the
row's count against the number of query words and `_match_attr` clears
`A_DIM` and sets `A_BOLD`, so bold wins over the dim a pasted pad or a disabled
side would otherwise get.

The cursor is preserved by identity, not by index. `restore_index(rows, item,
fallback)` walks the list looking for the very same object and falls back to a
clamped index when it is gone. `_start_search` remembers the focused row as
`search_anchor` before filtering; every keystroke re-filters with
`_apply_filter(current_row)`, so the cursor follows its row through the
re-ordering; `_end_search` drops the filter and puts the cursor back on the row
it was on — or on the anchor when nothing matched.

`search_char(key)` accepts only printable ASCII (32 to 126), so control keys and
everything curses reports as a `KEY_*` code (>= 256) never end up in the query.

## All-pads mode

`*` flips `show_all`. `visible_pads(pads, sides, show_all)` first keeps the pads
whose `side_key` belongs to an enabled side — with no sides at all nothing can
be filtered by side, so every pad passes — and then, unless `show_all`, keeps
only the candidates.

Pads that already have paste are listed dimmed and marked with `· `. They are
never `undefined`: `allowed_state` refuses that state for them, `next_state`
puts them straight to `open` if they somehow are in another state, and
`cycle_state` then toggles open ↔ ignore. For a candidate the cycle is
undefined → open → ignore → open …; undefined is only ever a starting point.
Setting a pasted pad to `ignore` is what closes its opening, and the status line
says so explicitly ("paste opening closed" / "paste opening kept").

`apply_component(pads, pad)` sets the cursor pad's state on every pad of the
same project, side and reference, skipping the ones that may not take it, and
returns how many changed. `find_undefined(pads, start, forward)` wraps around
the list and only stops on candidates, so `n`/`N` walk the pads that still need
a decision even in the all-pads view.

`_sync_visible(keep)` recomputes the visible list. It caches a signature of
`(show_all, the keys of the enabled sides)` and does nothing when that has not
changed, and it returns immediately while a search is active, because the cursor
then indexes the filtered list rather than `self.visible`.

## Footer wrapping

`wrap_items(items, width, max_lines=2)` lays the key help out greedily: items
are joined with two spaces and never split, as many as fit go on a line, the
rest start the next one. `wrap_text` does the same for a status message, split
on whitespace and joined with single spaces. Both call `_wrap`, which clips
every line with `ellipsis` and, when something had to be dropped, appends `…` to
the last line so the truncation is visible:

```python
wrap_items(["a bb", "c dd", "e ff"], 12, 2)      # ['a bb  c dd', 'e ff']
wrap_items(["aaaa", "bbbb", "cccc", "dddd"], 9, 2)  # ['aaaa', 'bbbb  …']
```

`FOOTER_LINES` is 2, so the help never takes more than two rows. When the
terminal is too short even for that, `_draw` re-wraps into the number of rows
that are actually left.

## Inline editing

`e` or Enter on a numeric Layout field calls `_start_edit`: the buffer is
prefilled with `fmt_mm(current value)` and `edit_fresh` is set, so the first
character typed replaces the whole prefill while Backspace keeps it and edits it
instead. `edit_buffer(buffer, key)` accepts digits, `.` and `-` and returns
`None` for anything else, so stray keys are ignored rather than appended.

Enter parses the buffer with `parse_number` (rejects anything that is not a
finite float) and checks it with `valid_value` (rejects NaN and the infinities,
then applies the field rule: `ge0` requires >= 0, `gt0` requires > 0, `any`
accepts everything); on success the value is rounded to 6 decimals and stored,
on failure the status line says `invalid value ... for <label>` and the buffer
stays open. Esc cancels. Space on a numeric field does not edit — it only prints
"press e or enter to edit".

`+` / `-` and the left/right arrows use `step_value`, which adds or subtracts
`field.step`, clamps to `field.minimum` when the field has one, rounds to 6
decimals and normalises `-0.0` to `0.0`. The result is still checked with
`valid_value` before it is stored.

## Layout, presets and the generate confirmation

`compute_layout` and `on_preview` are callbacks handed in by `cli._run`;
the TUI never imports the packer or the renderer itself.

- `_refresh_layout()` calls `compute_layout(None)` for the live configuration
  and catches any exception, setting `self.layout = None` and putting the error
  in the status line.
- `_refresh_presets()` packs once per size and orientation — with eight sizes
  and two orientations that is 16 packs — each on a `config.copy()` so the live
  configuration is untouched, and stores `True`, `False` or `None` when packing
  raised.
- `_invalidate()` is called after anything that can move a cell (toggling a
  side, picking a size, changing the orientation or a layout parameter): it
  re-packs the live layout and drops the preset cache, which the loop refills
  the next time the Stencil page is drawn.

Generating is guarded when the block does not fit. `_confirm(keyname, pending)`
returns `True` at once when the layout fits or is unknown; otherwise it stores
the key in `self.pending`, shows `NOFIT_WARNING` ("layout does not fit the
stencil — press again to generate anyway") and returns `None`, so the loop keeps
running. Pressing the same key again — Enter on the Pads page, `g` elsewhere —
then generates. `pending` is read and cleared at the top of every other key, so
any key in between cancels the confirmation.

`_preview` sets the status to "rendering…" and redraws *before* calling back,
because a full render takes about a second (see [render.md](render.md)), then
reports the path or the error.

## Terminal requirements

The module imports the standard library `curses` at module level, which is why
there is no Windows support without a curses port. Colours are optional:
`_init_colors` returns quietly when the terminal has none and `_state_attr` /
`_kind_attr` fall back to `A_DIM`, `A_BOLD` and `A_REVERSE`. `curses.wrapper`
restores the terminal on the way out, including after an exception.
`KEY_RESIZE` calls `update_lines_cols()` and `clear()` to force a full repaint.
`_put` clips every write to the window and swallows `curses.error` and
`UnicodeError`, so a narrow, short or otherwise awkward terminal degrades
instead of crashing the run. The UI uses a handful of non-ASCII glyphs (`→`,
`●`, `·`, `⌀`, `…`), so a UTF-8 locale is expected.

See [architecture.md](architecture.md) for where the TUI sits in the pipeline
and what the caller does with its return value.
