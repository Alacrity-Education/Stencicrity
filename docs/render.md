# The preview renderer

`src/render.rs` turns a [`Layout`](data-model.md) into a PNG. It is meant to
be looked at, not to be exact: it rasterises straight from the `geo` geometry
of the gerber objects with `tiny-skia`, so an edge is anti-aliased and a
0.5 mm dot at 20 px/mm is ten pixels across.

```rust
render_preview(&projects, &layout, path, &RenderOptions {
    px_per_mm: 20.0,          // reduced so the longer side stays within max_px
    max_px: 12000,            // no command line switch
    selected: None,           // Option<(SideId, usize)> — drawn in cyan
    title: "stencil".into(),  // prefixed to the legend line
})
```

`open_file(path)` spawns `xdg-open` detached, with all three standard streams
on `/dev/null`, and returns false when it could not be started — the CLI
prints the path either way.

## Pipeline

`render_preview` draws in this order; each colour class is accumulated in one
mask and composited before the next begins, so a later class covers an earlier
one exactly as the class order says.

| # | What | Colour |
| --- | --- | --- |
| 0 | the jig pin raster over the whole sheet, when the datum rides on one | `COLOR_GRID` `#202020` |
| 1 | a faint dashed guide under every divider line | `COLOR_GUIDE` `#333333` |
| 2 | all copper objects of every cell | `COLOR_COPPER` `#2e2e2e` |
| 2 | every pad's copper shape on top of it | `COLOR_PAD` `#5a5a5a` |
| 3 | the stencil boundary, then every board outline | `COLOR_SHEET` `#8a8a8a`, `COLOR_OUTLINE` `#c8c8c8` |
| 4 | closed paste openings and ignored candidates | `COLOR_IGNORE` `#3a5fcd` |
| 5 | everything that becomes an opening: active paste, opened pads, divider dots, dowel holes, alignment slots, the orientation X | `COLOR_OPEN` `#ff2a2a` |
| 6 | candidates still `undefined` | `COLOR_UNDEFINED` `#ffd000` |
| 6a | the datum: a dashed ghost circle per jig pin, an orange bracket at each datum corner with a green nesting arrow | `COLOR_PIN` `#4aa3ff`, `COLOR_DATUM` `#ff9a1f`, `COLOR_NEST` `#39d98a` |
| 6b | a labelled box around every component that still has undefined pads | `COLOR_UNDEFINED` |
| 7 | the selected pad, boxed and labelled | `COLOR_SELECTED` `#00e5ff` |
| 8 | one label per cell, inside it, right of the top-left datum feature | `COLOR_LABEL` `#dddddd` |
| 9 | the rulers over their bands, then the legend | `COLOR_RULER` `#b0b0b0`, `COLOR_LABEL` |

The background is `#141414`. Step 4 before step 5 is what makes a closed
opening visible: the blue is painted first and the surviving red is painted
over it, so the change stands out instead of disappearing.

The orientation X is drawn as what it is — an opening — in the `dot_dia`
stroke width it is cut with, using the same `layout::marker_strokes` the
writer uses.

Cells that overflowed the sheet are drawn outside the stencil boundary:
`content_size(layout)` is the union of the sheet and the block, so the image
simply grows to cover them.

## `Frame`: pixel geometry

`Frame` holds everything that maps millimetres to pixels and reserves the
bands:

| Field | Meaning |
| --- | --- |
| `ppmm` | the scale actually used |
| `content_w`, `content_h` | sheet (or sheet ∪ block) size, mm |
| `left`, `bottom` | ruler band thickness, px |
| `legend` | legend band height, px |
| `major`, `medium`, `minor` | tick lengths, px |
| `font_px`, `label_w` | ruler label size and the widest label |
| `width_px`, `height_px` | the image |
| `origin_y` | the image row of sheet y = 0 |

`to_px(x, y)` is `(left + x·ppmm, origin_y − y·ppmm)` — the Y flip, because
image rows grow downwards while sheet coordinates grow upwards.

The bands scale with the drawing rather than being fixed pixel counts: the
ruler base is `max(RULER_MIN_PX 28, round(RULER_MM 6 · ppmm))`, the legend band
`max(BAND_PX 48, round(LEGEND_MM 5 · ppmm))` capped at a third of `max_px`,
and both grow further when the labels would not fit beside the ticks.

### Choosing the scale

The bands grow with the scale and the scale is limited by the bands, so
`frame_for` iterates instead of shrinking once: up to 12 rounds of "how large
may `ppmm` be so that `content · ppmm + bands ≤ max_px`", until the value
stops moving. Integer rounding can still push the image one pixel over the
cap, so up to 24 further rounds multiply by 0.999 until it is inside. The
requested `px_per_mm` is never exceeded, only reduced.

At the default 20 px/mm a 420 x 320 mm sheet comes out 8634 x 6659 px;
asking for 40 px/mm on a 380 mm sheet gives 12000 x 8972, not 15200 — the cap,
not the request.

## Class masks

The expensive resource is the bitmap, so there is exactly **one** 8-bit
`ClassMask` for the whole render, zeroed and reused for each colour class,
plus a second mask of the same size allocated lazily and only when a gerber
file actually uses clear polarity.

| Operation | What it does |
| --- | --- |
| `fill_geom` / `fill_objects` | fill a geometry (or an object's, under a `Transform`) into the mask; a clear object subtracts through the scratch mask |
| `ellipse`, `fill_sheet_ring` | the dots, holes, slots and marker strokes, already in sheet coordinates |
| `composite(&mut pm, colour)` | blend the mask into the pixmap in that colour |
| `reset()` | zero the dirty rows and forget them |

Shapes are accumulated at full image size, but the dirty region is tracked, so
`composite` and `reset` only touch rows that were actually drawn. Subtracting
a clear shape is bounded by the path's own bounds, and the scratch mask is
wiped over just that box afterwards.

At the 12000 px cap the pixmap is 432 MB and the mask 108 MB; at the default
380 x 280 mm sheet it is 185 MB and 46 MB. Measured peak RSS for a whole run
over the eight example boards is 839 MB and 367 MB respectively — see
[benchmarks.md](benchmarks.md).

## Labels and fonts

Text is drawn with `ab_glyph` from **DejaVu Sans**, embedded at compile time:

```rust
const FONT_DATA: &[u8] = include_bytes!("../assets/DejaVuSans.ttf");
```

`assets/DejaVuSans-LICENSE.txt` carries its Bitstream Vera / Arev licence and
ships with the packages. Embedding it is what makes the binary independent of
the host's fonts — the preview looks the same on a machine with no fontconfig
at all.

`Face` keeps the size convention of the Pillow renderer this module was ported
from — the nominal size is the em square in pixels, and `width` is the
equivalent of Pillow's `getlength`, kerning included — so every pixel-sized
constant here still means what it meant when it was chosen. It draws a string
at a `(HAnchor, VAnchor)` anchor pair — left / middle / right, and ascender /
middle / descender. Glyphs are outlined by `ab_glyph` and blended per coverage
value, so text is anti-aliased like everything else.

Two helpers keep text inside its box: `fitted_size` scales the pixel size down
until the string fits a width, not below a floor, and `fit_label` additionally
truncates with an ellipsis when even the floor (11 px) is too wide. The cell
labels use `fit_label` against the room between the cell's left datum feature
and its right edge; the legend uses `fitted_size` against the image width.

## Rulers and the pin raster

`draw_rulers` paints the two bands last, over whatever spilled into them,
leaving the row and column of the stencil boundary itself untouched. Ticks are
at 1 mm (only from `MINOR_PPMM` 8 px/mm up), 5 mm and 10 mm, with lengths
`TICK_MINOR`/`TICK_MEDIUM`/`TICK_MAJOR` (0.20/0.40/0.60) of the nominal band.
`Frame::label_step` picks the labelled interval from `LABEL_STEPS`
(10, 20, 50, … 1000 mm) — the first one whose pixel spacing leaves room for
the label — and a label that would still come within 0.35 of a font size of
its predecessor is skipped, so the numbers never touch at any scale.

`draw_pin_raster` draws the datum's raster (`slot_pitch` for `slots`,
`hole_grid` for `holes`, nothing for `none`) across the whole sheet in
`COLOR_GRID`, under everything else, so the jig pins can be seen sitting on
its intersections.

The legend line under the image is
`<title>:  stencil W x H mm   block W x H mm   N cell(s)` followed by
`LEGEND_KEY`, the fixed colour key, and `—  DOES NOT FIT` in red when the
block does not fit.

## The terminal preview

`src/ascii.rs` renders the same picture into a terminal for the TUI's split
view: a window of the sheet in square sub-pixels, two per character row, drawn
with `▀`/`▄`/`█`. It reuses this module's class order and `class_color`
returns the same RGB values, so the two pictures agree. It does not reuse the
code: the PNG fills paths, the terminal samples points. See
[tui.md](tui.md#the-split-view).

## Known limitations

* `max_px` has no command line switch; only `--px-per-mm` is exposed, and it
  is an upper bound rather than a guarantee.
* Clear polarity is handled per colour class, not globally: a clear object on
  the copper layer does not erase a paste opening drawn in a later class.
* The image is a single pixmap in memory. A 700 x 600 mm sheet at 20 px/mm is
  inside the cap, but raising `--px-per-mm` on a large sheet is what makes the
  tool's memory peak, and the cap is the only thing bounding it.
