# Data model

Everything the program passes around lives in `src/model.rs`: eight structs,
one id type, two aliases and the constants. It imports `geo` and
`crate::gerber`, so it is the bottom of the dependency graph and every other
module can use it. The module comment is the authoritative description of the
coordinate systems and of the layout model; this file lists the fields.

## Coordinate systems

| Name | Origin | Units | Where |
| --- | --- | --- | --- |
| board | whatever KiCad used; top and bottom of one project share it | mm | `Pad.x/y`, `Pad.geom`, `Side.board_bbox`, `Project.bbox`, everything `gerber` produces |
| sheet | bottom-left corner of the stencil, X right, Y up | mm | `Area.x/y/w/h`, `Area.board_rect`, `Layout.dots`, `Layout.dividers`, `Area.slots/holes/pins/marker`, the report, the preview rulers |
| image | top-left pixel of the PNG, Y down | px | inside `render.rs` only (`Frame::to_px` maps sheet → image and flips Y) |

`Transform` maps board to sheet:

```
x' = (-x if mirror else x) + dx
y' =  y                    + dy
```

A bottom side is placed mirrored about the Y axis, so that the cut-out piece
matches the board once the board is flipped over. A mirrored board that
occupies `[minx, maxx]` in board x occupies `[-maxx, -minx]` after mirroring,
which is why `pack` computes `dx = bx - if mirror { -maxx } else { minx }`.

## Aliases and small types

| Item | Definition | Notes |
| --- | --- | --- |
| `Geom` | `geo::MultiPolygon<f64>` | planar geometry in mm; the one geometry type the whole crate uses |
| `Bounds` | `(f64, f64, f64, f64)` | `(minx, miny, maxx, maxy)` |
| `SideId` | `{ project: usize, side: usize }` | an index pair into a `&[Project]`; `Copy`, `Eq`, `Hash`. `get()` / `get_mut()` resolve it, `all_sides()` enumerates every side in order |

`SideId` exists because a `Side` cannot hold a back-reference to its `Project`
without fighting the borrow checker: the TUI mutates one pad while reading the
rest, so everything that names a side names it by index. `tui::PadId` is
`(SideId, usize)` for the same reason.

## Constants

| Constant | Value |
| --- | --- |
| `SIDE_TOP`, `SIDE_BOTTOM` | `"top"`, `"bottom"` |
| `STATES` | `["undefined", "open", "ignore"]` (also `STATE_UNDEFINED` / `STATE_OPEN` / `STATE_IGNORE`) |
| `STENCIL_SIZES` | `(270,270) (380,280) (420,320) (450,350) (460,460) (520,420) (600,600) (700,600)`, long side first, smallest first |
| `DEFAULT_STENCIL_SIZE` | `(380, 280)` — a literal, not `STENCIL_SIZES[0]` |
| `ORIENTATIONS` | `["landscape", "portrait"]` |
| `SORT_ORDERS` | `["height", "name"]` |
| `DATUM_MODES` | `["slots", "holes", "none"]` |
| `DEFAULT_IGNORE_PREFIXES` | `["NT", "TP"]` |

Helpers: `size_label((380,280))` → `"380x280"`; `parse_size("380 X 280")` →
`Ok((380, 280))` (case insensitive, `×` accepted, long side first, `Err` on
anything else); `side_key(p, s)` → `"<project>/<side>"`.

## `Transform`

`Copy`, `Default` (identity), `PartialEq`.

| Field | Type | Meaning |
| --- | --- | --- |
| `mirror` | `bool` | negate x before translating |
| `dx`, `dy` | `f64` | translation, mm |

`apply(x, y) -> (f64, f64)` for a point, `apply_geom(&Geom) -> Geom` for a
geometry (`geo::MapCoords` over every vertex).

## `Pad`

One pad flash found on a copper layer. `Clone`.

| Field | Type | Meaning |
| --- | --- | --- |
| `key` | `String` | `<project>/<side>/<REF>.<pin>@<x>,<y>` with three decimals, board coordinates — the identity in the configuration file |
| `project`, `side` | `String` | the side it belongs to, by name |
| `ref_`, `pin` | `String` | from the `%TO.P` object attribute; `?` and a running index when it has none |
| `x`, `y` | `f64` | flash position, board coordinates, never mirrored |
| `function` | `String` | first token of `AperFunction` (`"SMDPad"`, …), empty when absent |
| `shape` | `String` | `Aperture::describe()`, e.g. `"R 0.28x0.52"` |
| `flash` | `gerber::Flash` | the copper flash itself; the writer re-emits exactly this when the pad is opened |
| `geom` | `Geom` | the pad's copper shape, board coordinates |
| `has_paste` | `bool` | the paste layer already opens this pad |
| `state` | `String` | one of `STATES` |
| `paste_indices` | `Vec<usize>` | indices into `Side::paste_objects()` of the openings covering it |

| Method | Meaning |
| --- | --- |
| `is_candidate()` | `!has_paste` — the user has to decide about it |
| `is_closed()` | `has_paste && state == "ignore"` — a paste opening the user removed |
| `label()` | `"<REF>.<pin>"` |
| `side_key()` | `"<project>/<side>"` |

`has_paste` and `state` are independent: a pad *with* paste is `open` by
default and can be set to `ignore` (closed); a pad *without* paste starts
`undefined` (or `ignore` by the prefix rule) and is set to `open` or `ignore`.
`undefined` never applies to a pasted pad.

## `Side`

One side (top or bottom) of one project — one stencil cell when enabled.

| Field | Type | Meaning |
| --- | --- | --- |
| `project_name` | `String` | duplicated from the project, so a `Side` is usable alone |
| `board_bbox` | `Bounds` | the project's board bounding box, board coordinates |
| `name` | `String` | `"top"` or `"bottom"` |
| `copper`, `paste` | `Option<GerberFile>` | the two layers; at least one of them is present |
| `mirror` | `bool` | place mirrored (set for bottom sides unless `--no-mirror-bottom`) |
| `enabled` | `bool` | the user switch from the file / the Sides page |
| `pads` | `Vec<Pad>` | every copper flash that is a pad, those with paste included, ordered ref / pin / x / y |

| Method | Meaning |
| --- | --- |
| `key()`, `label()` | `"proj/top"`, `"proj top"` |
| `board_width()`, `board_height()` | from `board_bbox` |
| `paste_objects()`, `copper_objects()` | the layers' object slices (empty when the layer is missing) |
| `candidates()`, `pasted_pads()`, `open_pads()`, `closed_pads()` | filtered iterators over `pads` |
| `closed_paste_indices()` | `BTreeSet` of the paste objects the closed pads cover |
| `active_paste_objects()` | the openings that reach the stencil (closed ones removed) |
| `closed_paste_objects()` | the complement; the preview draws these blue |
| `has_openings()` | anything at all is cut for this side — decides whether it gets a cell in the final pack |
| `is_relevant()` | it has paste objects *or* candidate pads — decides whether it is offered at all |

`has_openings()` and `is_relevant()` are deliberately different: a side with
only candidates is relevant (the user may open one) but has no openings until
one is opened, and is then dropped from the sheet with a `dropped …: no
openings at all` line.

## `Project`

| Field | Type | Meaning |
| --- | --- | --- |
| `name` | `String` | from `%TF.ProjectId`, else the source basename; made unique with ` (2)`, ` (3)` |
| `source` | `String` | the zip path or directory it came from |
| `outline` | `Option<GerberFile>` | the `Edge_Cuts` / profile layer, when there is one |
| `bbox` | `Bounds` | the board bounding box: the outline's when present, else the union of the copper and paste bounds |
| `sides` | `Vec<Side>` | top then bottom, whichever exist |

`width()`, `height()` come from `bbox`; `side(name)` looks one up.

## `LayoutParams`

Everything on the TUI Layout page, persisted in the `[layout]` section.
`Clone`, `PartialEq`, `Default` (the values below).

| Field | Config key | Default | Meaning |
| --- | --- | --- | --- |
| `gap` | `spacing` | 30.0 | mm between neighbouring boards; the dotted border runs in the middle |
| `datum` | `datum` | `"slots"` | `slots` \| `holes` \| `none` |
| `hole_dia` | `hole_dia` | 5.0 | dowel hole diameter (holes datum) |
| `hole_inset` | `hole_inset` | 2.0 | cell edge to hole edge; may be negative |
| `slot_width` | `slot_width` | 4.5 | slot size across the cell edge |
| `slot_length` | `slot_length` | 8.0 | slot size along the cell edge |
| `slot_offset` | `slot_offset` | 0.5 | cell edge to the slot's outer wall |
| `slot_pitch` | `slot_pitch` | 20.0 | raster of the modular jig |
| `slot_web` | `slot_web` | 3.0 | minimum foil between a slot's inner wall and the board |
| `pin_dia` | `pin_dia` | 3.0 | jig pin diameter for the slots datum |
| `marker` | `marker` | `true` | cut the orientation X |
| `marker_size` | `marker_size` | 4.0 | stroke length of the X; its width is `dot_dia` |
| `dot_dia` | `dot_dia` | 0.5 | divider dot diameter |
| `dot_pitch` | `dot_pitch` | 3.0 | divider dot spacing |
| `dot_line_gap` | `dot_line_gap` | 2.5 | each cell's dotted line runs this/2 inside its edge |
| `dot_clearance` | `dot_clearance` | 0.5 | metal a divider dot must leave to a slot, hole or marker; less and the dot is dropped |
| `hole_grid` | `hole_grid` | 8.0 | holes datum: hole centres snap to this grid; 0 = off |
| `outer_border` | `outer_border` | `false` | also dot the cell edges on the block boundary |
| `sort` | `sort` | `"height"` | `height` \| `name` |

Derived values, so no call site repeats the arithmetic:

| Method | Value | Default |
| --- | --- | --- |
| `pad()` | `gap / 2` | 15.0 |
| `holes()`, `slots()` | `datum == …` | |
| `hole_offset()` | `hole_inset + hole_dia / 2` — cell edge to hole centre | 4.5 |
| `slot_inner()` | `slot_offset + slot_width` — cell edge to the wall the pin touches | 5.0 |
| `pin_offset()` | `slot_inner() - pin_dia / 2` for slots, `hole_offset()` for holes — cell edge to pin centre | 3.5 |
| `min_pad_for_slots()` | `slot_inner() + slot_web` — the smallest cell padding a slot plus its web fits in | 8.0 |

`pin_offset()` is the single number the packer snaps on: a cell corner sits at
`k * pitch - pin_offset`, so every pin centre of the sheet lands on the raster
measured from the sheet origin.

## `Config`

The whole `.stencicrity` file. `Clone`, `PartialEq`, `Default`.

| Field | Type | Meaning |
| --- | --- | --- |
| `size` | `(u32, u32)` | long side first, one of `STENCIL_SIZES` |
| `orientation` | `String` | one of `ORIENTATIONS` |
| `layout` | `LayoutParams` | the `[layout]` section |
| `ignore_prefixes` | `Vec<String>` | `[rules] ignore_prefixes`, upper case, order preserved |
| `sides` | `BTreeMap<String, bool>` | side key → enabled |
| `pads` | `BTreeMap<String, String>` | pad key → state |

`size_label()` renders the size; `sheet_size()` returns `(width, height)` in mm
honouring the orientation (landscape puts the long side on x). The two maps are
`BTreeMap`s so a rewritten file is in a stable order, and they keep entries for
pads and sides the current gerbers no longer explain.

## `Area`

The placement of one side: its cell on the sheet. All sheet coordinates.

| Field | Type | Meaning |
| --- | --- | --- |
| `side` | `SideId` | which side this cell holds |
| `x`, `y` | `f64` | cell bottom-left corner |
| `w`, `h` | `f64` | cell size |
| `board_rect` | `Bounds` | where the board bounding box lands inside the cell |
| `transform` | `Transform` | board → sheet for this side's objects |
| `row` | `usize` | the packing order; informational only |
| `overflow` | `bool` | did not fit on the sheet; parked to the right of it |
| `holes` | `Vec<(f64, f64)>` | dowel hole centres (holes datum), deduplicated across touching cells |
| `slots` | `Vec<(f64, f64, f64, f64)>` | obround slots `(cx, cy, w, h)`, `w` along x — bottom edge first, then left edge |
| `pins` | `Vec<(f64, f64)>` | jig pin centres, one per slot (equal to `holes` for the holes datum) |
| `datum_corner` | `(f64, f64)` | the corner the piece is pushed toward — always the cell's bottom-left |
| `marker` | `Option<(f64, f64)>` | centre of the orientation X (slots datum with `marker` on) |

`rect()` gives `(x, y, x + w, y + h)`.

## `Layout`

What `pack` returns.

| Field | Type | Meaning |
| --- | --- | --- |
| `params` | `LayoutParams` | a clone of `config.layout` as it was when packing ran |
| `areas` | `Vec<Area>` | one per enabled side, in packing order |
| `width`, `height` | `f64` | the stencil sheet, mm |
| `block` | `Bounds` | bounding box of all cells |
| `fits` | `bool` | nothing overflowed and the block is inside the sheet |
| `dots` | `Vec<(f64, f64)>` | divider dot centres, deduplicated |
| `dividers` | `Vec<(f64, f64, f64, f64)>` | the dotted line segments the dots sit on |
| `heuristic` | `String` | the MaxRects heuristic that won |
| `overflow` | `usize` | how many cells fit nowhere |
| `dots_dropped` | `usize` | dots the `dot_clearance` rule removed for coming too close to a slot, hole or marker |

`block_width()` / `block_height()` come from `block`. `dots_dropped` is
reported by `layout_report`; `layout::dropped_dots(&layout)` recomputes the
same number from a finished `Layout` when only the layout is at hand.

## How they reference each other

```
Vec<Project> ── the one owner of everything
   Project { outline, bbox, sides: Vec<Side> }
      Side { copper, paste: Option<GerberFile>, pads: Vec<Pad> }
         Pad { flash: gerber::Flash, geom: Geom }

Config { size, orientation, layout: LayoutParams,
         sides: BTreeMap<key, bool>, pads: BTreeMap<key, state> }

Layout { params, areas: Vec<Area>, dots, dividers, ... }
   Area { side: SideId ──> &projects[.project].sides[.side] }
```

Nothing points backwards: an `Area` names its side with a `SideId`, a `Pad`
names its side with two `String`s, and the `Config` maps name both with text
keys. That is what lets the TUI hand `&mut [Project]` to one function and
`&[Project]` to another in the same frame.
