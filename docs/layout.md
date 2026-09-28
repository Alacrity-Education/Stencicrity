# Layout: cells, packing, dividers and the datum

`src/layout.rs` turns a list of enabled board sides into one `Layout`: where
every cell sits on the sheet, where its alignment features are, where the
dotted cut lines run and which dots survive. It is the one module with no
geometry library behind it — everything here is rectangles and arithmetic on
`f64` — and it is pure: `pack(&projects, &sides, &config)` reads the model and
returns a value, touching nothing.

Named tolerances, never inlined:

| Constant | Value | For |
| --- | --- | --- |
| `EPS` | 1e-9 | general |
| `FIT_EPS` | 1e-6 | "does it fit", grid checks |
| `MERGE_EPS` | 1e-6 | two coordinates are the same divider line |
| `ALIGN_EPS` | 1e-9 | two touching cell edges are the same edge |
| `HOLE_EPS` | 1e-6 | two dowel holes are the same hole |
| `GRID_EPS` | 1e-9 | slack of the raster arithmetic, in steps |
| `MAX_PITCH_STEPS` | 1000 | safety net of `edge_for_slots` |

## Cells

Every enabled side becomes one rectangular **cell**. `cell_of(board_w,
board_h, params, grid)` sizes it:

1. the base is `board + gap` — `gap/2` of padding on each of the four sides;
2. with `datum = slots` the padding floor rises to
   `max(gap/2, min_pad_for_slots())` = `max(15, slot_inner + slot_web)` = 15 mm
   at the defaults, then both sides are rounded **up to a whole number of
   `slot_pitch`** (`ceil_pitch`), and finally grown until the longer edge
   carries two slots and the shorter one carries one (`edge_for_slots`);
3. with `datum = holes` the cell is grown by `grid_size` until the
   hole-to-hole distance `size - 2·hole_offset` is a whole number of
   `hole_grid`;
4. with `datum = none` (or a raster of 0) it stays `board + gap`.

The board is always centred inside its cell, so growing a cell only adds foil
around the board. `pack` computes the placement from that:

```
bx = cx + (cw - board_w) / 2
by = cy + (ch - board_h) / 2
dx = bx - (if mirror { -maxx } else { minx })
dy = by - miny
```

### Growing a cell for the raster

`slot_steps(size, params)` is the arithmetic the whole `slots` datum rests on.
A cell corner sits at `k · pitch − pin_offset`, so measured from that corner
the raster positions are `pin_offset + k · pitch`. A slot needs
`slot_offset + slot_length/2` of clearance from each end of its edge, so the
usable range is

```
lo = slot_offset + slot_length / 2
hi = size - lo
first = ceil ((lo - pin_offset) / pitch)
last  = floor((hi - pin_offset) / pitch)
```

and the edge carries `last - first + 1` slots. With the defaults
(`pin_offset` 3.5, `lo` 4.5, `pitch` 20) `first` is 1: the raster position at
3.5 mm from the corner is too close to the end, which is exactly why it is
free for the orientation X. An edge of `n` pitches therefore carries `n − 1`
slots — 40 mm one, 60 mm two, 80 mm three — and `edge_for_slots(count)` walks
`k = 1, 2, 3 …` until `slot_count(k · pitch) >= count`.

`grid_size`, `snap_up`, `snap_down` and `ceil_pitch` are the four rounding
helpers; they all treat a value within `GRID_EPS` (or `ALIGN_EPS`) of a raster
multiple as being on it, so a coordinate that arrived through a different
addition order is not rounded up by one whole pitch.

## Ordering

`ordered_sides` decides what the packer sees first. With `sort = height` the
key is `(-board_height, -board_width, natural project name, top before
bottom)`, rounded to 1e-6 first so two boards that differ by a float wobble
compare equal and fall through to the name; with `sort = name` it is
`(natural project name, top before bottom)`. `util::natural_key` compares
digit runs as numbers, so `R10` sorts after `R2`, and text runs case
insensitively.

Ordering is the only source of non-determinism the packer could have, and
there is none: the same inputs give the same sheet every time.

## The MaxRects packer

`maxrects(cells, sheet, heuristic, ho, grid)` is a textbook MaxRects bin
packer with one addition: the bottom-left corner of a candidate position is
first pushed up onto the raster with `snap_up`, so a cell is only ever placed
where its pins land on the grid.

It keeps a list of maximal free rectangles, starting with the whole sheet. For
each cell in order it scores every free rectangle that still holds the cell
after the snap, takes the best, and then splits every free rectangle the
placement overlaps into its maximal remainders (`split`) and drops the ones
another rectangle already covers (`prune`).

| Heuristic | Score | Meaning |
| --- | --- | --- |
| `bottom-left` | `(y + h, x)` | lowest top edge, then leftmost |
| `best short side` | `(min leftover, max leftover)` | tightest fit on the tighter axis |
| `best area` | `(leftover area, min leftover)` | least wasted area |

Ties inside one heuristic are broken by the placement itself (`x`, then `y`),
so no two runs disagree. `best_packing` runs all three and keeps the result
with the fewest unplaced cells, then the smallest bounding box of the placed
ones, then the earliest heuristic in `HEURISTICS`. The winner's name ends up
in `Layout.heuristic` and in the report.

## Overflow and centring

Cells that fit nowhere are lined up to the right of the sheet, each snapped to
the raster, and counted in `Layout.overflow`; they still get an `Area` (with
`overflow = true`) so the preview can draw them and the report can name them.

The block of *placed* cells is then centred on the stencil by
`snap_down(((sheet - block) / 2).max(0), grid)` — a whole number of raster
pitches, so every pin stays on the raster measured from the sheet origin.
Overflowing cells move with it.

`aligned(starts, sizes, offset)` does the translation. Floating point addition
is not associative, so `(start + offset) + size` can miss
`(start + size) + offset` by an ULP and two cells that touched in the bin would
overlap by ~1e-13 mm on the sheet — enough to make `dividers_of` see two lines
where there is one. Every translated coordinate is therefore snapped onto the
far edge of an already translated cell when the two are within `ALIGN_EPS`.

## Dividers

Every cell owns one dotted line along each of its four edges, running
`dot_line_gap / 2` **inside** that edge; the extent of a line is the extent of
its edge shortened by the same amount at both ends, so a cell's four lines are
exactly its rectangle inset by `dot_line_gap / 2`, closing at the corners.

Two cells that touch therefore show two lines `dot_line_gap` apart, one
belonging to each, and the scissors cut between them. No line is ever drawn
outside a cell. `dot_line_gap = 0` puts every line on the edge itself, so
touching cells share one.

`dividers_of(areas, outer, line_gap)` collects the pieces, snapping each line's
coordinate with a `Snap` (a `MERGE_EPS` tolerance map) so two cells that agree
to a float wobble contribute to the same line, and merges collinear pieces with
`merge`. A line whose *edge* lies on the outer boundary of the block is dropped
unless `outer_border` — nothing has to be cut apart out there. The result is a
`Vec<(x0, y0, x1, y1)>`, verticals first, each sorted by coordinate.

## Dots

`dots_of(dividers, params, cover)` walks each segment, fits
`n = floor(length / dot_pitch)` gaps on it and centres the run:
`start = (length - n · dot_pitch) / 2`, then `n + 1` dots from there. A
zero-length segment gets one dot; a segment shorter than one pitch gets one
dot in the middle. Dots are then deduplicated with a `Dedupe` spatial hash at
`dot_dia`, so the shared corner of two lines is one opening.

### The clearance rule

A dot that sits in, or too close to, a datum opening is dropped: the neck of
metal between the two would tear when the sheet is cut. The rule is

```
distance(dot centre, feature) < dot_dia / 2 + dot_clearance
```

so `dot_clearance = 0` drops only the dots that actually overlap a feature,
and the default 0.5 mm keeps half a millimetre of steel between a 0.5 mm dot
and the nearest slot wall.

`Cover` answers that in O(1). Every datum feature is stored as a **capsule** —
a segment plus a radius, the set of points within that radius of the segment —
which makes the distance from a point to it
`segment_distance(point, segment) - radius`:

| Feature | Segment | Radius |
| --- | --- | --- |
| obround slot | between the two cap centres, along the slot's long axis | half the short axis (`slot_width / 2`) |
| dowel hole | the centre, degenerate | `hole_dia / 2` |
| orientation X | its two `marker_strokes`, so two capsules | `dot_dia / 2` — the stroke half width |

Capsules are bucketed into a hash grid whose cell is the largest feature plus
twice the margin, so a dot is only measured against the features filed under
its own bucket. `Layout.dots_dropped` counts what went;
`layout::dropped_dots(&layout)` recomputes the same number from a finished
layout.

## The datum

`datum_of(rect, params, seen)` returns `(holes, slots, pins)` for one cell.

### `slots`

Obround slots at every raster position that fits, along the cell's **bottom**
edge and its **left** edge; the top and right edges get none.

| Quantity | Formula | Default |
| --- | --- | --- |
| slot centre, across the edge | `slot_offset + slot_width / 2` | 2.75 mm inside |
| slot outer wall | `slot_offset` | 0.5 mm inside |
| slot inner wall (`slot_inner`) | `slot_offset + slot_width` | 5.0 mm inside |
| pin centre (`pin_offset`) | `slot_inner - pin_dia / 2` | 3.5 mm inside |
| foil to the board (`slot_web`) | guaranteed by `min_pad_for_slots` | ≥ 3 mm |

A slot on the bottom edge is `(cx, cy + across, slot_length, slot_width)` — the
long axis along the edge — and one on the left edge swaps the two sizes. Every
slot lies entirely inside its own cell: its outer wall is 0.5 mm in, which
*clips the cell's own dotted line* at 1.25 mm but never reaches the
neighbouring cell. That is deliberate, and the report says so; pushing the
slots that far out frees the foil beside the board for the squeegee.

The jig is a plate with `pin_dia` pins on the same raster. The piece is dropped
over them, then pushed toward its **datum corner** — always the cell's
bottom-left in sheet coordinates — until the slot wall nearest the board
touches its pin. Two pins on one edge fix that axis and the rotation, one on
the other edge fixes the second axis: three contacts, exactly constrained. All
the slots that fit are opened, so a modular jig may use whichever suit it.

Because a bottom side is placed mirrored, its datum corner is the board's
physical bottom-right; the report says so per cell.

### The orientation X

`marker_of(x, y, params)` puts the X at `(x + pin_offset, y + pin_offset)` —
the first raster point inside the datum corner, where the left pin column
meets the bottom pin row, and the one raster point of the two edges that never
carries a slot (`first` is 1, not 0).

`marker_strokes(centre, size)` returns the two segments, at +45° and −45°,
each of length `size`, so together they are an X whose bounding square has the
half-side `marker_half = size·√2/4 + dot_dia/2`. At the defaults that is
1.66 mm, against the 3.5 mm the X sits inside the edge: 1.84 mm clear of the
cell edge and 0.59 mm clear of the cell's own dotted line. The writer strokes
the X with a `dot_dia` round aperture, so it is an opening like any other;
`Cover` keeps divider dots out of it.

`marker_trouble()` in the report warns when the X would cross the cell edge or
reach a slot.

### `holes`

Four round dowel holes near the cell corners, `hole_offset = hole_inset +
hole_dia/2` inside each edge. A negative `hole_inset` moves a hole onto the
dotted line; two cells that would then share a hole get one, because
`holes_of` runs every centre through a `Dedupe` at `HOLE_EPS`. Simple, but
over-constrained: four pins in four holes only fit with clearance, so the
piece can still shift. `hole_grid` (default 8 mm) is the raster for this datum
and applies to it only.

### `none`

No features, no raster, cells exactly `board + gap`.

## `fits`

```
fits = overflow == 0
    && block inside (0, 0, sheet_w, sheet_h) within FIT_EPS
```

A layout that does not fit is still complete: the gerbers are written, the
preview grows to cover the parked cells, and the TUI asks for a second `w`.

## `layout_report`

`layout_report(&projects, &layout, &config)` renders the text block that heads
`stencil-report.txt`. It is the one impure-ish function here (it formats), and
it reimplements a few Python format specs (`fmt_g` for `%g`,
`fmt_thousands_1` for `{:,.1f}`) so the text is byte-identical to what the
retired implementation produced.

Header lines: `Stencil:`, `Block:`, `Cells:`, then a `Datum:` block (three
lines plus warnings for `slots`, one line for `holes`, one for `none`), a
`Marker:` block under `slots`, and a `Grid:` block saying whether every pin is
on the raster and what the growth cost in cell area (`grid_waste`). Then one
paragraph per cell — its rectangle, the board rectangle, the datum corner, the
marker, every slot or hole and every pin both in sheet coordinates and
relative to the board's bottom-left corner, the nesting direction and the pad
counts — and a closing `Divider dots:` line.

The three consistency checks it can warn about: `crowded()` (the slots of one
cell would overlap each other), `marker_trouble()` (the X crosses the cell edge
or reaches a slot) and `pins_on_grid()` (a cell too small for its raster).

## Worked example

Eight example boards, `stencicrity --batch --no-open --size 420x320` with the
stored configuration:

```
Stencil: 420.0 x 320.0 mm (420x320 landscape)
Block:   380.00 x 220.00 mm at (16.50, 56.50)..(396.50, 276.50)   FITS
Cells:   11 placed with MaxRects (bottom-left), 0 overflow, gap 30.0 mm (padding ≥ 15.00 mm), sort by height
Grid:    pin centres on the 20.0 mm slot_pitch raster: yes (63 pin(s), measured from the sheet origin)
Divider dots: 742 (⌀0.50 mm, pitch 3.00 mm, 35 dotted line(s), one per cell edge, 1.25 mm inside the edge; 138 dropped within 0.5 mm of a slot, hole or marker)
```

The first cell, `alacrity badge top`, board 85.65 x 54.05 mm:

| Step | Result |
| --- | --- |
| `board + gap` | 115.65 x 84.05 |
| `ceil_pitch(·, 20)` | 120 x 100 |
| `edge_for_slots(2) = 60`, `edge_for_slots(1) = 40` | already satisfied, cell stays 120 x 100 |
| placed at | (16.50, 56.50) — and `16.50 = 1·20 − 3.5`, so the pins land on the raster |
| board centred in the cell | x 33.67..119.33, y 79.47..133.53 |
| `slot_count(120)` / `slot_count(100)` | 5 on the bottom edge, 4 on the left |
| slot centres, bottom | x = 40, 60, 80, 100, 120 (= corner + 3.5 + k·20), y = 59.25 |
| slot centres, left | x = 19.25, y = 80, 100, 120, 140 |
| pins | the same x/y, but 3.5 mm inside the edge: (40, 60) … and (20, 80) … |
| marker X | (20.00, 60.00) — `pin_offset` in from both edges |

Nine pins for that cell, 63 over the sheet, every one of them a multiple of
20 mm from the sheet origin. The `Grid:` line also reports that the cells cost
17,490.8 mm² more than plain `board + gap` — the least this raster allows.

## See also

* [data-model.md](data-model.md) for `LayoutParams`, `Area` and `Layout`.
* [kinematic-alignment.md](kinematic-alignment.md) for why the `slots` datum
  looks the way it does.
* [render.md](render.md) for how the cells, the dots and the datum are drawn.
