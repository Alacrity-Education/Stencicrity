# The TUI

`src/tui.rs` is the ratatui front-end that runs between the first preview and
the generation step. It is the largest module in the crate, and almost all of
it is pure: everything above the `App` struct is plain functions over plain
data, unit tested in place and driven from outside the crate by
`tests/tui_api.rs` and `tests/tui_split.rs`.

```
run_tui(&mut projects, &sides, &mut config, hooks, title) -> Result<bool>
```

true when the user pressed `w`, false on `q`. It mutates `pad.state`,
`side.enabled`, `config.size`, `config.orientation` and `config.layout.*` in
place and never touches `config.sides` / `config.pads` — the caller collects
those from the objects afterwards. With neither pads nor sides it returns
`Ok(true)` without touching the terminal at all.

## Structure

| Layer | What it is |
| --- | --- |
| pure helpers | state cycling, row formatting, field stepping and validation, search matching and ranking, footer wrapping, the split plan, the fit line |
| `App` | all the state, `draw()` and `handle_key()` — and nothing else |
| `run_tui` / `event_loop` | the terminal, the event loop, the `TerminalGuard` |

`TuiHooks` is how the TUI reaches back into the pipeline without depending on
it:

| Callback | Signature | Used for |
| --- | --- | --- |
| `compute_layout` | `&mut dyn FnMut(&[Project], &Config) -> Layout` | re-pack after every change, and 16 times for the Stencil page's fit table |
| `on_preview` | `&mut dyn FnMut(&[Project], &Config, Option<PadId>, bool) -> String` | render the PNG (and open it); returns the path |

`event_loop` calls `before_draw()`, draws, then `run_preview()` — so the
`rendering…` frame is on screen before the blocking render starts — and then
blocks on one key event. A resize needs no handling: `Terminal::draw` resizes
and the next frame reads the new size.

`TerminalGuard::drop` calls `ratatui::restore()`, so the terminal comes back
whatever happens, a panic included.

## The four pages

Switched with `1`–`4`, `Tab` and `Shift-Tab`. The session starts on **Pads**
when there is anything to decide, on **Sides** otherwise.

The screen is: a tab bar with the title, a sub-header, the body, the always
visible fit line, and one or two footer lines. On a short terminal they are
given up in order: the sub-header goes below 7 rows, the fit line below 5, the
tab bar below 3, and the key help is re-wrapped into whatever lines are left.

### 1. Pads

One row per candidate pad of the **enabled** sides:

```
  state       pad          project              side    function      shape            position
→ ? undefined  U3.14       indxworks            bottom  SMDPad        R 0.28x0.52      x= 148.082 y= -99.568
  - ignore   · TP1.1       RBARF                top     SMDPad        C ⌀1.00          x=  12.500 y=   4.250
```

The marker in front of the state is `+` for open, `-` for ignore and `?` for
undefined, and the state itself is coloured. A pad that already has paste is
dimmed and marked with a `·` in front of its reference, so a terminal without
colour can still tell it apart.

The sub-header says `pads <n>/<total>`, whether the list is "pads to decide" or
"all pads", the `*` hint, and how many pads are hidden on disabled sides.

### 2. Sides

```
  [x] alacrity badge · top          85.7 x    54.1 mm   paste 46     pads to decide 4      undefined 0
  [ ] LED lamp · top               100.1 x    78.1 mm   paste 15     pads to decide 0      undefined 0
```

Switching a side off removes its cell from the sheet *and* its pads from the
Pads page; `sync_visible` follows the cursor by pad identity into whatever
list results, and re-applies an active filter to it.

### 3. Stencil

One row per preset, with the verdict for both orientations:

```
  ● 420 x 320 mm   landscape: 420 x 320  fits  portrait: 320 x 420  NO            ← landscape
```

The two columns come from `refresh_presets`, which packs the whole sheet 16
times (8 sizes x 2 orientations) with a cloned `Config`. That is a few
milliseconds, and it is recomputed lazily: `presets` is set to `None` by every
change and refilled by `before_draw` when this page is in front.

### 4. Layout

`LAYOUT_FIELDS` is a `static [Field; 19]` and it is the single source of the
page, the stepping, the inline editor and the validation:

| Field | Label | Kind | Step | Minimum | Rule |
| --- | --- | --- | --- | --- | --- |
| `Gap` | spacing (gap between boards) | length | 0.5 | 0 | ≥ 0 |
| `Datum` | datum (alignment features) | choice | | | slots / holes / none |
| `HoleDia` | hole diameter | length | 0.5 | 0.1 | > 0 |
| `HoleInset` | hole inset (dotted line to hole edge) | length | 0.5 | — | any |
| `SlotWidth` | slot width | length | 0.5 | 0.1 | > 0 |
| `SlotLength` | slot length | length | 0.5 | 0.1 | > 0 |
| `SlotOffset` | slot offset (edge to outer wall) | length | 0.5 | 0 | ≥ 0 |
| `SlotPitch` | slot pitch (modular jig raster) | length | 5.0 | 1.0 | > 0 |
| `SlotWeb` | slot web (to board) | length | 0.5 | 0.1 | > 0 |
| `PinDia` | pin diameter | length | 0.5 | 0.1 | > 0 |
| `Marker` | orientation marker | bool | | | |
| `MarkerSize` | marker size | length | 0.5 | 0.5 | > 0 |
| `DotDia` | dot diameter | length | 0.5 | 0.1 | > 0 |
| `DotPitch` | dot pitch | length | 0.5 | 0.1 | > 0 |
| `DotLineGap` | dotted line gap (between touching cells) | length | 0.5 | 0 | ≥ 0 |
| `DotClearance` | dot clearance (to slot/hole/marker) | length | 0.1 | 0 | ≥ 0 |
| `HoleGrid` | hole grid (holes datum, 0 = off) | length | 1.0 | 0 | ≥ 0 |
| `OuterBorder` | outer border | bool | | | |
| `Sort` | sort | choice | | | height / name |

`datum_row(attr)` marks the rows that only mean something under one datum —
the two hole rows under `holes`, the six slot rows plus the two marker rows
under `slots`. `field_applies` is false for those under any other datum and
they are drawn dimmed, but they stay fully editable, so a value can be set
before switching over.

### The status line and the fit line

The fit line sits above the footer and is always there:

```
stencil 420x320 landscape · block 380.0 x 220.0 mm · FITS · pads: undefined 10  open 0  ignore 38  closed 0
```

`FITS` is green, `DOES NOT FIT` red. The footer shows the key help of the
current page, or the last status message when there is one; `wrap_items` lays
the help items out over at most `FOOTER_LINES` (2) lines and marks a truncation
with `…`.

## Keys

`handle_key` dispatches in this order: an open inline edit swallows everything,
then an open search does, then Ctrl-C quits and any other Ctrl-chord is
ignored, then Esc, then the global keys, then the page handler.

| Key | Everywhere |
| --- | --- |
| `1`–`4`, `Tab`, `Shift-Tab` | change page |
| `↑`/`↓`, `k`/`j` | move one row |
| `PgUp`/`PgDn` | move a screenful |
| `Home`/`g`, `End`/`G` | first / last row |
| `p` | render the preview PNG **and** open it in the image viewer |
| `v` | toggle the split view |
| `w` | generate; twice when the layout does not fit |
| `q`, `Ctrl-C` | quit without generating |
| `Esc` | see below — never quits |
| `/` | start a search (Pads and Sides only) |

| Key | Pads | Sides | Stencil | Layout |
| --- | --- | --- | --- | --- |
| `space` | cycle the state | toggle the side | pick the size | flip a bool, cycle a choice (a number says "press e") |
| `Enter` | — | toggle the side | pick the size | edit the value |
| `o` | set `open` | — | toggle the orientation | — |
| `i` | set `ignore` | — | — | — |
| `a` | apply this pad's state to the whole component | — | — | — |
| `n` / `N` | next / previous undefined candidate | all off (`N`) | — | — |
| `*` | show all pads | — | — | — |
| `A` | — | all on | — | — |
| `+`/`=`/`→`, `-`/`_`/`←` | — | — | — | step by the field's step, or cycle a choice forward / backward |
| `e` | — | — | — | inline edit |

`cycle_state` is `open → ignore → open`; `undefined` is only ever a starting
point, so `next_state` sends an untouched candidate to `open` first. A pad
that already has paste can only be `open` or `ignore` (`allowed_state`), so
`a` skips the members of a component that may not take the target state.

`escape_action(editing, searching, pending, filtered)` is Esc's whole
behaviour, in priority order: cancel the inline edit, else leave the search
(dropping its filter), else cancel a pending `w`, else clear the filter a
finished search left, else do nothing at all.

## Search

`/` opens a vim-like incremental search on the Pads and Sides pages. Every
printable ASCII character goes into the query (`search_char` rejects control
keys and anything with Ctrl or Alt), Backspace removes one, and the arrows,
PgUp/PgDn and Home/End still move the cursor through the filtered list.

Matching is per word:

* `query_words` lower-cases the query, splits on whitespace and drops
  duplicates, so `"u1 u1"` is the one-word query `"u1"`;
* `match_count(fields, query)` counts how many of those words are a
  case-insensitive substring of **any** field;
* a row is kept when the count is at least 1 — the words are OR-ed;
* `filter_counts` sorts by the count, descending, with a stable sort, so the
  rows that matched the most words come first and ties keep their original
  order;
* a row that matched **every** word is a *full* match and is drawn bold.

The fields a row answers to:

| Page | Fields |
| --- | --- |
| Pads | `REF.pin`, reference, pin, project, side, function, shape, state, the position text, plus `paste` for a pad that has one and `paste closed` once its opening was removed |
| Sides | project, side, label, `mirrored`, `on`/`off`, the board size text |

The sub-header becomes `filter: <query>  (<n> of <N>, <k> full)`; the
`, k full` part is left out for a one-word query, where every match is full
anyway, and `no match` replaces the whole tail when nothing survived.

### The filter the search leaves behind

Enter leaves the search but **keeps** the filter: the list stays filtered and
ranked and every key works on the rows that are left. Esc leaves and clears it
at once, and clears a filter later on the way vim's `:noh` drops a highlight.
Either way the cursor stays on the row it was on — `restore_index` looks the
anchor row up by identity in the new list and falls back to a clamped index
when it is gone.

Each page keeps its own filter (`filters: [Option<String>; 4]`), and `/`
always starts a fresh, empty search, so typing over an active filter replaces
it rather than narrowing it. `esc clear filter` is inserted into the footer
help right behind `/ search` while one is active.

## All-pads mode

`*` switches `show_all`. `visible_pads(projects, pads, sides, show_all)` is
the base list: every pad of the **enabled** sides (with no sides at all,
every pad), and without `show_all`, only the candidates.

In all-pads mode the pasted pads are listed dimmed and can be set to `ignore`,
which removes their paste openings from the stencil. They are never
`undefined`, and `n` / `N` still walk only the candidates.

## The split view

`v` divides the terminal with a `│` (`SEPARATOR`). The app is drawn into a
`Buffer` of its own width and blitted onto the left half, so no page's drawing
code knows about the split.

`split_plan(width) -> SplitPlan`:

| Width | `left` | Right half |
| --- | --- | --- |
| ≥ `SPLIT_MIN_WIDTH` (150) | `SPLIT_LEFT_WIDTH` (90) | the picture, `width - 91` columns |
| enough for `SPLIT_LEFT_MIN` (60) + 1 + the prompt | `width - 1 - len(prompt)` | exactly the prompt |
| anything narrower | 0 | the prompt, full width |

The prompt is `SPLIT_PROMPT`, verbatim:

```
Terminal preview requires a larger terminal, please resize.
```

`panel_rows(height)` gives the picture the full height minus one row for the
caption, minus one more for the legend when the half is at least
`LEGEND_MIN_HEIGHT` (5) rows tall.

The picture itself comes from [`ascii.rs`](../src/ascii.rs): a `Raster` of
square sub-pixels, two stacked per character cell and drawn with `▀`, `▄` and
`█`, so one millimetre is the same number of screen pixels across and down.
Classes are painted in the PNG's own order (copper, pads, outline, ignored,
openings, undefined, selected), so a later class covers an earlier one, and
`class_color` returns the very same RGB values the PNG uses.

The pad it draws is the one the **Pads page** cursor is on, whatever page is
in front (`panel_pad`). The zoom (`zoom_step`) gives the pad's larger dimension
`ZOOM_SPAN` (0.25) of the panel width, clamped to at least `MIN_SPAN_COLS` (3)
columns and at most the whole panel, and the window is centred on the pad. The
caption above it reads

```
U3.14 · indxworks bottom · window 24.00 x 22.00 mm · 1 col = 0.20 mm
```

When there is nothing to draw the right half says `no pad selected`
(`NO_PAD`) or `<pad> is not placed on the sheet` — the latter when the pad's
side is switched off or the layout is empty.

## Caches

Two, because the two expensive things must not run per frame:

| Cache | Keyed by | Invalidated by |
| --- | --- | --- |
| `geom_cache: GeomCache` | `SideId` | never — the objects do not change, only their classes do, and those are read from the live `Pad` |
| `panel_cache` | `(pad, cols, rows, generation)` | `touch()`, which bumps `generation` on every change the right half has to follow |

`invalidate()` = `refresh_layout()` + drop the presets + `touch()`, and every
mutation calls it. So moving the cursor, changing a pad state, a side switch,
a sheet size or a layout number all redraw the panel; a repaint after a resize
that changed nothing else reuses it.

The `GeomCache` entry for a side holds a `Shape` per copper object, per paste
object and per pad — an `IntervalTreeMultiPolygon` plus its bounds — so
sampling a sub-pixel is a point-in-polygon lookup rather than a walk over
every edge. Nothing is ever transformed: the sample point is mapped *back* to
board coordinates through the area's `Transform`.

## Inline editing

`e` or Enter on a numeric Layout row starts an edit prefilled with the current
value. The first typed character replaces that prefill (`edit_fresh`), after
which `edit_buffer` accepts digits, `.` and `-`, and Backspace. Enter parses
the buffer with `parse_number`, checks it against the field's `Rule` and
rounds it to 1e-6; an invalid buffer leaves a message and the edit open. Esc
cancels the edit and nothing else.

`+` / `-` use `step_value`, which adds the field's step, clamps to the field's
`minimum` and rounds to 1e-6 (and normalises `-0`). A step that would leave
the valid range is refused with a message instead of being clamped silently.
On a non-numeric row `+` cycles the choice forward and `-` backward.

## Generate and the confirmation

`w` returns `Action::Generate` straight away when the layout fits. When it
does not, the first `w` only sets `pending = Some('w')` and puts
`NOFIT_WARNING` in the status line; a second `w` generates, and any other key
clears the pending state (the dispatcher takes it at the top of every
`handle_key`, and Esc reports `generate cancelled`).

`Action` is the whole contract with the event loop: `Continue`, `Generate`,
`Quit`.
