# Gerber reader and writer

`src/gerber.rs` is a minimal RS-274X (Gerber X2) reader that turns a gerber
file into `geo` geometry, and `src/writer.rs` is the matching writer. Together
they are the only place in the crate that knows the file format; everything
else works on `GraphicObject`s and `Geom`s.

The reader is deliberately strict: anything it cannot represent exactly is a
`GerberError` rather than a silent approximation, because the output is cut
into steel. The subset it does accept is what KiCad emits.

## What the parser supports

`parse_gerber(text, name) -> Result<GerberFile, GerberError>` walks the file as
a sequence of blocks: `%…%` is an extended command, everything up to the next
`*` is a data block. Whitespace between blocks is skipped.

| Command | Handling |
| --- | --- |
| `%FSLAX46Y46*%` | format spec: leading (`L`) or trailing (`T`) zero omission, absolute (`A`) or incremental (`I`), integer and decimal digit counts. Anything malformed is an error. |
| `%MOMM*%` / `%MOIN*%` | unit; inches scale coordinates and standard aperture modifiers by 25.4. `G70` / `G71` switch the same scale mid-file. |
| `%ADD<code><template>[,<mods>]*%` | aperture definition; `C`, `R`, `O`, `P` are built in, any other template names a macro that must already be defined. |
| `%AM<name>*…*%` | aperture macro: the blocks after the name are kept verbatim and evaluated per aperture. |
| `%LPD*%` / `%LPC*%` | polarity; stored on every object as `dark`. |
| `%TF.…*%`, `%TA.…*%`, `%TO.…*%`, `%TD…*%` | file, aperture and object attributes. `TA` attaches to the next `AD`, `TO` to the next objects, `TD` deletes one attribute or (bare) all of them. |
| `G01` / `G02` / `G03` | linear, clockwise, counter-clockwise interpolation. |
| `G74` / `G75` | single- and multi-quadrant arc mode. |
| `G36` / `G37` | region start / end. |
| `D01` / `D02` / `D03` | interpolate, move, flash. `D10` and up select an aperture; an undefined one is an error. |
| `G90` / `G91` | absolute / incremental coordinates. |
| `M00` / `M02` | end of file; parsing stops. |
| `G04` | comment, ignored. `G54` / `G55` are harmless prefixes. |
| `IP`, `IN`, `LN`, `IJ`, `IR` | ignored. |

Rejected with a `GerberError`, because honouring them would change the
geometry and ignoring them would produce a wrong stencil:

| Command | Why |
| --- | --- |
| `%SR…*%` with a repeat count | step and repeat would multiply objects |
| `%LM…*%` other than `N` | aperture mirroring |
| `%LR…*%` other than 0, `%LS…*%` other than 1 | aperture rotation / scaling |
| `%AS…*%` other than `AXBY` | axis swap |
| `%OF…*%`, `%SF…*%`, `%MI…*%` with a non-zero value | deprecated transformations |
| an unknown macro name, an unsupported macro primitive | no geometry to build |

Two deprecated behaviours *are* accepted, because real files use them: a data
block with coordinates but no `D` code repeats the last operation (treated as
`D01`), and trailing-zero omission pads the digits back out to
`int_digits + dec_digits`.

## Apertures to geometry

`Aperture::geometry()` builds the shape centred on the origin, once, into a
`OnceCell`; `Aperture::prims()` does the same for the evaluated macro
primitives. `object_geometry(obj)` then places it.

| Template | Modifiers | Geometry |
| --- | --- | --- |
| `C` | `dia[, hole]` | circle |
| `R` | `w, h[, hole]` | rectangle |
| `O` | `w, h[, hole]` | obround: a circle when `w == h`, else the segment between the two cap centres buffered by half the short axis |
| `P` | `dia, n[, rot[, hole]]` | regular `n`-gon, first vertex at `rot` |
| macro | evaluated | union of the exposure-on primitives, minus the exposure-off ones, in order |

A non-zero hole modifier is subtracted last, as a circle at the origin.

Circles are discretised to **96 vertices** (`CIRCLE_SEGS`), the first at angle
0 — the same vertex set `shapely`'s `Point.buffer(r, quad_segs=24)` produces,
which is what made the port comparable object for object against the
implementation it replaced.

Arcs are discretised to a chord error below `ARC_TOLERANCE` (0.004 mm): the
step angle is `2·acos(1 − tol/r)` capped at 45°, and the number of chords is
clamped to `1..=4000`. `arc_sweep` computes the signed sweep; coincident start
and end points with a zero sweep mean a full circle.

Strokes:

* a **round** aperture buffers the path — one capsule per segment (a rectangle
  plus two 48-vertex cap arcs, stepping in 360/96° increments from the segment
  normal), unioned;
* any **other** aperture takes the convex hull of the aperture's exterior
  vertices translated to every point of the path. That is exact for a convex
  aperture on a straight path, which is all KiCad emits.

Regions become the union of their contours; `fix()` — an even-odd union with
the empty geometry, resolved by `i_overlay` — turns a self-touching ring into
a proper polygon with holes, standing in for shapely's `make_valid`.

## The macro evaluator

`Macro::evaluate(&params)` binds `$1, $2, …` to the aperture's modifiers, walks
the macro's blocks and returns a `Vec<MacroPrim>`. A block is a comment (`0 …`,
skipped), a variable assignment (`$4=$1x0.5`) or a primitive
(`<code>,<mod>,…`). Every modifier is an expression.

`Expr` is a recursive-descent parser over `+ - x X / ( ) $n` with the usual
precedence; `x` and `X` are both multiplication, as the spec requires.

| Code | Primitive | Reduced to |
| --- | --- | --- |
| 1 | circle | `MacroPrim::Circle` |
| 2, 20 | vector line | 4-point `Outline` (a degenerate one becomes a circle) |
| 21 | centre line | 4-point `Outline` |
| 4 | outline | `Outline` of the listed points (a repeated closing point is dropped) |
| 5 | polygon | `Outline` of `n` points on a circle |
| 6 | moiré | ignored — decoration only |
| 7 | thermal | the ring minus the cross, as one `Outline` per resulting polygon |
| anything else | | `GerberError` |

Every primitive takes an optional trailing rotation about the macro origin.
`MacroPrim` carries only two shapes — a circle or an outline — which is what
makes `mirrored()` (negate every x) and `as_gerber()` (one `1,…` or `4,…`
line) trivial, and what lets the writer re-emit a macro without re-deriving
it.

## Baking apertures for output

The writer never emits the original `%AD` line: an aperture that has been
mirrored is not the same aperture any more. `bake_aperture(&ap, mirror)`
resolves one for output:

* a **macro** aperture becomes `template = "MACRO"` with its evaluated
  primitives, each `mirrored()` when the transform mirrors;
* a **`P`** aperture keeps its modifiers but its rotation becomes `180 - rot`
  (a regular polygon mirrored about the Y axis is the same polygon rotated);
* `C`, `R` and `O` are symmetric about the Y axis and are copied unchanged.

`BakedAperture::key()` is the deduplication key: the template and its
modifiers at six decimals, or `MACRO|<primitive lines>|`, always with the
aperture attributes appended. Two apertures with the same shape but different
`%TA` attributes are therefore two apertures, as they must be.

## Graphic objects

| Type | Fields | Geometry |
| --- | --- | --- |
| `Flash` | `x, y, aperture, attrs, dark` | the aperture shape translated to `(x, y)` |
| `Stroke` | `seg, aperture, attrs, dark` | the buffered / hulled segment above |
| `Region` | `contours: Vec<Vec<Segment>>, attrs, dark` | the union of the contours |

`Segment` is a straight line or an arc (`arc`, `cx`, `cy`, `clockwise`);
`points()` discretises it, `contour_points()` chains a contour's segments
without repeating the shared endpoints.

`GerberFile` holds the `name`, the `file_attrs` (`%TF`), the `macros`, the
`apertures` (by D-code) and the `objects` in file order. `bounds()` is the
union of every object's bounding box and is what the board bounding box comes
from when there is no outline layer. `file_function()` reads
`%TF.FileFunction`, which is how `project::classify_layer` recognises a layer.

Object attributes matter downstream: `%TO.P` carries `<ref>,<pin>[,<name>]`
and is what `pads::detect_pads` builds a pad's identity from, and
`%TA.AperFunction`'s first token is what decides whether a flash is a pad at
all.

## The writer

`GerberWriter::new(file_function)` collects objects and primitives; the body is
buffered so the aperture list can be assembled in front of it by `render()`.

| Method | Emits |
| --- | --- |
| `comment(text)` | `G04 <text>*` with `*` and `%` stripped |
| `add_object(obj, tr)` | a flash, stroke or region under a `Transform` |
| `add_circle(x, y, dia, attrs)` | a `C,<dia>` flash |
| `add_obround(cx, cy, w, h, attrs)` | an `O,<w>X<h>` flash — `w` along x, so a slot on a bottom cell edge is `add_obround(cx, cy, slot_length, slot_width)` and one on a left edge swaps them |
| `add_polygon(points)` | a `G36`/`G37` region; degenerate input is skipped |
| `add_line(x0, y0, x1, y1, width)` | a stroke with a round aperture |
| `add_rect_outline(x0, y0, x1, y1, width)` | four such strokes |

State is tracked so nothing is repeated: a `D<code>` is emitted only when the
aperture changes, `%LPD*%` / `%LPC*%` only when the polarity changes, `G01` /
`G02` / `G03` only when the interpolation changes, and a `D02` move only when
the current point is not already there (a region contour forces one).
Mirroring a transform reverses an arc's direction, so `clockwise` is XOR-ed
with `tr.mirror`. `G75*` is emitted once, right after the aperture list, when
any arc was written.

Coordinates are integer nanometres (`FSLAX46Y46`) rounded half to even.

Apertures are registered on first use: `register()` looks
`BakedAperture::key()` up, assigns the next D-code from 10 and appends the
`%TA` lines, the `%ADD…` line and a closing `%TD*%` when the aperture had
attributes. Macro bodies are deduplicated separately by their text and named
`M1`, `M2`, … as they appear.

## What a generated file looks like

```
%TF.GenerationSoftware,Alacrity-Education,stencicrity,0.2.0*%
%TF.CreationDate,2026-09-28T05:56:29+03:00*%
%TF.FileFunction,Paste,Top*%
%TF.FilePolarity,Positive*%
%FSLAX46Y46*%
G04 Gerber Fmt 4.6, Leading zero omitted, Abs format (unit mm)*
G04 Created by stencicrity 0.2.0*
%MOMM*%
%LPD*%
G01*
G04 APERTURE LIST*
%AMM1*
4,1,4,-0.275000,0.200000,...,-0.275000,0.200000,0*
1,1,0.400000,-0.275000,0.200000*
...
%
%TA.AperFunction,SMDPad,CuDef*%
%ADD10M1*%
%TD*%
%ADD11C,0.500000*%
G04 APERTURE END LIST*
G04 stencil stencil - merged paste layer*
G04 --- alacrity badge top ---*
D10*
X63740000Y120425000D03*
...
M02*
```

`%TF.GenerationSoftware` is the X2 triple `<vendor>,<application>,<version>`:
the vendor is the constant `Alacrity-Education`, the application and the
version default to `stencicrity` and the crate version and can be overridden
with `set_software()`. `project::is_own_output()` looks for either
`stencicrity` or the older `pcbstencil` in that value, so a `stencil-out/`
left by any previous version is never read back in as an input. The creation
date is local time with an offset; `set_created()` overrides it for
reproducible output.

## Known limitations

* **Macro modifiers are not unit-scaled.** `define_aperture` scales standard
  aperture modifiers by the current unit but deliberately leaves macro
  parameters alone, because a macro parameter is not a length in every
  position and KiCad only uses mm. A `MOIN` file with macro apertures would be
  read at 1/25.4 of its size.
* **A non-round stroke aperture is hulled**, which is exact for a convex
  aperture on a straight path and an over-approximation otherwise.
* **Step and repeat is rejected**, not expanded.
* Polarity is per object. A clear object does not erase the dark objects under
  it in `object_geometry`; only the renderer subtracts it, per colour class.
* `Aperture::describe()` rounds to 1/100 mm after quantising to the 1 nm file
  resolution, so it is a label, never a measurement.
