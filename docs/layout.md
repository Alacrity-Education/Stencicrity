# Layout: cells, packing, dividers and holes

`pcbstencil/layout.py` turns a list of enabled board sides into one `Layout`:
where every side sits on the stencil sheet, where the dowel pin holes go, and
which dotted lines mark the border between two boards. It is pure geometry —
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
stretched to fill space; the only thing that ever makes a cell bigger than
`board + gap` is the dowel hole grid.

```
    cell (w, h)                       · = divider dots (two lines per edge)
 ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·   o = dowel pin hole
 ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·
      o                     o         ho = hole_inset + hole_dia/2
 ·                                ·        (LayoutParams.hole_offset)
 ·        +------------+          ·
 ·        |            |          ·   the board bbox is centred in the cell
 ·        |   board    |          ·   padding = (cell - board) / 2,
 ·        |            |          ·   which is gap/2 unless the cell was
 ·        +------------+          ·   grown for the hole grid
 ·                                ·
      o                     o
 ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·
 ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·  ·
 |<-ho->|                            holes sit ho inside both cell edges
```

### Growing a cell for the hole grid

With `hole_grid > 0` every dowel hole on the whole sheet has to land on one
common raster of that pitch, measured from the sheet origin. Two independent
conditions follow from that, and `_cell_size` handles the first one:

* the two holes on one cell edge are `cell - 2*ho` apart, so that distance has
  to be a whole number of grid pitches;
* the cell corner has to sit at `k * pitch - ho`, which is the packer's job
  (`_snap_up`, below).

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

Worked arithmetic for the smallest board in the sample set, RBARF, with the
default parameters (`gap` 30, `hole_dia` 5, `hole_inset` 2, `hole_grid` 8):

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

With `hole_grid = 0` the same cell is exactly 43.405 x 41.610 mm.

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

`_snap_up(value, ho, grid)` returns the smallest cell corner `>= value` whose
holes land on the grid: it computes `steps = (value + ho) / grid` and takes
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
whole number of pitches and every hole stays on the grid measured from the
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
dowel holes, `row` (the packing order index, informational only) and the
`overflow` flag.

## Dividers

`_dividers(areas, outer, line_gap)` returns the dotted **lines**, not the cell
edges — `Layout.dividers` holds `(x0, y0, x1, y1)` tuples that the preview draws
as faint dashed guides and `_dots` walks to place the openings.

1. **Collect.** The four edges of every cell are collected into a dict keyed by
   `(orientation, coordinate)`, the coordinate passed through a `_Snap` with
   `_MERGE_EPS` (1e-6 mm) so that two cells whose edges are the same line to
   within a rounding error share one key. A degenerate edge (extent
   `<= _MERGE_EPS`) is skipped.
2. **Drop the block boundary.** Unless `outer_border` is set, a line whose
   coordinate equals the block's minimum or maximum on its axis is dropped:
   nothing has to be cut apart on the outer boundary of the block. The test
   runs on the cell edge, before doubling, so both lines of a boundary pair go
   away together. The boundary comes from `_block_of(areas)`, overflow cells
   included.
3. **Double.** Each surviving edge becomes two parallel lines at
   `-line_gap/2` and `+line_gap/2`, both with the full extent of the edge; the
   cut runs between them. `line_gap <= 0` keeps a single line on the edge
   itself. The shifted coordinates go through a second `_Snap`, so lines
   belonging to two different edges that land on the same coordinate merge
   instead of being dotted twice.
4. **Merge.** Per line, the collected intervals are merged by `_merge`:
   overlapping *and* touching intervals become one, so a shared edge is dotted
   once and several cells abutting one long border give a single straight
   divider with evenly spaced dots.

## Dots

`_dots(dividers, params)` places the openings along each divider segment:

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

## Holes

`_holes(x, y, w, h, params, seen)` returns the four dowel hole centres of one
cell, at `hole_offset = hole_inset + hole_dia/2` inside both cell edges it is
near, in the order bottom-left, bottom-right, top-right, top-left. With
`holes = off` it returns an empty list, and `Area.holes` is then empty for every
cell.

`hole_inset` may be negative: at `hole_inset = -hole_dia/2` the hole centre sits
exactly on the cell edge, and two cells sharing that edge would put a hole in
the same place. A single `_Dedupe(_HOLE_EPS)` (1e-6 mm) is threaded through all
cells of the sheet, so the second cell simply does not get that hole — the
report prints `dowel holes: none (shared with a neighbouring cell or off)` when
a cell ends up with none at all.

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
| `Cells:` | placed count, winning heuristic, overflow count, gap, padding (`≥` when the grid is on), sort order |
| `Holes:` | diameter, inset and the resulting centre offset — or `off` |
| `Grid:` | pitch, whether every hole is on the raster, the hole count, and the extra cell area the grid cost — or `off (cells are exactly board + gap)` |

`_holes_on_grid` re-checks every hole centre of every area against the raster
from the sheet origin with `_FIT_EPS` tolerance, so the report verifies the
invariant rather than asserting it. `_grid_waste` sums
`area.w * area.h - (board.width + gap) * (board.height + gap)` over all areas:
the square millimetres the grid cost against plain `board + gap`.

Then one block per area: the label (with `(mirrored)`), the cell rectangle, the
board rectangle and its size, the dowel holes both in sheet coordinates and
relative to the bottom left corner of the board — the number a fixture plate is
drilled from — followed by the paste opening count, the candidate pads by state
and the closed opening count. The last line counts the divider dots, their
diameter and pitch, the number of dotted lines, whether an edge is one line or
two, and whether the block boundary is included.

## Worked example

Eight KiCad gerber zips in one folder, `--batch --no-open`, everything left at
the defaults: 380x280 landscape, `gap` 30, holes on at 5 mm diameter and 2 mm
inset, dots 0.5 mm at 3 mm pitch, `dot_line_gap` 2.5, `hole_grid` 8,
`outer_border` off, `sort = height`.

The first pack sees all 13 relevant sides. One cell does not fit, the best-area
heuristic wins, and the block measures **433.0 x 265.0 mm** — `DOES NOT FIT`.
Before generating, the CLI drops every enabled side without a single opening:
`PhotoAmp bottom` has no paste layer and nothing was opened on it, so it goes,
and the remaining 12 sides are packed again. This time the bottom-left
heuristic wins and the block is **353.00 x 273.00 mm at (11.50, 3.50)..(364.50,
276.50)** — it fits. Dropping one side changes both the block and the winning
heuristic, which is worth remembering when comparing the first preview with the
generated one.

| cell | side | x | y | w | h |
| --- | --- | ---: | ---: | ---: | ---: |
| 1 | LED lamp for gardening top | 11.50 | 3.50 | 137.0 | 113.0 |
| 2 | alacrity badge top | 155.50 | 3.50 | 121.0 | 89.0 |
| 3 | alacrity badge bottom (mirrored) | 155.50 | 99.50 | 121.0 | 89.0 |
| 4 | Midea WiFi Dongle top | 283.50 | 3.50 | 57.0 | 73.0 |
| 5 | Midea WiFi Dongle bottom (mirrored) | 283.50 | 83.50 | 57.0 | 73.0 |
| 6 | indxworks top | 11.50 | 123.50 | 57.0 | 65.0 |
| 7 | indxworks bottom (mirrored) | 75.50 | 123.50 | 57.0 | 65.0 |
| 8 | airbox top | 283.50 | 163.50 | 81.0 | 57.0 |
| 9 | PhotoAmp top | 11.50 | 195.50 | 121.0 | 49.0 |
| 10 | KliFan bottom (mirrored) | 139.50 | 195.50 | 97.0 | 49.0 |
| 11 | RBARF top | 243.50 | 227.50 | 49.0 | 49.0 |
| 12 | RBARF bottom (mirrored) | 299.50 | 227.50 | 49.0 | 49.0 |

The report for that run says: 12 cells placed with MaxRects (bottom-left),
0 overflow, gap 30.0 mm (padding ≥ 15.0 mm), sort by height; all 48 dowel holes
on the 8 mm grid; cells grown for the grid cost 9,090.9 mm2 more than
`board + gap`; and 2019 divider dots on 78 dotted lines.

Switching the grid off (`hole_grid = 0`) changes the picture: all 13 sides then
fit, the bottom-left heuristic wins, the block shrinks to 361.35 x 230.84 mm
and the RBARF cell is exactly 43.405 x 41.610 mm. The grid buys a fixture plate
with one hole raster at the price of roughly 9,100 mm2 of cell area and, on this
set, of one board's worth of room.

## Performance

`pack` is cheap enough to run on every keystroke. Measured on the eight sample
projects (13 sides, mean of 50 calls):

| configuration | time per `pack` |
| --- | --- |
| default, `hole_grid = 8` | ~2.7 ms |
| `hole_grid = 0` | ~1.5 ms |

The TUI re-packs whenever a side is toggled, a sheet size is picked or a layout
parameter changes, and the Stencil page packs once per size and orientation to
fill its fits column — currently 8 sizes x 2 orientations = 16 packs, still
well under a frame. Rendering the preview afterwards costs about a second and
dominates everything the layout does; see [`render.md`](render.md).

## See also

* [`architecture.md`](architecture.md) — where `pack` sits in the pipeline.
* [`data-model.md`](data-model.md) — `LayoutParams`, `Area` and `Layout` field
  by field, and the three coordinate systems.
* [`render.md`](render.md) — how the cells, dividers, dots and holes are drawn.
* [`development.md`](development.md) — adding a layout parameter end to end.
* [`kinematic-alignment.md`](kinematic-alignment.md) — a separate design study
  on locating the cut stencil pieces on a pin jig.
