# Layout: cells, packing, dividers and the datum

`pcbstencil/layout.py` turns a list of enabled board sides into one `Layout`:
where every side sits on the stencil sheet, where its alignment features (the
*datum*) go, and which dotted lines mark the border between two boards. It is pure geometry —
no gerber objects are read here, only the bounding box of every project — and
it is deterministic: the same sides and the same `LayoutParams` always give the
same sheet. The public entry points are `pack(sides, config) -> Layout` and
`layout_report(layout, config) -> str`; everything else is module private.
Sheet coordinates have their origin at the bottom left corner of the stencil,
X to the right, Y up, in millimetres (see [`data-model.md`](data-model.md)).

## Cells

Every enabled side gets one rectangular **cell**: the board bounding box padded
by `gap/2` on each of its four sides, so two neighbouring cells that touch keep
their boards at least `gap` apart and the shared edge sits in the middle of that
gap. `LayoutParams.pad` is that `gap/2`. Cells are never rotated and never
stretched to fill space; the only two things that ever make a cell bigger than
`board + gap` are the `slots` datum's raster and the `holes` datum's pin grid.
`_cell_of(board_w, board_h, params, grid)` is the single place that decides a
cell size:

| `datum` | cell |
| --- | --- |
| `slots` | `board + 2 * max(gap/2, min_pad_for_slots)` on both axes, each rounded **up to a whole number of `slot_pitch`**, then grown until the longer axis carries two slots and the shorter one carries one (`_edge_for_slots`, the 2 + 1 of an exact location — 3 and 2 pitches with the default numbers). |
| `holes` | `board + gap`, then `_cell_size` grows each side until the hole-to-hole distance is a whole number of grid pitches. |
| `none` | exactly `board + gap`. |

```
 datum = slots (default)              · = divider dots (two lines per edge)
 ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·   [==] = obround slot,  x = jig pin
 ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·
 ·                                ·   the board bbox is centred in the cell;
[=]       +------------+          ·   padding = (cell - board) / 2, at least
[x]       |            |          ·   max(gap/2, min_pad_for_slots)
[=]       |   board    |          ·
 ·        |            |          ·   slot_offset  edge -> outer wall (0.5)
 ·        +------------+          ·   slot_inner   edge -> inner wall (5.0)
[=]                               ·   slot_web     inner wall -> board (>= 3)
[x]                               ·   slot_pitch   raster of the jig (30):
[=]  [==x==]        [==x==]       ·                cell edges, slot centres
 ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·                and pin centres ride on it
 ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·   the piece is pushed toward the
 ^     |<-- 30 -->|                   bottom-left datum corner until each
 datum corner (Area.datum_corner)     inner wall touches its pin


 datum = holes (legacy)
 ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·   o = dowel pin hole
 ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·
      o                     o         ho = hole_inset + hole_dia/2
 ·                                ·        (LayoutParams.hole_offset)
 ·        +------------+          ·
 ·        |   board    |          ·
 ·        +------------+          ·
 ·                                ·
      o                     o
 ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·
 ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·
 |<-ho->|                            holes sit ho inside both cell edges
```

### Growing a cell for the raster

Every jig pin of the whole sheet has to land on one common raster measured from
the sheet origin. `_grid_of(params)` is the pitch that actually applies:
`slot_pitch` under `slots`, `hole_grid` under `holes`, 0 under `none` (no pins
to align). `ho` below is `params.pin_offset`, the cell edge to pin centre
distance of whichever datum is active — `slot_inner - pin_dia/2` (3.5 mm) or
`hole_offset` (4.5 mm).

Both datums need the cell corner to sit at `k * pitch - ho`, which is the
packer's job (`_snap_up`, below). Beyond that:

* `slots`: the cell **size** is a whole number of pitches on both axes
  (`_ceil_pitch`), so a cell placed next to another one starts on the same
  raster again and neighbours keep touching exactly. The slots themselves then
  simply sit on every raster position that fits.
* `holes`: the two holes on one cell edge are `cell - 2*ho` apart, so that
  distance has to be a whole number of grid pitches — `_cell_size` grows the
  cell until it is. Neighbours then no longer touch; the slivers are free
  space.

`_ceil_pitch(value, pitch)` is the `slots` side of it, `steps = max(1,
ceil(value / pitch - _GRID_EPS))` times the pitch. `_grid_size` is the `holes`
side and grows one cell side `s` to the next admissible length:

```
steps = max(0, ceil((s - 2*ho) / grid - _GRID_EPS))
grown = steps * grid + 2*ho
```

and `_cell_size` takes `max(s, grown)` per axis, so a cell is only ever grown,
never shrunk. `_GRID_EPS` (1e-9, in pitches) keeps a side that is already an
exact multiple from being pushed up one whole pitch by floating point noise.
The board stays centred in the grown cell, so the padding grows past `gap/2` —
the report prints `padding ≥ 15.0 mm` instead of `padding 15.0 mm` whenever a
raster is on.

Worked arithmetic for the smallest board in the sample set, RBARF, with
`datum = slots` and otherwise default parameters (`gap` 30, `slot_pitch` 30,
`slot_offset` 0.5, `slot_width` 4.5, `slot_web` 3):

| step | value |
| --- | --- |
| board bbox | 13.405 x 11.610 mm |
| padding floor `max(gap/2, min_pad_for_slots)` | `max(15, 0.5 + 4.5 + 3)` = 15 mm |
| raw cell (`board + 2 * 15`) | 43.405 x 41.610 mm |
| rounded up to whole pitches | `ceil(43.405/30)` = 2, `ceil(41.610/30)` = 2 → 60 x 60 mm |
| 2 + 1 minimum (`_edge_for_slots`) | the longer axis needs 3 pitches → 90 x 60 mm |
| cell | 90.0 x 60.0 mm |
| slots | bottom edge 90 mm → 2, left edge 60 mm → 1 |
| padding kept by the board | (90 - 13.405)/2 = 38.3 mm in x, (60 - 11.610)/2 = 24.2 mm in y |

The same board with `datum = holes` and the legacy pin grid (`hole_dia` 5,
`hole_inset` 2, `hole_grid` 8):

| step | value |
| --- | --- |
| board bbox | 13.405 x 11.610 mm |
| raw cell (`board + gap`) | 43.405 x 41.610 mm |
| `ho = hole_inset + hole_dia/2` | 2 + 5/2 = 4.5 mm |
| width: `ceil((43.405 - 9) / 8)` | `ceil(4.30)` = 5 steps |
| width: `5 * 8 + 9` | 49.0 mm |
| height: `ceil((41.610 - 9) / 8)` | `ceil(4.08)` = 5 steps |
| height: `5 * 8 + 9` | 49.0 mm |
| cell | 49.0 x 49.0 mm |
| padding kept by the board | (49 - 13.405)/2 = 17.8 mm in x, (49 - 11.610)/2 = 18.7 mm in y |

With `hole_grid = 0` the holes cell is exactly 43.405 x 41.610 mm, and with
`datum = none` it always is. The `slots` datum is the expensive one for a small
board: rounding 43.4 x 41.6 mm up to 90 x 60 mm costs 1,494 mm2, but it is what
puts the cell corners, the slots and the pins of every cell on the same 30 mm
raster — 3,594 mm2 for this cell, 28,039 mm2 over the twelve generated cells.

## Ordering

`ordered_sides(sides, params)` drops the disabled sides and sorts the rest into
the order the packer receives them. The sort is stable and total, so the result
does not depend on discovery order.

| `params.sort` | key |
| --- | --- |
| `SORT_HEIGHT` (default) | `(-round(height, 6), -round(width, 6), natural_key(project name), top before bottom)` |
| `SORT_NAME` | `(natural_key(project name), top before bottom)` |

Heights and widths are rounded to 6 decimals before being negated, so two
boards that differ only in floating point noise are treated as equally tall and
fall through to the name. `natural_key` (from `pcbstencil/pads.py`) compares
embedded numbers numerically, so `board2` sorts before `board10`. The top side
of a project always comes immediately before its bottom side.

For the eight sample projects with `sort = height` the first five cells offered
to the packer are:

1. `LED lamp for gardening / top`
2. `alacrity badge / top`
3. `alacrity badge / bottom`
4. `Midea WiFi Dongle / top`
5. `Midea WiFi Dongle / bottom`

## The MaxRects packer

`_maxrects` is Jylanki's MaxRectsBinPack without rotation. The free space is a
list of *maximal* free rectangles, starting as the whole sheet
`(0, 0, sheet_w, sheet_h)` (nothing at all when either dimension is zero).
Cells are placed one at a time, in the order above:

1. Every free rectangle `(fx, fy, fw, fh)` is tried. Its bottom left corner is
   first snapped up with `_snap_up`, so `x + ho` and `y + ho` are multiples of
   the grid; the usable size shrinks to `aw = fw - (x - fx)`,
   `ah = fh - (y - fy)`.
2. The rectangle is skipped when `aw + _FIT_EPS < cw` or `ah + _FIT_EPS < ch`
   (`_FIT_EPS` = 1e-6 mm of slack).
3. The remaining candidates are scored at the snapped position, and the best
   score wins. The comparison key is `(score, x, y)`, so ties are broken by the
   placement itself: leftmost first, then lowest.
4. The chosen position is recorded and every free rectangle is split by
   `_split` into its up to four maximal remainders (left, right, bottom, top of
   the placed cell; a rectangle that does not overlap is returned unchanged).
   `_prune` then drops every rectangle another one already contains, compared
   with `_FIT_EPS` slack.
5. A cell for which no free rectangle qualifies gets `None` — it is *overflow*.

The three heuristics differ only in the score, computed from the snapped
position `(x, y)`, the usable size `(aw, ah)` and the cell size `(cw, ch)`:

| constant | name | score tuple |
| --- | --- | --- |
| `HEURISTIC_BL` | bottom-left | `(y + ch, x)` — lowest top edge, then leftmost |
| `HEURISTIC_BSSF` | best short side | `(min(dw, dh), max(dw, dh))` with `dw = aw - cw`, `dh = ah - ch` |
| `HEURISTIC_BAF` | best area | `(aw * ah - cw * ch, min(dw, dh))` |

`_best_packing` runs all three over the same cell list and keeps the winner by
`(unplaced, bw * bh, rank)`: fewest cells that did not fit, then the smallest
area spanned by the placed cells, then the earliest heuristic in `HEURISTICS`
order (bottom-left, best short side, best area). `bw` and `bh` are measured
from the sheet origin — `max(x + cw)` and `max(y + ch)` over the placed cells —
not as a true bounding box, which is the same thing whenever a cell touches the
origin side. The winning heuristic name ends up in `Layout.heuristic` and in
the report.

## Raster snapping, overflow and centring

`ho` is `params.pin_offset` throughout: `slot_inner - pin_dia/2` (3.5 mm by
default) for the `slots` datum, `hole_offset` (4.5 mm) for `holes`. The packer
does not care which datum produced it, and `grid` is whatever `_grid_of`
returned — `slot_pitch` or `hole_grid`.

`_snap_up(value, ho, grid)` returns the smallest cell corner `>= value` whose
pins land on the grid: it computes `steps = (value + ho) / grid` and takes
`round(steps)` when that is within `_ALIGN_EPS` of the exact value (a position
that is already admissible is returned as the exact grid position, not pushed
up a whole pitch), otherwise `ceil(steps)`; the result is `steps * grid - ho`.
`_snap_down(value, grid)` is the mirror image for a distance rather than a
corner: `round` when already admissible, else `floor`, clamped at 0.
Both return `value` unchanged when `grid <= 0`.

**Overflow.** Cells the packer could not place are parked on a shelf to the
right of the sheet so the user can see them: the first one goes to
`(_snap_up(sheet_w), _snap_up(0.0))` and each following one starts at
`_snap_up(previous x + previous width)`. They keep their grid alignment, they
are translated together with the rest of the block, and `_block_of` includes
them — which is exactly why `Layout.block` can be wider than the sheet and why
`fits` is `False`.

**Centring.** The block of *placed* cells is centred on the stencil. The offset
is `_snap_down(max(0, (sheet - block) / 2), grid)` on each axis, so it is a
whole number of pitches and every pin stays on the raster measured from the
sheet origin. With no placed cell at all the offset is zero. The block is
therefore centred only to within one pitch — under `slots` it also starts at
`pitch - pin_offset` (26.5 mm) at the earliest, because that is the first
admissible corner on the sheet.

**Touching cells.** Under `slots` every cell size is a whole number of pitches
and every corner sits on the same lattice, so the corner of a free rectangle
left by a placed cell is already admissible and `_snap_up` returns it
unchanged: neighbours touch exactly, their two dotted lines are `dot_line_gap`
apart, and no sliver is ever left between them. Under `holes` only the corners
are constrained, so the slivers stay.

**`_aligned`.** Floating point addition is not associative: `(start + offset) +
size` can differ from `(start + size) + offset` by one ULP, and two cells that
touch exactly in the bin would then overlap by ~1e-13 mm on the sheet — enough
to turn one shared divider into two lines a hair apart. `_aligned` therefore
translates the coordinates of one axis in sorted order and snaps every new
coordinate onto an already translated far edge when the two are within
`_ALIGN_EPS` (1e-9 mm), remembering each `value + size` as it goes.

After that, `pack` builds one `Area` per side: the cell rectangle, the board
rectangle (`board_rect`, the board bbox centred in the cell), the board → sheet
`Transform` (`dx = bx - (-maxx if mirror else minx)`, `dy = by - miny`), the
datum (`holes`, `slots`, `pins` and `datum_corner`, from `_datum()`), `row`
(the packing order index, informational only) and the `overflow` flag.

## Dividers

`_dividers(areas, outer, line_gap)` returns the dotted **lines**, not the cell
edges — `Layout.dividers` holds `(x0, y0, x1, y1)` tuples that the preview draws
as faint dashed guides and `_dots` walks to place the openings.

The rule is one line per cell edge, owned by that cell and running *inside* it:
two cells that touch therefore show two lines `line_gap` apart and the scissors
cut between them, while an edge facing free space keeps its one line inside the
cell and the cut goes anywhere outside it. No line is ever drawn outside a cell.

1. **Inset.** Every cell rectangle is pulled in by `line_gap/2` on all four
   sides: a left edge at `x` gives a line at `x + line_gap/2`, a right edge at
   `x` one at `x - line_gap/2`, and likewise for bottom and top. The extent of
   a line is the extent of its edge shortened by the same `line_gap/2` at both
   ends, so the four lines of a cell meet at their corners instead of sticking
   out — together they are exactly the cell rectangle inset by `line_gap/2`.
   `line_gap <= 0` leaves every line on the edge itself, so two touching cells
   share one. A cell thinner than `line_gap` cannot hold the inset; it is then
   clamped to half the cell in that direction, which collapses the two opposite
   lines onto the cell's centre line and drops the two perpendicular ones (no
   length left). A line with extent `<= _MERGE_EPS` is skipped.
2. **Drop the block boundary.** Unless `outer_border` is set, the line of an
   edge lying on the block's minimum or maximum on its axis is dropped: nothing
   has to be cut apart on the outer boundary of the block. The test runs on the
   cell **edge**, not on the position of the line it carries. The boundary comes
   from `_block_of(areas)`, overflow cells included.
3. **Collect.** The surviving lines go into a dict keyed by
   `(orientation, coordinate)`, the coordinate passed through a `_Snap` with
   `_MERGE_EPS` (1e-6 mm) so that two lines that are the same line to within a
   rounding error share one key.
4. **Merge.** Per line, the collected intervals are merged by `_merge`:
   overlapping *and* touching intervals become one, so two cells stacked along
   one line with the same inset give a single straight divider with evenly
   spaced dots.

## Dots

`_dots(dividers, params, cover)` places the openings along each divider
segment:

```
n     = floor(length / dot_pitch)        (0 when the pitch is not positive)
start = (length - n * dot_pitch) / 2     (half the leftover, so the run is centred)
dots  = start + i * dot_pitch,  i = 0 .. n
```

A segment shorter than one pitch gets `n = 0` and therefore one dot in its
middle; a zero-length segment gets one dot at its position. The result is then
deduplicated with `_Dedupe(params.dot_dia)`: a dot closer than one dot diameter
to a dot already kept is dropped, which is what keeps the corners where two
dividers cross from stacking openings on top of each other. `_Dedupe` is a
spatial hash — points are bucketed by `floor(coord / min_dist)` and only the
3x3 neighbouring buckets are searched.

The dots are not a drawing aid: they are flashed into the paste layer as real
round openings of `dot_dia`, so the sheet can be snapped apart along them.

## The datum

`_datum(x, y, w, h, params, grid, seen)` produces the alignment features of one
cell and returns `(holes, slots, pins)` in sheet coordinates. `pins` is filled
for *both* datums — it is what the grid check and the preview's ghost circles
use — and `Area.datum_corner` is always the cell's bottom left corner `(x, y)`.

### `slots`

One obround slot at **every** raster position that fits along the bottom edge
and along the left edge — never the top or the right one — so a modular jig
with pins on that raster can use whichever of them suit it:

```
across = slot_offset + slot_width/2              # cell edge -> slot centre line
pin    = slot_inner - pin_dia/2                  # == params.pin_offset

for off in _slot_offsets(w, params):             # the bottom edge, left to right
    slots.append((x + off, y + across, slot_length, slot_width))
    pins.append((x + off, y + pin))
for off in _slot_offsets(h, params):             # the left edge, bottom to top
    slots.append((x + across, y + off, slot_width, slot_length))
    pins.append((x + pin, y + off))
```

`_slot_offsets(size, params)` is where the raster lives. The cell corner sits at
`k * slot_pitch - pin_offset` (the packer put it there), so seen from the corner
the raster positions are `pin_offset + k * slot_pitch`; the ones that keep the
slot inside the span `_slot_span` returns — `[slot_offset + length/2,
size - slot_offset - length/2]`, which holds its round ends `slot_offset` clear
of the two cell edges it runs into — are returned, in order. With the default
numbers that is `3.5 + 30k` for `k = 1, 2, ...`, so an edge of `n` pitches
carries `n - 1` slots: 60 mm one, 90 mm two, 120 mm three. `_edge_for_slots`
walks the same function when `_cell_of` sizes a cell, so the two can never
disagree.

Every slot centre and every pin centre is therefore an exact multiple of
`slot_pitch` in both axes, measured from the sheet origin. Each pin sits tangent
to the slot wall nearest the board — a bottom pin touches its slot from below, a
left pin from the left — because the piece is pushed toward the bottom-left
`datum_corner`. Two contacts on one edge fix that axis and the rotation, one on
the other edge fixes the second axis: exact constraint, no over-determination,
and the extra slots only add choices for the jig, never contacts.

A slot's outer wall is `slot_offset` (0.5 mm) inside the cell edge, so it lies
inside its own cell but **crosses that cell's own dotted line**, which runs
`dot_line_gap/2` (1.25 mm) in: the dots that fall inside it are dropped (see
*Dots inside a datum opening*). It never reaches the neighbouring cell, whose
own line is another `dot_line_gap` away. The inner wall is `slot_inner` inside
the edge and the cell always has at least `min_pad_for_slots = slot_inner +
slot_web` of padding (`_cell_of` saw to it), so the board keeps its `slot_web`
of foil and the squeegee keeps a clear lane beside it.

### `holes`

`_holes(x, y, w, h, params, seen)` returns the four dowel hole centres of one
cell, at `hole_offset = hole_inset + hole_dia/2` inside both cell edges it is
near, in the order bottom-left, bottom-right, top-right, top-left. The pins
*are* the holes.

`hole_inset` may be negative: at `hole_inset = -hole_dia/2` the hole centre sits
exactly on the cell edge, and two cells sharing that edge would put a hole in
the same place. A single `_Dedupe(_HOLE_EPS)` (1e-6 mm) is threaded through all
cells of the sheet, so the second cell simply does not get that hole — the
report prints `dowel holes: none (shared with a neighbouring cell)` when a cell
ends up with none at all.

### `none`

Three empty lists. `_grid_of` also returns 0, so no snapping happens anywhere
and every cell is exactly `board + gap`, whatever `hole_grid` says.

### Dots inside a datum opening

`_dots` is handed a `_Cover(areas, params)` and drops any dot whose centre would
fall inside a slot or a dowel hole — a dot there would be cut out of the foil
twice and weaken the wall the pin touches. `_Cover` stores every opening as a
segment plus a radius (a slot is a stadium: its centre segment grown by
`min(w, h)/2`; a hole is a point grown by `hole_dia/2`) and buckets them on a
raster at least as coarse as the largest opening, so `covers(x, y)` only tests
the shapes in one bucket. `_in_obround` is the point-in-stadium test.

## `fits`

```
fits = (not overflow
        and block.minx >= -_FIT_EPS and block.miny >= -_FIT_EPS
        and block.maxx <= sheet_w + _FIT_EPS and block.maxy <= sheet_h + _FIT_EPS)
```

The CLI prints `fits` / `DOES NOT FIT` after the first pack, warns again before
generating, and writes the gerbers regardless. The TUI shows `FITS` /
`DOES NOT FIT` in its status line and requires the generate key to be pressed
twice when the layout does not fit (see [`tui.md`](tui.md)). With every side
switched off the layout has no areas at all, the block is `(0, 0, 0, 0)` and
`fits` is `True`.

## `layout_report`

`layout_report(layout, config)` renders the text that becomes the first half of
`<name>-report.txt`. Its head is five lines:

| line | content |
| --- | --- |
| `Stencil:` | sheet size in mm, the preset label and the orientation |
| `Block:` | block size and its corners, `FITS` or `DOES NOT FIT` |
| `Cells:` | placed count, winning heuristic, overflow count, gap, padding (`≥` when the cells were grown), sort order |
| `Datum:` | the mode and its numbers — for `slots` three or four lines (the raster, the two wall offsets, the pin diameter and the push direction; the slot size and the bottom+left counts that occur; the sizing rule and the web; and, when the slots reach the dotted line, the line saying so), for `holes` one (diameter, inset and the resulting centre offset), for `none` just `none` |
| `Grid:` | the pitch and where it comes from (`slot_pitch` or `hole_grid`), whether every *pin* is on it and how many there are, plus a line with the cell area it cost — or `off` |

Under `slots` the `Datum:` block explains that the slots clip the cell's own
dotted line and stop `slot_offset` short of the neighbour whenever
`slot_offset` is smaller than the dotted band (`dot_line_gap/2 + dot_dia/2`,
how far inside the cell edge the dots reach). That is the normal case with the
default 0.5 mm offset and it is not a warning: the cut is made between the two
lines of a shared edge, and the slot never reaches the other one. A `hole_grid`
set under the slots datum gets one line saying it applies to the holes datum
only.

`_pins_on_grid` re-checks every *pin* centre of every area against the raster
from the sheet origin with `_FIT_EPS` tolerance, so the report verifies the
invariant rather than asserting it. `_grid_waste` sums
`area.w * area.h - (board.width + gap) * (board.height + gap)` over all areas:
the square millimetres the grid cost against plain `board + gap`.

Then one block per area: the label (with `(mirrored)`), the cell rectangle, the
board rectangle and its size, then the datum. Under `slots` that is the datum
corner, the slot count per edge (`4 on the bottom edge + 3 on the left edge`),
every slot as `(cx, cy) w x h`, every jig pin centre and a
`nesting:` line giving the push direction in sheet *and* board coordinates (a
mirrored side is pushed toward the board's physical bottom-right). Under
`holes` it is the four hole centres and the same four as pins. Every coordinate
is printed twice, on the sheet and relative to the bottom left corner of the
board — the second is the number a fixture plate is drilled from. A pin that
could not be put on the raster is marked `OFF THE RASTER (the cell is too small
for it)`. After the datum come the paste opening count, the candidate pads by state
and the closed opening count. The last line counts the divider dots, their
diameter and pitch, the number of dotted lines, how far inside its edge each of
them runs, and whether the block boundary is included.

## Worked example

Eight KiCad gerber zips in one folder, `--batch --no-open`, everything left at
the defaults: 380x280 landscape, `gap` 30, `datum = slots` (4.5 x 12 mm slots,
0.5 mm offset, 30 mm pitch, 3 mm web, 3 mm pins), dots 0.5 mm at 3 mm pitch,
`dot_line_gap` 2.5, `hole_grid` 8 (holes datum only), `outer_border` off,
`sort = height`.

The first pack sees all 13 relevant sides and the block measures
**780.00 x 240.00 mm** with 4 cells in overflow — the raster is not free. Before
generating, the CLI drops every enabled side without a single opening:
`PhotoAmp bottom` has no paste layer and nothing was opened on it, so it goes,
and the remaining 12 sides are packed again, to **660.00 x 240.00 mm at
(26.50, 26.50)..(686.50, 266.50)**, bottom-left heuristic, 9 placed and 3 in
overflow. Dropping one side changes both the block and sometimes the winning
heuristic, which is worth remembering when comparing the first preview with the
generated one.

| cell | side | x | y | w | h | slots (bottom + left) |
| --- | --- | ---: | ---: | ---: | ---: | --- |
| 1 | LED lamp for gardening top | 26.50 | 26.50 | 150.00 | 120.00 | 4 + 3 |
| 2 | alacrity badge top | 176.50 | 26.50 | 120.00 | 90.00 | 3 + 2 |
| 3 | alacrity badge bottom (mirrored) | 176.50 | 116.50 | 120.00 | 90.00 | 3 + 2 |
| 4 | Midea WiFi Dongle top | 296.50 | 26.50 | 60.00 | 90.00 | 1 + 2 |
| 5 | Midea WiFi Dongle bottom (mirrored) | 296.50 | 116.50 | 60.00 | 90.00 | 1 + 2 |
| 6 | indxworks top | 26.50 | 146.50 | 90.00 | 60.00 | 2 + 1 |
| 7 | indxworks bottom (mirrored) | 26.50 | 206.50 | 90.00 | 60.00 | 2 + 1 |
| 8 | airbox top | 116.50 | 206.50 | 90.00 | 60.00 | 2 + 1 |
| 9 | PhotoAmp top | 206.50 | 206.50 | 120.00 | 60.00 | 3 + 1 |
| 10 | KliFan bottom (mirrored), overflow | 386.50 | 26.50 | 120.00 | 60.00 | 3 + 1 |
| 11 | RBARF top, overflow | 506.50 | 26.50 | 90.00 | 60.00 | 2 + 1 |
| 12 | RBARF bottom (mirrored), overflow | 596.50 | 26.50 | 90.00 | 60.00 | 2 + 1 |

Every cell is a whole number of 30 mm steps, every corner sits at
`k * 30 - 3.5` mm and every cell touches its neighbours. The report says 9
cells placed with MaxRects (bottom-left), 3 overflow, all 46 pins (one per
slot) on the 30 mm `slot_pitch` raster, 28,038.9 mm2 of cell area spent on it,
and 861 divider dots on 35 dotted lines — one line per cell edge, 1.25 mm
inside it, a few of them merged where two cells are stacked along the same
line. 136 dots of the raw dot grid fall inside a slot and are dropped: with a
0.5 mm offset a slot always crosses the line of its own cell, which is the
point (the foil beside the board stays free), and never the neighbour's.

The same set with the other two datums, everything else unchanged:

| `datum` | block after dropping `PhotoAmp bottom` | cells | raster |
| --- | --- | --- | --- |
| `slots` | 660.00 x 240.00 mm, 3 overflow | whole multiples of 30 mm, grown by 28,038.9 mm2 | 46 pins on the 30 mm `slot_pitch` |
| `holes` | 353.00 x 273.00 mm, fits | grown by 9,090.9 mm2 in total | 48 pins on the 8 mm `hole_grid` |
| `none` | 361.35 x 194.30 mm, fits | exactly `board + gap` | off (no pins to align) |

`none` is the floor. `holes` is padded out to the next 8 mm multiple, RBARF's
43.405 x 41.610 mm becoming 49 x 49 mm. `slots` is by far the most expensive
here: the same RBARF cell becomes 90 x 60 mm, because a 30 mm raster cannot
carry two slots on one edge in less than three steps. Thirteen small boards on
a 380 x 280 sheet are the worst case for it — a coarse raster pays off on
fewer, larger boards, or on a larger sheet (all 13 sides fit on 600 x 600, block
540 x 210 mm), and `--slot-pitch 20` brings the default sheet within reach
again (600 x 260 mm, 3 overflow).

## Performance

`pack` is cheap enough to run on every keystroke. Measured on the eight sample
projects (13 sides, mean of 50 calls):

| configuration | time per `pack` |
| --- | --- |
| `datum = slots`, `slot_pitch = 30` | ~1.4 ms |
| `datum = holes`, `hole_grid = 8` | ~1.9 ms |
| `datum = none` | ~1.2 ms |

The `slots` datum adds one slot per raster position and a `_Cover` lookup per
dot on top of the packing; `_slot_offsets` is a handful of arithmetic per cell
edge, so it stays in the same range as the grid-free case.


The TUI re-packs whenever a side is toggled, a sheet size is picked or a layout
parameter changes, and the Stencil page packs once per size and orientation to
fill its fits column — currently 8 sizes x 2 orientations = 16 packs, still
well under a frame. Rendering the preview afterwards costs about a second and
dominates everything the layout does; see [`render.md`](render.md).

## See also

* [`architecture.md`](architecture.md) — where `pack` sits in the pipeline.
* [`data-model.md`](data-model.md) — `LayoutParams`, `Area` and `Layout` field
  by field, and the three coordinate systems.
* [`render.md`](render.md) — how the cells, dividers, dots and the datum are
  drawn.
* [`development.md`](development.md) — adding a layout parameter end to end.
* [`kinematic-alignment.md`](kinematic-alignment.md) — the design study the
  `slots` datum comes from.
