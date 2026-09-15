# Architecture

Stencicrity reads the gerber exports of several KiCad projects, places every
board side on one stencil sheet of an orderable size, copies the solder-paste
openings over, marks each cell with dotted cut lines and the alignment datum
(by default obround slots on a 20 mm raster along the bottom and left edges of
every cell plus an X inside the datum corner that says which way round the
piece goes, or four dowel-pin holes for the legacy jig), asks the user what to
do with the copper pads the paste layer does not cover, and writes a single
`F_Paste` gerber (plus a copper reference layer, a zip, a preview PNG and a
text report). Everything the user decides is stored in a hidden `.stencicrity`
file next to the gerbers, so the second run over the same boards needs no
interaction.

## The pipeline

`stencicrity.py` is a three-line shim around `pcbstencil.cli.main()`.
`main()` builds the parser, parses the command line, rejects impossible
numbers with `_check_args()` and calls `_run()` inside a `try`. `_run()` is the
whole program, top to bottom:

```
                  cli.main()
                      |
                      v
   1 discover    project.discover_projects()      zips/dirs -> Project/Side
   2 config      config.load_config()             ./.stencicrity -> Config
                 cli.apply_cli_config()           command line wins
   3 pads        pads.detect_pads()               copper flashes -> Pad
                 config.apply_config()            stored states -> objects
                 cli.apply_selection()            --only / --exclude
                 config.save_config()             write the file back
   4 select      Side.is_relevant()               sides worth a cell
   5 pack        layout.pack()                    cells -> sheet (MaxRects)
   6 preview     render.render_preview()          PNG + render.open_file()
   7 TUI         tui.run_tui()                    decide, re-pack, re-render
                 config.save_config()             write the file back again
   8 final pack  layout.pack()                    only sides with openings
   9 write       cli._write_paste/_write_copper/_write_outline
                 cli._zip_files()                 one deflate zip
                 render.render_preview()          the final preview
                 layout.layout_report() + cli._summary()  -> report.txt
```

Step by step, with the functions that do the work:

| # | Step | Calls | Notes |
| --- | --- | --- | --- |
| 1 | Discover | `project.discover_projects(inputs or None, cwd, mirror_bottom=not args.no_mirror_bottom, exclude_dirs=[out_dir], warn=warn)` | With no positional arguments it takes every `*.zip` directly in the working directory plus every immediate subdirectory that holds a classifiable gerber. The output directory and files written by pcbstencil itself are skipped. No projects at all returns exit code 2. |
| 2 | Load the configuration | `config.load_config(source_path, warn=warn)`, then `cli.apply_cli_config(config, args)` | The file has to be read *before* the pads are detected, because `[rules] ignore_prefixes` decides the default state of a pad nobody has decided on yet. A pre-0.1.2 `./<name>.stencil` is read once instead and reported with a `note: migrated …` line. |
| 3 | Detect pads | `pads.detect_pads(side, include_tht=args.include_tht, ignore_prefixes=config.ignore_prefixes)` for every side | Fills `side.pads` and marks which pads the paste layer already covers. |
| 3 | Apply the configuration | `config.apply_config(config, projects)`, `cli.apply_selection(projects, args.only, args.exclude)` | `apply_config` pushes the stored side switches and pad states into the objects; `--only` / `--exclude` run afterwards, so they win over the file. |
| 3 | Save | `config.save_config(config_path, config, projects)` | Written straight away, so an interrupted run still leaves a usable file. |
| 4 | Select the sides | `Side.is_relevant()` | A side is relevant when it has paste objects or candidate pads. None at all returns 2. `cli.side_table()` prints the overview. |
| 5 | First pack | `layout.pack(sides, config)` | Uses every *enabled* relevant side, whether or not it has openings. |
| 6 | First preview | `render.render_preview(layout, preview_path, px_per_mm=args.px_per_mm, title=name)`, then `render.open_file()` unless `--no-open` | Skipped with a warning when every side is switched off. |
| 7 | The TUI | `tui.run_tui(all_pads, sides, config, on_preview=…, compute_layout=…, title=…)` | Skipped with `--batch`. The two callbacks are closures over `sides` and `config` defined in `_run`: `compute_layout(cfg)` is `pack(sides, cfg or config)` and `on_preview(pad, do_open)` re-packs, re-renders and optionally opens the PNG. Afterwards the configuration is saved again; a user who quit instead of generating gets exit code 1. |
| 8 | Final pack | `layout.pack(final_sides, config)` | `final_sides` are the enabled sides that have at least one opening; sides with none are dropped and reported, and if nothing is left the run returns 2. This is a *second* packing over a possibly shorter list, so the final layout can differ from the one in the TUI. |
| 9 | Write | `cli._write_paste`, `cli._write_copper`, `cli._write_outline`, `cli._zip_files`, `render.render_preview`, `layout.layout_report`, `cli._summary` | The paste layer carries the surviving paste objects, the opened pads, the divider dots and the datum: obround slots (`writer.add_obround` over `area.slots`) plus, with `marker` on, the two `dot_dia` strokes of the orientation X (`writer.add_line` over `layout.marker_strokes(area.marker, …)`) for `datum = slots`, round holes (`writer.add_circle` over `area.holes`) for `datum = holes`, nothing for `none`; the report is `layout_report()` followed by the same summary that is printed on stdout. |

With `--batch` the TUI is skipped, undefined pads stay closed, and two
warnings are printed instead: how many pads were left undefined, and whether
the block fits.

## Module map

**`stencicrity.py`** - the executable entry point. Imports `pcbstencil.cli.main`
and exits with its return value. The installed console script
`stencicrity = pcbstencil.cli:main` does the same thing.

**`pcbstencil/model.py`** - the shared data model and the only module the
others all depend on: `Transform`, `Pad`, `Side`, `Project`, `LayoutParams`,
`Config`, `Area`, `Layout`, plus the constants (`STATES`, `STENCIL_SIZES`,
`ORIENTATIONS`, `SORT_ORDERS`, `DEFAULT_IGNORE_PREFIXES`) and three small
helpers (`size_label`, `parse_size`, `side_key`). It imports only `shapely` and
`gerber`. Its module docstring is the reference for the coordinate conventions.
See [data-model.md](data-model.md).

**`pcbstencil/gerber.py`** - a self-contained RS-274X (Gerber X2) reader.
`parse_gerber(text, name) -> GerberFile` is the public entry point; the rest is
apertures (`Aperture`, `Macro`, `MacroPrim`, `bake_aperture`, `BakedAperture`),
graphic objects (`Flash`, `Stroke`, `Region`, `Segment`, `object_geometry`) and
geometry helpers (`polygons_of`, `arc_points`). It depends on shapely only and
raises `GerberError` for anything it cannot represent. See
[gerber.md](gerber.md).

**`pcbstencil/writer.py`** - the matching writer. `GerberWriter(file_function)`
collects objects and primitives, deduplicates apertures by
`BakedAperture.key()`, and `render()` / `write(path)` assemble the file
(header, macro list, aperture list, body, `M02*`). It depends on `gerber` for
baking and on `model.Transform` for the board-to-sheet mapping.

**`pcbstencil/project.py`** - discovery. `load_source()` reads a zip or a
directory into `{name: text}`, `classify_layer()` maps a file to one of the
five roles (`copper_top`, `copper_bottom`, `paste_top`, `paste_bottom`,
`outline`) using the X2 `%TF.FileFunction` header first and KiCad file-name
patterns second, `discover_projects()` builds the `Project` / `Side` objects
and gives each project a unique name (`%TF.ProjectId`, else the source
basename, else a `(2)` suffix). The board bounding box comes from the outline
layer when there is one, otherwise from the union of the copper and paste
bounds. Depends on `gerber` and `model`.

**`pcbstencil/pads.py`** - pad detection and ordering. `detect_pads()` turns
copper flashes into `Pad` objects, decides which of them the paste layer
already covers (shapely `STRtree` plus an area/centroid test) and gives each
one its `default_state()`. Also the natural sort key used everywhere
(`natural_key`), the ordering helpers (`sorted_pads`, `sorted_candidates`) and
the counters (`state_counts`, `closed_count`). Depends on `gerber` and `model`.
See [pads-and-config.md](pads-and-config.md).

**`pcbstencil/config.py`** - the `.stencicrity` file. `load_config()` parses it
(never raising: every problem is a warning and the default is kept),
`apply_config()` / `collect_config()` move state between the file and the
objects, `format_config()` renders the text and `save_config()` collects and
writes it atomically through a temporary file in the same directory. Depends on
`model` and `pads`.

**`pcbstencil/layout.py`** - the packer. `pack(sides, config) -> Layout` orders
the enabled sides, turns each into a cell, places the cells with a MaxRects bin
packer under three heuristics, centres the block, computes the divider lines,
the dots and the datum features (slots or dowel holes, the jig pin centres and,
for the slots datum, the centre of each cell's orientation X).
`marker_strokes()` / `marker_half()` turn that centre into the two crossed
strokes the writer and the renderer draw and into the square they cut, which is
what keeps divider dots out of the X. `layout_report(layout, config)` renders
the text report. Depends on `model` and `pads` (for `natural_key`) - no
shapely.
See [layout.md](layout.md).

**`pcbstencil/render.py`** - the preview. `render_preview(layout, path, …)`
rasterises the sheet with Pillow from the shapely geometry of the gerber
objects; `open_file(path)` hands a file to the desktop viewer. Depends on
Pillow, shapely, `gerber` and `model`. See [render.md](render.md).

**`pcbstencil/tui.py`** - the curses front-end. `run_tui(pads, sides, config,
on_preview=…, compute_layout=…)` returns True when the user asked to generate.
Everything above the `_App` class is pure and testable; `_App` only draws and
dispatches keys. Depends on `curses` and `model`. See [tui.md](tui.md).

**`pcbstencil/cli.py`** - the command line and the pipeline above. It imports
the heavy modules lazily inside `_run()` so that `--help` and `--version` work
even when a dependency is missing.

```
cli ──┬──> project ──> gerber, model
      ├──> config  ──> model, pads
      ├──> pads    ──> gerber, model
      ├──> layout  ──> model, pads
      ├──> render  ──> gerber, model      (Pillow)
      ├──> writer  ──> gerber, model
      └──> tui     ──> model              (curses)

                 model ──> gerber         (shapely)
```

## Two copies of the state

The same decisions exist twice while the program runs:

* **The objects.** `Project` → `Side` → `Pad`. `side.enabled` is the side
  switch, `pad.state` is the open/ignore/undefined decision. This is what the
  packer, the renderer and the writers read.
* **The `Config` dictionaries.** `config.sides` maps `"<project>/<side>"` to a
  bool and `config.pads` maps a pad key to a state string. This is what the
  file contains, and it also holds entries for pads and sides that are *not* in
  the current gerbers.

Two functions reconcile them, and nothing else writes across the boundary:

* `config.apply_config(config, projects)` pushes the dictionaries into the
  objects. A side not in `config.sides` defaults to enabled. A candidate pad
  whose key is missing (or whose stored value is not a known state) falls back
  to `pads.default_state()`; a pad that already has paste may only be `open` or
  `ignore` and defaults to `open`.
* `config.collect_config(config, projects)` pulls the objects back into the
  dictionaries. Every candidate is stored with its state. A pasted pad is
  stored only when it was closed (`= ignore`); when it is open again its key is
  removed, because open is the default. Keys that match nothing in the current
  projects are left untouched, which is how hand-made decisions survive a
  temporarily missing board - `format_config()` writes them out under a
  "not found in the current gerbers" header.

`save_config()` is `collect_config()` plus an atomic write, so saving is always
the last word of the objects. The TUI deliberately mutates only the objects and
the scalar parts of the config (`size`, `orientation`, `layout.*`) and never
touches `config.sides` / `config.pads`; `_run()` collects them afterwards.

## Error handling and exit codes

`_run()` raises nothing of its own: user-level problems are either a printed
error with a return code or a warning. `main()` wraps it in

```python
except (GerberError, OSError) as exc:   # -> 2
except KeyboardInterrupt:               # -> 1
```

| Code | When |
| --- | --- |
| 0 | The stencil was generated. |
| 1 | The user left the TUI with `q` (the configuration is still saved), or Ctrl-C anywhere. |
| 2 | No gerber projects found; no side has paste openings or pads to decide; no enabled side has a single opening; any `GerberError` or `OSError`; and, from argparse itself, a bad command line or a value rejected by `_check_args()`. |

Warnings go to stderr through the local `warn()` helper, which flushes stdout
first so the two streams stay in order. They never stop the run: an unreadable
source, a layer that fails to parse, a duplicate layer role, an unknown key in
the configuration file, a bad number in it, a block that does not fit, pads left
undefined in `--batch` - all of these are warnings. A gerber file that fails to
parse is dropped from its project; a project that ends up with neither copper
nor paste is skipped.

Inside the TUI, exceptions from `compute_layout` and `on_preview` are caught and
shown in the status line, so a broken preview never leaves the user stuck in
curses.

## Performance

Measured on the eight sample gerber zips in `comanda-stencil` (8 projects, 13
relevant sides, ~4700 gerber objects), Python 3.14, shapely 2:

| Stage | Time | Notes |
| --- | --- | --- |
| Discovery: read 76 archive members, classify them, parse the 40 gerber layers, compute the board bounding boxes | 0.08 s | Dominated by `GerberFile.bounds()`, which builds the geometry of every object. |
| `detect_pads()` over all 13 sides | 0.19 s | One `STRtree` per side plus one intersection per pad/opening pair. |
| `layout.pack()` over 13 sides | 2.7 ms with `datum = holes` and `hole_grid = 8`, 1.5 ms with the grid off | Three heuristics are run over the full cell list each time. |
| `render_preview()` at 20 px/mm | 1.1 s | Produces a 7834 x 5859 px PNG of about 1.9 MB. |
| The whole `--batch --no-open` run | 2.5 s | Two renders (first preview and final) are two thirds of it. |

The practical consequences: packing is cheap enough to redo on every keystroke
that changes something (the TUI does), and filling the Stencil page's fit
column costs 16 packs (8 sizes x 2 orientations), still a few tens of
milliseconds. Rendering is not cheap, which is why the preview is only redrawn
on `p` or `v` and why the status line says `rendering…` first.
