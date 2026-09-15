# Gerber reader and writer

`pcbstencil/gerber.py` is a minimal RS-274X (Gerber X2) reader that turns a
gerber file into shapely geometry, and `pcbstencil/writer.py` is the matching
writer. Neither is a complete implementation of the specification: they cover
what KiCad emits, and anything outside that either raises `GerberError` or is
ignored on purpose. The reader is the only place that touches gerber syntax on
the way in; the writer is the only place that touches it on the way out.
Everything in between works on shapely geometry and on the dataclasses in
[`data-model.md`](data-model.md).

## What the parser supports

`parse_gerber(text, name)` runs `GerberParser` over the file and returns a
`GerberFile` (file attributes, macros, apertures, objects). `_iter_blocks`
splits the text into extended commands (`%...%`) and data blocks (`...*`);
whitespace between blocks is skipped, and an unterminated `%` or `*` raises.

| command | handling |
| --- | --- |
| `FS` | `re.match(r"FS([LT]?)([AI]?)X(\d)(\d)Y(\d)(\d)")`. `T` sets trailing-zero omission, `I` sets incremental coordinates, the four digits set the integer/decimal digit counts (KiCad writes `FSLAX46Y46`). Anything the regex does not match raises |
| `MO` | `MOIN` sets the unit scale to 25.4, anything else to 1.0. `GerberFile.unit` is always `"mm"`: coordinates are converted at read time, never stored in inches |
| `AD` | `_AD_RE` splits `ADD<code><template>[,<mods>]`. `C`, `R`, `O` and `P` are standard templates and their modifiers are scaled to mm; any other name must be a macro already defined in the file |
| `AM` | the whole `%AM...%` command becomes a `Macro(name, blocks)`; the blocks are kept as text and evaluated lazily |
| `LP` | `LPC` sets clear polarity, anything else dark. The flag is copied onto every object created afterwards |
| `TF` | stored in `GerberFile.file_attrs` (this is where `ProjectId`, `FileFunction` and `GenerationSoftware` come from) |
| `TA` | accumulated in the current aperture attribute dictionary and copied into every `Aperture` defined afterwards |
| `TO` | accumulated in the current object attribute dictionary and copied onto every flash, stroke and region created afterwards (`%TO.P` is what pad detection reads, see [`pads-and-config.md`](pads-and-config.md)) |
| `TD` | `%TD.<name>*%` drops that one key from both dictionaries; `%TD*%` with no name clears both completely |
| `G01`/`G02`/`G03` | linear, clockwise and counter-clockwise interpolation |
| `G36`/`G37` | begin and end a region. `D02` inside a region closes the current contour and starts a new one |
| `G74`/`G75` | single-quadrant and multi-quadrant arc mode; the parser starts in multi-quadrant |
| `G70`/`G71` | deprecated inch/mm switches, treated like `MOIN`/`MOMM` |
| `G90`/`G91` | absolute and incremental coordinates |
| `G04` | comment, skipped (also the `G4 ` spelling) |
| `D01`/`D02`/`D03` | interpolate, move, flash |
| `D10` and up | select that aperture; an undefined code raises |
| `M00`/`M02` | end of file: parsing stops there, the rest of the text is never read |
| bare coordinates | a block with coordinates but no `D` code repeats the last operation, i.e. it is treated as `D01` (deprecated, but KiCad-adjacent tools emit it) |
| `G54`, `G55`, `IP`, `IN`, `LN`, `IJ`, `IR` | parsed away and ignored |

Attribute keys are stored without their leading dot, so `%TA.AperFunction,SMDPad,CuDef*%`
becomes `{"AperFunction": "SMDPad,CuDef"}`. `Aperture.function` then returns
only the first comma-separated token, `SMDPad`.

What raises `GerberError`:

| input | why |
| --- | --- |
| `SR` with a repeat count above 1 | step and repeat is not supported. The guard is `re.search(r"X([2-9]\|\d\d)\|Y([2-9]\|\d\d)", cmd)`, so `%SRX1Y1I0J0*%` (the command that closes a block) passes silently |
| `LM` other than `N`, `LR` other than 0, `LS` other than 1 | aperture transformations are not applied to the geometry, so a file that uses them would be read wrong |
| `OF`, `SF` or `MI` with a non-zero `A` or `B` value | deprecated image transformations, same reason |
| `ADD<n><name>` with an unknown macro name | the macro must be defined before it is used |
| an aperture template that is neither `C`/`R`/`O`/`P` nor a macro | nothing to build geometry from |
| a macro primitive code that is not 1, 2, 20, 21, 4, 5, 6 or 7 | unsupported primitive |
| `D01` or `D03` before any aperture was selected | no shape to draw |
| a malformed `FS` or `AD` | cannot be interpreted |
| an unterminated `%` or `*` block | truncated file |
| a macro expression that does not parse | see the evaluator below |

One gap: `AS` (axis select) appears in the same tuple as `OF`, `SF` and `MI`,
but the guard inside only tests those three, so `%ASAYBX*%` is accepted and
ignored instead of rejected. A file with swapped axes would be read wrong
without a warning. This is repeated under "Known rough edges" in
[`development.md`](development.md).

## Apertures to geometry

`Aperture.geometry()` returns the aperture shape centred on the origin, in mm.
The result is cached in `_geom`, so an aperture used by a thousand flashes is
built once. Circles are approximated with `quad_segs=24` (96 segments on a full
circle).

| template | modifiers | geometry |
| --- | --- | --- |
| `C` | diameter, [hole] | a buffered point |
| `R` | width, height, [hole] | a `box` centred on the origin |
| `O` | width, height, [hole] | an obround: a buffered line segment along the longer axis, or a circle when the two sides are equal |
| `P` | diameter, vertices, [rotation], [hole] | a regular polygon on the circumscribed circle, rotated by the third modifier |
| macro | the macro parameters | the union/difference of its evaluated primitives |

The optional last modifier of `C`, `R`, `O` and `P` is a round hole; when it is
greater than zero it is subtracted at the end.

`Aperture.describe()` produces the short shape text that the TUI and the
`.stencicrity` file show:

| template | text |
| --- | --- |
| `C` | `C ⌀1.00` |
| `R`, `O` | `R 0.70x0.70`, `O 1.60x0.90` |
| `P` | `P ⌀2.00 n=6` |
| macro | falls back to the bounding box of the built geometry: `<name> 1.60x0.60` |

## The macro evaluator

`Macro.evaluate(params)` walks the blocks of an `%AM%` command with
`{1: params[0], 2: params[1], ...}` as the variable table. A block starting
with `0` is a comment, a block of the form `$n=<expression>` assigns a
variable, and anything else is a primitive: the leading integer is the code and
the remaining comma-separated fields are evaluated as expressions.

`_Expr` is a small recursive-descent evaluator. It removes all whitespace, then
parses:

| level | accepts |
| --- | --- |
| expression | `term (('+' \| '-') term)*` |
| term | `factor (('x' \| 'X' \| '/') factor)*` |
| factor | unary `-` or `+`, `( expression )`, `$n`, or a decimal number |

`$n` reads variable `n`, defaulting to `0.0` when it was never set. Text left
over after the expression, or a character that starts none of the above, raises
`GerberError`.

| code | primitive | result |
| --- | --- | --- |
| 1 | circle | one `MacroPrim("circle")`; the centre is rotated by the optional last modifier |
| 2, 20 | vector line | a four-point outline around the segment; a zero-length segment degenerates to a circle of the line width |
| 21 | centre line | a rectangle outline around `(cx, cy)` |
| 4 | outline | the listed points as an outline; a repeated closing point is dropped |
| 5 | polygon | a regular polygon with `n` vertices on the given diameter |
| 7 | thermal | built with shapely (an annulus minus a cross of width `gap`, rotated about the origin) and returned as one outline primitive per resulting polygon |
| 6 | moiré | ignored: it is a decoration, never an opening |
| other | — | raises `GerberError` |

Every primitive is reduced to a `MacroPrim`, which is either a circle
(`d`, `cx`, `cy`) or an outline (a point list), plus an `exposure` flag:

* `geometry()` builds the shapely shape.
* `mirrored()` returns the same primitive with `x -> -x`.
* `as_gerber()` renders it back to macro syntax, as primitive 1 or primitive 4.

`Aperture._build_geometry` composes them in order: an exposure-1 primitive is
unioned into the result, an exposure-0 primitive is subtracted from it, and the
result is cleaned up after every step so boolean leftovers (stray lines and
points) do not accumulate.

## Baking apertures for output

The writer never sees the original macro text. `bake_aperture(ap, mirror)`
resolves an aperture into a `BakedAperture`:

* A macro aperture becomes `BakedAperture("MACRO", [], prims, attrs)`, where
  `prims` are the already evaluated primitives. The writer re-emits them with
  `as_gerber()`, so the generated `%AM%` contains plain numbers and the output
  file carries no macro parameters at all. This also means two apertures that
  used the same macro with different parameters become two different macros in
  the output, and two that used different macros but evaluate identically
  become one.
* A standard aperture keeps its template and modifiers.

Mirroring (bottom sides, see [`layout.md`](layout.md)) is folded into the
aperture rather than into the coordinates:

| template | under mirror |
| --- | --- |
| macro | every primitive is replaced by `MacroPrim.mirrored()` (`x -> -x`) |
| `P` | the rotation modifier becomes `180 - rot`, appended when the aperture had none |
| `C`, `R`, `O` | unchanged: they are symmetric about the Y axis |

`BakedAperture.key()` is the deduplication key used by the writer. It is the
concatenated macro body for a macro, or `template,mods` joined with `X` for a
standard aperture, followed by the attributes sorted by name. Two apertures
with the same shape but different `%TA` attributes stay separate.

## Graphic objects

| class | fields | meaning |
| --- | --- | --- |
| `Segment` | `x0, y0, x1, y1, arc, cx, cy, clockwise` | one straight or circular piece of a path; `points()` discretises it |
| `Flash` | `x, y, aperture, attrs, dark` | one `D03` |
| `Stroke` | `seg, aperture, attrs, dark` | one `D01` outside a region |
| `Region` | `contours, attrs, dark` | one `G36`/`G37` block, as a list of contours of `Segment`s |

`object_geometry(obj)` maps them to shapely:

* A flash is the aperture geometry translated to `(x, y)`.
* A stroke with a round (`C`) aperture is the path buffered by half the
  aperture diameter; a degenerate path becomes a disc.
* A stroke with any other aperture is the convex hull of the aperture placed at
  every discretised path point. That is an approximation: it is right for a
  straight segment and wrong for a path that curves back on itself.
* A region is each contour's points joined end to end (duplicated joints are
  dropped) and the resulting polygons unioned.

Arcs are discretised, never kept as arcs. `ARC_TOLERANCE = 0.004` mm is the
allowed chord error; `_arc_steps` derives the step angle from it, clamps that
angle to `pi/4` and the step count to 4000. `arc_sweep` returns the signed
sweep and treats a zero sweep with coincident end points as a full circle
(`±2*pi`). `arc_points` interpolates the radius linearly from start to end, so
a slightly inconsistent centre does not produce a discontinuity.

In single-quadrant mode (`G74`) the sign of the I/J offsets is not in the file.
`_single_quadrant_centre` tries all four sign combinations, discards the ones
whose sweep exceeds 90 degrees, and keeps the candidate whose start and end
radii differ least.

## The writer

`GerberWriter(file_function)` buffers a body and an aperture list and assembles
the file in `render()`. The header is fixed:

```
%TF.GenerationSoftware,<software>,pcbstencil,<version>*%
%TF.CreationDate,<local ISO timestamp>*%
%TF.FileFunction,<file function>*%
%TF.FilePolarity,<polarity>*%
%FSLAX46Y46*%
G04 Gerber Fmt 4.6, Leading zero omitted, Abs format (unit mm)*
G04 Created by pcbstencil <version>*
%MOMM*%
%LPD*%
G01*
G04 APERTURE LIST*
```

then the macros, then the aperture definitions, then `G04 APERTURE END LIST*`,
then `G75*` when any arc was written, then the body, then `M02*`.

| aspect | behaviour |
| --- | --- |
| coordinates | `_nm(mm) = int(round(mm * 1_000_000))`, matching the 4.6 format in the header |
| apertures | registered lazily on first use, D-codes counting up from 10, deduplicated by `BakedAperture.key()` |
| macros | named `M1`, `M2`, ... and deduplicated by their rendered body, so the same shape is defined once |
| aperture attributes | `%TA.<attr>,<value>*%` lines immediately before the `%ADD%`, and a `%TD*%` immediately after it, only when the aperture has attributes |
| polarity | `%LPD*%` / `%LPC*%` emitted only when it changes |
| interpolation | `G01*` / `G02*` / `G03*` emitted only when it changes |
| moves | `D02` emitted only when the current point differs, except at the start of a region contour, where it is forced |
| arcs | written with I/J relative to the start point; `clockwise = seg.clockwise != bool(tr.mirror)`, because mirroring about the Y axis reverses the direction of travel |
| regions | `G36*`, one forced `D02` plus the segments per contour, `G37*` |

Besides `add_object(obj, transform)` the writer has four primitive helpers used
by the CLI: `add_circle` (the border dots and the dowel holes),
`add_polygon` (a `G36` fill, used for a pad opening shrunk with
`--open-shrink`), `add_line` and `add_rect_outline` (the optional sheet
outline). All four write with dark polarity.

Object attributes (`%TO`) are parsed but deliberately never written back. The
output carries file attributes from the header and aperture attributes copied
from the source apertures; net names, component references and pin numbers do
not end up in the stencil.

## What a generated file looks like

From a batch run over the eight sample gerber zips (see the worked example in
[`layout.md`](layout.md)), the two generated gerbers contain:

| | `stencil-F_Paste.gbr` | `stencil-F_Cu.gbr` |
| --- | --- | --- |
| `%AM` macros | 87 | 95 |
| `%ADD` apertures | 103 | 153 |
| `%TA.AperFunction` lines | 0 | 153 |
| `G36` regions | 0 | 102 |
| `G75` | absent | absent |

The paste layer has no aperture attributes because KiCad does not put
`AperFunction` on paste apertures; the copper reference layer keeps the ones it
found (`SMDPad,CuDef`, `BGAPad,CuDef`, `ComponentPad`, `Conductor`,
`HeatsinkPad`, ...). Neither file has an arc, so neither gets a `G75`. The
first ten lines of that paste file:

```
%TF.GenerationSoftware,pcbstencil,pcbstencil,0.1.0*%
%TF.CreationDate,2026-09-15T04:05:23+03:00*%
%TF.FileFunction,Paste,Top*%
%TF.FilePolarity,Positive*%
%FSLAX46Y46*%
G04 Gerber Fmt 4.6, Leading zero omitted, Abs format (unit mm)*
G04 Created by pcbstencil 0.1.0*
%MOMM*%
%LPD*%
G01*
```

## Known limitations

* Macro aperture modifiers are not scaled by the file unit. `_define_aperture`
  scales the modifiers of `C`/`R`/`O`/`P` but takes macro parameters as they
  are, with the comment that macro parameters are not lengths in every position
  and that KiCad only writes mm. A `MOIN` file that uses macro apertures is
  therefore read wrong.
* No step and repeat, and no aperture or image transformations; see the
  `GerberError` table above.
* The writer always emits mm and the 4.6 format. The `%TF.FileFunction` value
  is whatever the caller passes (`Paste,Top`, `Copper,L1,Top`, `Profile,NP`).
* `GerberWriter.__init__` defaults `version="0.1.0"` and no caller passes
  `pcbstencil.__version__`, so the header of every generated file says 0.1.0
  whatever the installed release is. Listed under "Known rough edges" in
  [`development.md`](development.md).
* `%TF.GenerationSoftware` is written as `<software>,pcbstencil,<version>`
  with `software` also defaulting to `pcbstencil`, so vendor and application
  are the same string.
* A stroke with a non-round aperture is approximated by a convex hull, which is
  exact only for a straight segment.

See [`architecture.md`](architecture.md) for where parsing and writing sit in
the pipeline.
