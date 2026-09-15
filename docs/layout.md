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
`board + gap` are the room the `slots` datum needs and the `holes` datum's pin
grid. `_cell_of(board_w, board_h, params, grid)` is the single place that
decides a cell size:

| `datum` | cell |
| --- | --- |
| `slots` | at least `board + 2 * min_pad_for_slots` on both axes, at least `2 * slot_corner + slot_length` wide (the two bottom slots side by side) and at least `slot_length + 2 * slot_offset` tall (the left slot). The pin grid never grows it. |
| `holes` | `board + gap`, then `_cell_size` grows each side until the hole-to-hole distance is a whole number of grid pitches. |
| `none` | exactly `board + gap`. |

```
 datum = slots (default)              · = divider dots (two lines per edge)
 ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·   [==] = obround slot,  x = jig pin
 ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·
 ·                                ·   the board bbox is centred in the cell;
 ·        +------------+          ·   padding = (cell - board) / 2, which is
[=]       |            |          ·   gap/2 unless the cell was grown
[x]       |   board    |          ·
[=]       |            |          ·   slot_offset  edge -> outer wall (2.25)
 ·        +------------+          ·   slot_inner   edge -> inner wall (6.75)
 ·                                ·   slot_web     inner wall -> board (>= 3)
 ·   [==x==]        [==x==]       ·   slot_corner  corner -> slot centre (8)
 ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·
 ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·   the piece is pushed toward the
 ^                                    bottom-left datum corner until each
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

### Growing a cell for the hole grid (`holes` datum)

With `hole_grid > 0` every jig pin on the whole sheet has to land on one common
raster of that pitch, measured from the sheet origin. `_grid_of(params)` is the
pitch that actually applies — `hole_grid`, or 0 when the datum is `none`,
because then there are no pins to align. `ho` below is `params.pin_offset`, the
cell edge to pin centre distance of whichever datum is active.

For the `holes` datum two independent conditions follow, and `_cell_size`
handles the first one:

* the two holes on one cell edge are `cell - 2*ho` apart, so that distance has
  to be a whole number of grid pitches;
* the cell corner has to sit at `k * pitch - ho`, which is the packer's job
  (`_snap_up`, below).

The `slots` datum only needs the second one: its slot centres are moved along
their own edge instead of the cell being grown (see *The datum* below).

`_grid_size` grows one cell side `s` to the next admissible length:

```
steps = max(0, ceil((s - 2*ho) / grid - _GRID_EPS))
grown = steps * grid + 2*ho
```

and `_cell_size` takes `max(s, grown)` per axis, so a cell is only ever grown,
never shrunk. `_GRID_EPS` (1e-9, in grid steps) keeps a side that is already an
exact multiple from being pushed up one whole pitch by floating point noise.
The board stays centred in the grown cell, so the padding grows past `gap/2` —
the report prints `padding ≥ 15.0 mm` instead of `padding 15.0 mm` whenever the
grid is on.

Worked arithmetic for the smallest board in the sample set, RBARF, with
`datum = holes` and otherwise default parameters (`gap` 30, `hole_dia` 5,
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

With `hole_grid = 0` the same cell is exactly 43.405 x 41.610 mm — and so it is
with the default `datum = slots` at any grid, because 15 mm of padding is
already more than the 9.75 mm `min_pad_for_slots` and 43.405 mm is wider than
the `2 * 8 + 12 = 28` mm the two bottom slots need.

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

## Grid snapping, overflow and centring

`ho` is `params.pin_offset` throughout: `slot_inner - pin_dia/2` (5.25 mm by
default) for the `slots` datum, `hole_offset` (4.5 mm) for `holes`. The packer
does not care which datum produced it.

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
whole number of pitches and every pin stays on the grid measured from the
sheet origin. With no placed cell at all the offset is zero.

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

Three obround slots, all of them completely inside the cell's own padding:

```
xl, xr = _bottom_slot_xs(x, w, params, grid)     # the two bottom slot centres
yc     = _left_slot_y(y, h, params, grid)        # the left slot centre
bottom = y + slot_offset + slot_width/2          # centre line of a bottom slot
left   = x + slot_offset + slot_width/2          # centre line of the left slot

slots = [(xl, bottom, slot_length, slot_width),  # (cx, cy, w along x, h along y)
         (xr, bottom, slot_length, slot_width),
         (left, yc,   slot_width,  slot_length)]

pin  = slot_inner - pin_dia/2                    # == params.pin_offset
pins = [(xl, y + pin), (xr, y + pin), (x + pin, yc)]
```

The two bottom slots run along x, the left one along y. Each pin sits tangent
to the slot wall nearest the board — a bottom pin touches its slot from below,
the left pin from the left — because the piece is pushed toward the bottom-left
`datum_corner`. Two contacts on the bottom fix y and rotation, the third fixes
x: three contacts, exact constraint, no over-determination.

Nothing here can reach a neighbouring cell or the scissor zone. A slot's outer
wall is `slot_offset` inside the cell edge, its inner wall `slot_inner`, and the
cell always has at least `min_pad_for_slots = slot_inner + slot_web` of padding
(`_cell_of` grew it otherwise), so the board keeps its `slot_web` of foil.
Along its own edge a slot centre is confined to the span `_slot_span` returns,
`[slot_offset + length/2, size - slot_offset - length/2]`, which keeps its round
ends `slot_offset` clear of the two cell edges it runs into.

**Slots and the pin grid.** The cell is never grown for the grid; the slot
centre moves along its edge instead. `_on_grid_in(value, lo, hi, grid)` returns
the grid point nearest `value` that is still inside `[lo, hi]`, falling back to
the clamped `value` when the span holds no grid point at all (a very small cell
— the report then flags the pin `OFF GRID`). `_left_slot_y` applies that to the
cell's mid height. `_bottom_slot_xs` starts from `slot_corner` inside each
bottom corner, snaps both, and then makes sure the two slots do not overlap: if
the snapped pair is closer than `slot_length`, the right one is re-snapped
freely and the left one inside `[lo, xr - length]`; if that still fails, the
cell is too short and the two are spread to the ends of the span. With
`grid <= 0` both slots sit exactly `slot_corner` from the corners.

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
and every cell is exactly `board + gap`.

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
| `Datum:` | the mode and its numbers — for `slots` two lines (slot size, the two wall offsets, the pin diameter and the push direction; then the placement rule and the web), for `holes` one (diameter, inset and the resulting centre offset), for `none` just `none` |
| `Grid:` | pitch, whether every *pin* is on the raster and how many there are, plus one line saying either how much cell area the grid cost (`holes`) or that the cells were not grown and the slot centres moved instead (`slots`) — or `off` |

Under `slots` the `Datum:` block also carries a `WARNING:` line when
`slot_offset` is smaller than the dotted band — how far inside the cell edge
the dots reach, `dot_line_gap/2 + dot_dia/2`: the cut would then run into the
slots.

`_pins_on_grid` re-checks every *pin* centre of every area against the raster
from the sheet origin with `_FIT_EPS` tolerance, so the report verifies the
invariant rather than asserting it. `_grid_waste` sums
`area.w * area.h - (board.width + gap) * (board.height + gap)` over all areas:
the square millimetres the grid cost against plain `board + gap`.

Then one block per area: the label (with `(mirrored)`), the cell rectangle, the
board rectangle and its size, then the datum. Under `slots` that is the datum
corner, the three slots as `(cx, cy) w x h`, the three jig pin centres and a
`nesting:` line giving the push direction in sheet *and* board coordinates (a
mirrored side is pushed toward the board's physical bottom-right). Under
`holes` it is the four hole centres and the same four as pins. Every coordinate
is printed twice, on the sheet and relative to the bottom left corner of the
board — the second is the number a fixture plate is drilled from. A pin that
could not be put on the raster is marked `OFF GRID (the cell is too small for
it)`. After the datum come the paste opening count, the candidate pads by state
and the closed opening count. The last line counts the divider dots, their
diameter and pitch, the number of dotted lines, how far inside its edge each of
them runs, and whether the block boundary is included.

## Worked example

Eight KiCad gerber zips in one folder, `--batch --no-open`, everything left at
the defaults: 380x280 landscape, `gap` 30, `datum = slots` (4.5 x 12 mm slots,
2.25 mm offset, 8 mm corner distance, 3 mm web, 3 mm pins), dots 0.5 mm at 3 mm
pitch, `dot_line_gap` 2.5, `hole_grid` 8, `outer_border` off, `sort = height`.

The first pack sees all 13 relevant sides and the block measures
**371.65 x 244.02 mm** — it fits. Before generating, the CLI drops every enabled
side without a single opening: `PhotoAmp bottom` has no paste layer and nothing
was opened on it, so it goes, and the remaining 12 sides are packed again, to
**371.65 x 204.02 mm at (2.75, 34.75)..(374.40, 238.77)**, bottom-left
heuristic. Dropping one side changes both the block and sometimes the winning
heuristic, which is worth remembering when comparing the first preview with the
generated one.

| cell | side | x | y | w | h |
| --- | --- | ---: | ---: | ---: | ---: |
| 1 | LED lamp for gardening top | 2.75 | 34.75 | 130.05 | 108.05 |
| 2 | alacrity badge top | 138.75 | 34.75 | 115.65 | 84.05 |
| 3 | alacrity badge bottom (mirrored) | 258.75 | 34.75 | 115.65 | 84.05 |
| 4 | Midea WiFi Dongle top | 138.75 | 122.75 | 50.04 | 66.23 |
| 5 | Midea WiFi Dongle bottom (mirrored) | 194.75 | 122.75 | 50.04 | 66.23 |
| 6 | indxworks top | 250.75 | 122.75 | 55.45 | 58.75 |
| 7 | indxworks bottom (mirrored) | 306.75 | 122.75 | 55.45 | 58.75 |
| 8 | airbox top | 2.75 | 146.75 | 80.85 | 50.12 |
| 9 | PhotoAmp top | 250.75 | 186.75 | 118.44 | 44.02 |
| 10 | KliFan bottom (mirrored) | 90.75 | 194.75 | 96.47 | 44.02 |
| 11 | RBARF top | 90.75 | 146.75 | 43.40 | 41.61 |
| 12 | RBARF bottom (mirrored) | 194.75 | 194.75 | 43.40 | 41.61 |

Every cell is exactly `board + gap`: no cell is grown, because 15 mm of padding
already covers `min_pad_for_slots`. The report says 12 cells placed with
MaxRects (bottom-left), 0 overflow, all 36 pins (12 cells x 3) on the 8 mm
grid, the cells not grown for it, and 887 divider dots on 41 dotted lines —
one line per cell edge, 1.25 mm inside it, a few of them merged where two cells
are stacked along the same line. No dot is lost to a slot here: the dots reach
1.5 mm inside the cell edge and the slots start at `slot_offset` 2.25 mm. They
are only swallowed when a datum opening reaches the line itself, as with
`hole_inset = -2.5` (859 dots instead of 929).

The same set with the other two datums, everything else unchanged:

| `datum` | block after dropping `PhotoAmp bottom` | cells | grid |
| --- | --- | --- | --- |
| `slots` | 371.65 x 204.02 mm | exactly `board + gap` | 36 pins on the raster, no growth |
| `holes` | 353.00 x 273.00 mm | grown by 9,090.9 mm2 in total | 48 pins on the raster |
| `none` | 361.35 x 194.30 mm | exactly `board + gap` | off (no pins to align) |

`holes` produces a narrower but much taller block: every cell is padded out to
the next grid multiple, RBARF's 43.405 x 41.610 mm becoming 49 x 49 mm. `slots`
gets the same fixture-plate raster for free, because only the slot centres have
to move. `none` is the floor — 371.65 - 361.35 = 10.3 mm of the slots block is
the extra width `2 * slot_corner + slot_length = 28` mm forces on the narrowest
cells.

## Performance

`pack` is cheap enough to run on every keystroke. Measured on the eight sample
projects (13 sides, mean of 50 calls):

| configuration | time per `pack` |
| --- | --- |
| `datum = holes`, `hole_grid = 8` | ~2.7 ms |
| `hole_grid = 0` | ~1.5 ms |

The `slots` datum adds three slots and a `_Cover` lookup per dot on top of the
grid-free packing; it is in the same range.


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
