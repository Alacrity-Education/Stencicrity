# The preview renderer

`pcbstencil/render.py` turns a [`Layout`](data-model.md) into a PNG.
`render_preview(layout, path, *, px_per_mm=20.0, max_px=12000, selected=None,
title="")` rasterises the whole sheet with Pillow straight from the shapely
geometry of the gerber objects and returns `path`. It is meant to be looked at,
not to be exact: shapes are filled into 8-bit masks with no anti-aliasing, a
sub-pixel shape is kept as one pixel so it does not disappear, and a geometry
that fails to build is skipped rather than raising. A layout that does not fit
is drawn anyway — the image grows to cover the cells parked outside the sheet
and the legend says `DOES NOT FIT`.

The module has one other public function, `open_file(path)`, which hands the
file to the desktop viewer.

## Pipeline

`render_preview` runs in this order:

1. **Content size.** `_content_size(layout)` returns
   `(max(layout.width, layout.block[2], 1.0), max(layout.height, layout.block[3], 1.0))`,
   so overflow cells parked to the right of the sheet are inside the image.
2. **Frame.** `_frame_for(content_w, content_h, px_per_mm, max_px)` picks the
   scale and the band sizes (see below). Everything after this uses
   `frame.to_px`. Several line widths are derived from
   `scale = ppmm / _REFERENCE_PPMM`: `thin` (1 px at 20 px/mm), `outline_w`
   (min 2), `select_w` (min 3), and the dash/space lengths of the guides.
3. **Pin grid.** `_draw_hole_grid` paints one hairline per `hole_grid`
   multiple, under everything else.
4. **Divider guides.** A faint dashed line (`COLOR_GUIDE`) under every entry of
   `layout.dividers` — one per cell edge, `dot_line_gap / 2` inside it, so two
   touching cells show two of them with the cut between. The dots themselves
   are the real marking; no cell rectangles are drawn.
5. **Class masks.** Five `_ClassMask` instances are filled per area, using
   `area.transform.affine()` as the board-to-sheet matrix: copper objects into
   `copper`, every `side.pads` geometry into `pads`,
   `side.active_paste_objects` into `openings`, `side.closed_paste_objects`
   into `ignored`, and each candidate into `openings` / `ignored` / `undefined`
   according to its state.
6. **Dots and the datum openings.** `layout.dots` and every `area.holes` centre
   are added to the `openings` mask as ellipses, and every `area.slots` entry
   as an obround; they are already in sheet coordinates, so only `to_px` is
   applied (the slots go through `_fill_geometry` with the identity matrix).
   The ellipse radius is clamped to at least 1 px. `_obround(cx, cy, w, h)`
   builds the stadium polygon — a segment of length `|w - h|` buffered by
   `min(w, h)/2`, a plain circle when the two are equal — which is exactly the
   shape of the `O` aperture the writer flashes for that slot.
6a. **The jig.** `_draw_datum` draws, on top of the openings, one dashed blue
   ghost circle (`COLOR_PIN`) per entry of `area.pins` — where the fixture pin
   comes up through the foil, through a slot or through a dowel hole, for
   either datum, `pin_dia` wide for `slots` and `hole_dia` for `holes`. Under
   the `slots` datum each cell additionally gets an orange `COLOR_DATUM`
   bracket at its `datum_corner` (two legs of at most `_DATUM_LEG_MM` = 4 mm,
   shortened on a small cell) and a short green `COLOR_NEST` arrow running down
   the diagonal toward it, showing which way the piece is pushed. The arrow is
   skipped when the padding leaves it less than 3 mm of run.
7. **Composite copper, then pads.**
8. **Sheet rectangle and board outlines.** The stencil boundary is a rectangle
   from `(0, 0)` to `(width, height)` in `COLOR_SHEET`. Each area then gets its
   real board outline from `_outline_paths` (strokes and region contours of the
   project's `Edge_Cuts` layer, transformed to the sheet); when the project has
   no outline layer the `area.board_rect` rectangle is drawn instead.
9. **Composite ignored, openings, undefined** — in that order, so an opening
   wins over a blue pad and a yellow undefined pad wins over both.
10. **Component boxes.** `_pad_groups(area)` groups the *undefined* candidates
    of the area by reference; each group gets a yellow rectangle around its
    bounds (grown by 0.4 mm) labelled with the reference.
11. **Selected pad.** When `selected` is given, `_area_of` finds its area by
    project and side name, and the pad gets a cyan box (grown by 0.6 mm) with
    its `REF.pin` drawn to the right.
12. **Cell labels.** One per area, inside the cell.
13. **Rulers.** `_draw_rulers` paints the two bands over the content and draws
    the ticks and labels.
14. **Legend.** One fitted line at the bottom, with the `DOES NOT FIT` tail in
    the opening red when `layout.fits` is false.
15. `img.save(path, "PNG", compress_level=1)` — fast compression, large files.

## `_Frame`: pixel geometry

`_Frame` holds the pixel geometry of one image. Everything is derived from
`ppmm` and the content size:

| attribute | derivation |
| --- | --- |
| `ppmm` | pixels per millimetre, as chosen by `_frame_for` |
| `legend` | legend band height, passed in from `_legend_band` |
| `base` | nominal band thickness, `max(_RULER_MIN_PX, round(_RULER_MM * ppmm))` = `max(28, round(6 * ppmm))` |
| `major`, `medium`, `minor` | tick lengths, `base * 0.60 / 0.40 / 0.20` |
| `font_px` | ruler label size, `max(9, round(0.55 * base))` |
| `label_w` | width of the widest label that can occur, `ceil(max(content_w, content_h, 1))` rendered in that font |
| `left` | left band, `max(base, ceil(major + label_w + 0.30 * base))` — grown so the label fits next to the tick |
| `bottom` | bottom band, `max(base, ceil(major + 1.25 * font_px + 4))` |
| `width_px` | `left + max(1, ceil(content_w * ppmm))` |
| `height_px` | `max(1, ceil(content_h * ppmm)) + bottom + legend` |
| `origin_y` | image row of sheet `y = 0`, `height_px - bottom - legend` |
| `longest` | `max(width_px, height_px)`, the value compared against `max_px` |

`to_px(x, y)` returns `(left + x * ppmm, origin_y - y * ppmm)` — the Y term is
subtracted, which is the flip described below.

`label_step(span, vertical)` returns the millimetre distance between two
labelled ticks. It needs `1.8 * font_px` of room for a vertical ruler (labels
stacked) or `label_w + 0.8 * font_px` for a horizontal one, and returns the
first entry of `_LABEL_STEPS = (10, 20, 50, 100, 200, 500, 1000)` that is at
least that wide at the current scale.

`_legend_band(ppmm, max_px)` is `max(_BAND_PX, round(_LEGEND_MM * ppmm))` —
at least 48 px, nominally 5 mm — clamped to a third of `max_px`.

### Choosing the scale

The bands grow with the scale and the scale is limited by the bands, so
`_frame_for` does not shrink once: it iterates to a fixed point. Up to 12 times
it recomputes `ppmm = min(px_per_mm, (max_px - bands_w) / content_w,
(max_px - bands_h) / content_h)` and rebuilds the frame, stopping when the
value stops moving. The integer `ceil` in `width_px` / `height_px` can still
push the image one pixel over the cap, so a second loop multiplies `ppmm` by
0.999 up to 24 times until `frame.longest <= max_px`.

Two measured examples:

| sheet | asked | result |
| --- | --- | --- |
| 380 x 280 mm | 20 px/mm | ppmm stays 20.0, image 7834 x 5859 px (left band 234, bottom 159, legend 100) |
| 700 x 600 mm | 20 px/mm | ppmm reduced to 16.86, image 12000 x 10335 px |

The preview of the eight-project sample run is the first of those: 7834 x 5859
px, about 1.9 MB on disk. `px_per_mm` is exposed as `--px-per-mm`; `max_px` is
not — it is a keyword argument with a default of 12000 that no caller passes.

## The y flip and the three coordinate systems

| system | origin | units | used by |
| --- | --- | --- | --- |
| board | wherever KiCad put it | mm | the parsed gerber objects and `Pad.x` / `Pad.y` |
| sheet | bottom left of the stencil, X right, Y up | mm | `Layout`, `Area`, the report, the rulers |
| image | top left, X right, Y **down** | px | Pillow |

Board to sheet is `area.transform.affine()`, a shapely affine matrix
(`[-1|1, 0, 0, 1, dx, dy]`) applied with `affine_transform` before rasterising.
Sheet to image is `frame.to_px`. The flip lives only in `to_px`: image rows
grow downwards while sheet millimetres grow upwards, so the Y term is
subtracted from `origin_y`. Nothing else in the renderer inverts an axis, and
the ruler numbers therefore read the same as the coordinates in
`stencil-report.txt`.

## Class masks and colours

A `_ClassMask` is an `"L"` image the size of the whole picture plus a tracked
dirty bounding box. Shapes are drawn into it at full image coordinates, but
`composite(img, color)` only pastes the colour over the box that was actually
touched, which keeps the cost proportional to the drawn area rather than to the
sheet.

- `polygon` / `ellipse` draw straight into the mask; `ellipse` also calls
  `touch`.
- `merge_crop(box, tmp, erase)` merges a small temporary mask with
  `ImageChops.lighter` (or `subtract` when erasing).
- `_fill_polygon` transforms the exterior ring to pixels, records the dirty
  box, and: keeps a shape smaller than one pixel in both directions as a single
  pixel; fills the ring directly when the polygon has no interiors; otherwise
  builds a cropped temporary mask, punches the interior rings out of it with
  fill 0 and merges that.
- `_fill_geometry` applies the affine matrix and flattens the result with
  `polygons_of`, so multipolygons and collections work.
- `_fill_objects` calls `object_geometry` on each gerber object inside a bare
  `except Exception: continue` — a preview never fails because of one bad
  object — and passes value `0` for a clear-polarity object, which subtracts it
  from the mask.

The five masks, in the order they are composited:

| mask | colour | what goes in |
| --- | --- | --- |
| `copper` | `COLOR_COPPER` `#2e2e2e` | every object of the copper layer |
| `pads` | `COLOR_PAD` `#5a5a5a` | the geometry of every detected pad, pasted ones included |
| `ignored` | `COLOR_IGNORE` `#3a5fcd` | closed paste openings and candidates set to `ignore` |
| `openings` | `COLOR_OPEN` `#ff2a2a` | everything that is cut, alignment slots and dowel holes included |
| `undefined` | `COLOR_UNDEFINED` `#ffd000` | candidates still `undefined` |

The `openings` mask is the important one: it holds the active paste objects,
the pads the user opened, every divider dot and every datum opening — exactly the
set of shapes that ends up in `stencil-F_Paste.gbr`. Paste openings the user
closed are not simply left out; they are drawn in the `ignored` blue, so the
change stays visible against the copper underneath.

The remaining colours are drawn as lines or text rather than through masks:

| constant | value | use |
| --- | --- | --- |
| `BACKGROUND` | `#141414` | the image ground |
| `COLOR_SHEET` | `#8a8a8a` | the stencil boundary rectangle |
| `COLOR_GUIDE` | `#333333` | dashed guide under every divider line |
| `COLOR_GRID` | `#202020` | the jig pin grid hairlines |
| `COLOR_PIN` | `#4aa3ff` | dashed ghost circle of a jig pin (`_dashed_circle`: 22° dashes, a plain ring below 3 px radius) |
| `COLOR_DATUM` | `#ff9a1f` | the bracket at the corner the piece is pushed into (`slots`) |
| `COLOR_NEST` | `#39d98a` | the arrow showing the nesting direction (`slots`) |
| `COLOR_OUTLINE` | `#c8c8c8` | board outlines |
| `COLOR_SELECTED` | `#00e5ff` | the pad under the TUI cursor |
| `COLOR_LABEL` | `#dddddd` | cell labels and the legend |
| `COLOR_RULER_BG` | `#1c1c1c` | the two ruler bands |
| `COLOR_RULER` | `#b0b0b0` | ticks and ruler numbers |

## Labels and fitted fonts

`_font(size)` is `lru_cache`d by pixel size and tries `DejaVuSans.ttf` by name
and then two absolute paths before falling back to Pillow's built-in font.
`_text_width(font, text)` uses `font.getlength`.

`_fitted_font(text, size, available, minimum)` returns the largest face at or
below `size` whose rendering of `text` stays under `available` pixels, scaling
the requested size by `available / length` and never going below `minimum`.
`_fit_label(text, size, available, minimum=11)` goes one step further: if the
text still does not fit at the smallest allowed size it drops characters from
the end and appends `…`, and it returns an empty string when there is no room
at all.

Where they are used:

- **Cell label** — `"<project> · <side>"` plus `" (mirrored)"`, drawn inside
  the cell 1 mm below its top edge, starting `left_span + 1.0` mm from the left
  cell edge and stopping `right_span + 1.0` mm before the right one, so it
  never runs into a datum feature. The two spans depend on the datum: for
  `holes` both are `hole_offset + hole_dia/2` (clear of the two top holes), for
  `slots` `left_span = slot_inner` and `right_span = 0` (the bottom slots are
  low and the left slot is at mid height, so only the left edge is in the way),
  and for `none` both are 0.
- **Component box** — the reference, drawn with `ref_font`
  (`max(11, round(1.1 * ppmm))`) above the top-left corner of the box.
- **Selected pad** — `pad.label` (`REF.pin`) in the same font, to the right of
  the cyan box, vertically centred on it.
- **Legend** — one line with the title prefix, sheet size, block size, cell
  count and the colour key, fitted to the image width minus the margins. The
  `   —  DOES NOT FIT` tail is measured separately and drawn in `COLOR_OPEN`
  right after the fitted legend text.

## Rulers and the pin grid

`_draw_rulers` paints the two band rectangles over the already drawn content
(leaving the row and column of the stencil boundary itself untouched), then
walks whole millimetres from 0 to the content size. A tick is major every
10 mm, medium every 5 mm and minor every 1 mm, and the minor ticks are skipped
entirely below `_MINOR_PPMM` = 8 px/mm, where they would be noise. Labels are
drawn only on multiples of `label_step`, clamped so they stay inside the image,
and skipped when they would come within `0.35 * font_px` of the previous one.
The bottom ruler carries X, the left ruler Y, and the Y numbers grow upwards.

`_draw_hole_grid` draws one hairline per `hole_grid` multiple across the sheet,
measured from the sheet origin exactly like the jig pins, so every blue ghost
circle has to sit on a crossing — it is the visual check for the grid, and it
works the same for both datums. It returns immediately when `hole_grid <= 0` or
when the lines would be closer than two pixels.

## `open_file`

```python
open_file(path) -> bool
```

`os.startfile` on Windows, `open` on macOS, `xdg-open` everywhere else. The
viewer is started with `subprocess.Popen(..., start_new_session=True)` and all
three standard streams on `DEVNULL`, so it is detached and never blocks the
caller or scribbles on the terminal the TUI is using. Every exception is
swallowed and the function returns `False`; a machine without a viewer still
gets the PNG on disk.

The CLI calls it for the first preview unless `--no-open` was given; the TUI
calls it for `v` but not for `p` (see [tui.md](tui.md)).

## Performance

Rendering the 12-cell sample layout at 20 px/mm takes about 1.1 s on this
machine, and it is the slowest step of a run by a wide margin — packing the
same layout takes single-digit milliseconds (see [layout.md](layout.md)). A
normal CLI run renders twice, once for the first preview and once for the final
layout, plus once for every `p` or `v` pressed in the TUI.

The cost is dominated by rasterising the geometry: the eight sample projects
contribute roughly 4700 gerber objects, each of which is turned into shapely
geometry, transformed and filled into a mask. `--px-per-mm` is the knob —
halving it quarters the pixel work — and the masks themselves scale with the
image area, not with the number of objects.

See [architecture.md](architecture.md) for where rendering sits in the
pipeline and [data-model.md](data-model.md) for the objects the renderer reads.

## Example

`figures/example-preview.png` is the preview rendered from the eight example
boards with default settings (380 x 280 sheet, slots datum, 20 px/mm).
