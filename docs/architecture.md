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

It is one binary crate, `stencicrity`, with a library target (`src/lib.rs`) the
binary, the tests and the `bench` example all use. `src/main.rs` is three lines:
it calls `cli::main(std::env::args().collect())` and exits with the code it
returns.

## The pipeline

`cli::main` parses the command line with clap, rejects impossible numbers with
`check_args` and calls `run`, mapping an `anyhow::Error` onto exit code 2.
`run` is the whole program, top to bottom; its seven numbered comments are
these:

```
                 cli::main()
                     |
                     v
  1 discover    project::discover_projects()   zips/dirs -> Project/Side
  2 config      config::load_config()          ./.stencicrity -> Config
                cli::apply_cli_config()        command line wins
                pads::detect_pads()            copper flashes -> Pad
                config::apply_config()         stored states -> objects
                cli::apply_selection()         --only / --exclude
                config::save_config()          write the file back
  3 select      Side::is_relevant()            sides worth a cell
  4 preview     layout::pack()                 cells -> sheet (MaxRects)
                render::render_preview()       PNG + render::open_file()
  5 counts      pads::sorted_pads()            candidates, closed openings
  6 TUI         tui::run_tui()                 decide, re-pack, re-render
                config::save_config()          write the file back again
  7 generate    layout::pack()                 only sides with openings
                cli::write_paste/copper/outline
                cli::zip_files()               one deflate zip
                render::render_preview()       the final preview
                layout::layout_report() + cli::summary()  -> report.txt
```

Step by step, with the functions that do the work:

| # | Step | Calls | Notes |
| --- | --- | --- | --- |
| 1 | Discover | `discover_projects(&args.inputs, cwd, &DiscoverOptions { mirror_bottom: !args.no_mirror_bottom, exclude_dirs: vec![out_dir] }, &mut warn)` | With no positional arguments it takes every `*.zip` directly in the working directory plus every immediate subdirectory that holds a classifiable gerber. The output directory and files stencicrity itself wrote are skipped. No projects at all returns exit code 2. |
| 2 | Load the configuration | `load_config(&source_path, &mut warn)`, then `apply_cli_config(&mut config, args)` | The file has to be read *before* the pads are detected, because `[rules] ignore_prefixes` decides the default state of a pad nobody has decided on yet. A pre-0.1.2 `./<name>.stencil` is read once instead and reported with a `note: migrated …` line. |
| 2 | Detect pads | `detect_pads(side, &DetectOptions { include_tht, ignore_prefixes, .. })` for every side | Fills `side.pads` and marks which pads the paste layer already covers. |
| 2 | Apply the configuration | `apply_config(&config, &mut projects)`, `apply_selection(&mut projects, &args.only, &args.exclude)` | `apply_config` pushes the stored side switches and pad states into the objects; `--only` / `--exclude` run afterwards, so they win over the file. |
| 2 | Save | `save_config(&config_path, &mut config, &projects)` | Written straight away, so an interrupted run still leaves a usable file. |
| 3 | Select the sides | `Side::is_relevant()` | A side is relevant when it has paste objects or candidate pads. None at all returns 2. `cli::side_table` prints the overview. |
| 4 | First pack and preview | `pack(&projects, &sides, &config)`, `render_preview(…)`, then `open_file()` unless `--no-open` | Uses every *enabled* relevant side, whether or not it has openings. The render is skipped with a warning when every side is switched off. |
| 5 | Count | `sorted_pads(&projects, &sides, true)`, `state_counts`, `closed_count` | Prints how many candidate pads there are, split by state, and how many paste openings are closed. The same `Vec<PadId>` is what the TUI lists. |
| 6 | The TUI | `run_tui(&mut projects, &sides, &mut config, hooks, &title)` | Skipped with `--batch`. `TuiHooks` carries two `&mut dyn FnMut` closures defined in `run`: `compute_layout` is `pack(live, sides, cfg)` and `on_preview` re-packs, re-renders and opens the PNG. Afterwards the configuration is saved again; a user who quit instead of generating gets exit code 1. |
| 7 | Final pack | `pack(&projects, &final_sides, &config)` | `final_sides` are the enabled sides that have at least one opening; sides with none are dropped and reported, and if nothing is left the run returns 2. This is a *second* packing over a possibly shorter list, so the final layout can differ from the one in the TUI. |
| 7 | Write | `write_paste`, `write_copper`, `write_outline`, `zip_files`, `render_preview`, `layout_report`, `summary` | The paste layer carries the surviving paste objects, the opened pads, the divider dots and the datum: obround slots (`GerberWriter::add_obround` over `area.slots`) plus, with `marker` on, the two `dot_dia` strokes of the orientation X (`add_line` over `layout::marker_strokes`) for `datum = slots`, round holes (`add_circle` over `area.holes`) for `datum = holes`, nothing for `none`; the report is `layout_report()` followed by the same summary that is printed on stdout. |

With `--batch` the TUI is skipped, undefined pads stay closed, and two
warnings are printed instead: how many pads were left undefined, and whether
the block fits.

## Module map

**`main.rs`** - the executable entry point; exits with `cli::main`'s code.

**`lib.rs`** - the public library: `pub mod` for every module below plus
`VERSION` (`env!("CARGO_PKG_VERSION")`). Everything the tests and the `bench`
example reach for goes through it.

**`model.rs`** - the shared data model and the only module the others all
depend on: `Geom`, `Bounds`, `SideId`, `Transform`, `Pad`, `Side`, `Project`,
`LayoutParams`, `Config`, `Area`, `Layout`, the constants (`STATES`,
`STENCIL_SIZES`, `ORIENTATIONS`, `SORT_ORDERS`, `DATUM_MODES`,
`DEFAULT_IGNORE_PREFIXES`) and three small helpers (`size_label`, `parse_size`,
`side_key`). It imports `geo` and `gerber` and nothing else. Its module comment
is the reference for the coordinate conventions. See
[data-model.md](data-model.md).

**`gerber.rs`** - a self-contained RS-274X (Gerber X2) reader.
`parse_gerber(text, name) -> Result<GerberFile, GerberError>` is the public
entry point; the rest is apertures (`Aperture`, `Macro`, `MacroPrim`,
`bake_aperture`, `BakedAperture`), graphic objects (`Flash`, `Stroke`,
`Region`, `Segment`, `object_geometry`) and geometry helpers (`arc_points`,
`contour_points`, `geom_bounds`). It depends on `geo` and `i_overlay` only.
See [gerber.md](gerber.md).

**`writer.rs`** - the matching writer. `GerberWriter::new(file_function)`
collects objects and primitives, deduplicates apertures by
`BakedAperture::key()`, and `render()` / `write(path)` assemble the file
(header, macro list, aperture list, body, `M02*`). It depends on `gerber` for
baking and on `model::Transform` for the board-to-sheet mapping.

**`project.rs`** - discovery. `load_source()` reads a zip or a directory into
`Vec<(name, text)>`, `classify_layer()` maps a file to one of the five roles
(`copper_top`, `copper_bottom`, `paste_top`, `paste_bottom`, `outline`) using
the X2 `%TF.FileFunction` header first and KiCad file-name patterns second,
`discover_projects()` builds the `Project` / `Side` objects and gives each
project a unique name (`%TF.ProjectId`, else the source basename, else a `(2)`
suffix). The board bounding box comes from the outline layer when there is one,
otherwise from the union of the copper and paste bounds. `is_own_output()`
keeps a previous run's gerbers out of the inputs.

**`pads.rs`** - pad detection and ordering. `detect_pads()` turns copper
flashes into `Pad` objects, decides which of them the paste layer already
covers (an `rstar::RTree` over the paste envelopes plus an area/centroid test)
and gives each one its `default_state()`. Also the ordering helper
`sorted_pads` and the counters `state_counts` / `closed_count`. See
[pads-and-config.md](pads-and-config.md).

**`config.rs`** - the `.stencicrity` file. `load_config()` parses it (never
failing on content: every problem is a warning and the default is kept),
`apply_config()` / `collect_config()` move state between the file and the
objects, `format_config()` renders the text and `save_config()` collects and
writes it atomically through a temporary file in the same directory.

**`layout.rs`** - the packer. `pack(&projects, &sides, &config) -> Layout`
orders the enabled sides, turns each into a cell, places the cells with a
MaxRects bin packer under three heuristics, centres the block, computes the
divider lines, the dots and the datum features (slots or dowel holes, the jig
pin centres and, for the slots datum, the centre of each cell's orientation X).
`marker_strokes()` / `marker_half()` turn that centre into the two crossed
strokes the writer and the renderer draw and into the square they cut, which is
what keeps divider dots out of the X. `layout_report()` renders the text
report. Depends on `model` and `util` - no geometry library.
See [layout.md](layout.md).

**`render.rs`** - the preview. `render_preview(&projects, &layout, path, &opts)`
rasterises the sheet into a `tiny_skia::Pixmap` from the `geo` geometry of the
gerber objects and writes a PNG; `open_file(path)` hands a file to the desktop
viewer. Text is drawn with `ab_glyph` from the DejaVu Sans face embedded from
`assets/`. See [render.md](render.md).

**`ascii.rs`** - the half-block rasteriser behind the TUI's split view. A
`Raster` is a window of the sheet in square sub-pixels, two per character row;
`render_pad()` fills it in the same class order the PNG composites in, sampling
`Shape`s (an `IntervalTreeMultiPolygon` per object, kept in a `GeomCache`)
instead of transforming geometry.

**`tui.rs`** - the ratatui front-end. `run_tui(…) -> anyhow::Result<bool>`
returns true when the user asked to generate. Everything above the `App` struct
is pure and testable; `App` only draws (`App::draw`) and dispatches keys
(`App::handle_key`). See [tui.md](tui.md).

**`util.rs`** - two helpers everyone uses: `natural_key` (digit runs compare as
numbers, so `R10` sorts after `R2`) and `fmt_mm` (compact millimetre text).

**`cli.rs`** - the clap `Args` struct, the pipeline above, the three writers,
the zip and the stdout summary.

```
cli ──┬──> project ──> gerber, model
      ├──> config  ──> model, pads, util
      ├──> pads    ──> gerber, model, util    (rstar)
      ├──> layout  ──> model, util
      ├──> render  ──> gerber, layout, model  (tiny-skia, ab_glyph)
      ├──> writer  ──> gerber, model
      └──> tui     ──> ascii, model           (ratatui, crossterm)

              ascii ──> gerber, layout, model
              model ──> gerber                (geo)
             gerber ──> (geo, i_overlay)
```

## Two copies of the state

The same decisions exist twice while the program runs:

* **The objects.** `Project` → `Side` → `Pad`, held in one `Vec<Project>` that
  `run` owns; a `SideId { project, side }` is an index pair into it.
  `side.enabled` is the side switch, `pad.state` is the open/ignore/undefined
  decision. This is what the packer, the renderer and the writers read.
* **The `Config` maps.** `config.sides` maps `"<project>/<side>"` to a `bool`
  and `config.pads` maps a pad key to a state string, both `BTreeMap`. This is
  what the file contains, and it also holds entries for pads and sides that are
  *not* in the current gerbers.

Two functions reconcile them, and nothing else writes across the boundary:

* `config::apply_config(&config, &mut projects)` pushes the maps into the
  objects. A side not in `config.sides` defaults to enabled. A candidate pad
  whose key is missing (or whose stored value is not a known state) falls back
  to `pads::default_state()`; a pad that already has paste may only be `open`
  or `ignore` and defaults to `open`.
* `config::collect_config(&mut config, &projects)` pulls the objects back into
  the maps. Every candidate is stored with its state. A pasted pad is stored
  only when it was closed (`= ignore`); when it is open again its key is
  removed, because open is the default. Keys that match nothing in the current
  projects are left untouched, which is how hand-made decisions survive a
  temporarily missing board - `format_config()` writes them out under a
  "not found in the current gerbers" header.

`save_config()` is `collect_config()` plus an atomic write, so saving is always
the last word of the objects. The TUI deliberately mutates only the objects and
the scalar parts of the config (`size`, `orientation`, `layout.*`) and never
touches `config.sides` / `config.pads`; `run` collects them afterwards.

## Error handling and exit codes

`run` returns `anyhow::Result<i32>`: user-level problems are either a printed
error with a return code or a warning, and the `Err` arm carries a
`GerberError` or an `io::Error` up to `main`, which prints `error: {exc}` and
returns 2.

| Code | When |
| --- | --- |
| 0 | The stencil was generated. |
| 1 | The user left the TUI with `q` or Ctrl-C (the configuration is still saved). |
| 2 | No gerber projects found; no side has paste openings or pads to decide; no enabled side has a single opening; any `GerberError` or io error; and, from clap itself, a bad command line or a value rejected by `check_args`. |

Warnings go to stderr through the local `warn()` helper, which flushes stdout
first so the two streams stay in order. They never stop the run: an unreadable
source, a layer that fails to parse, a duplicate layer role, an unknown key in
the configuration file, a bad number in it, a block that does not fit, pads left
undefined in `--batch` - all of these are warnings. A gerber file that fails to
parse is dropped from its project; a project that ends up with neither copper
nor paste is skipped.

The TUI installs a `TerminalGuard` whose `Drop` calls `ratatui::restore()`, so
the terminal is put back whatever happens, a panic included (the panic hook
`ratatui::try_init` installs covers that case).

## Where the time goes

Measured on the eight sample gerber zips in the parent directory (8 projects,
40 classified layers, 5,472 graphic objects, 685 pads, 11 placed cells), release
build, single-threaded throughout:

| Stage | Time | Notes |
| --- | --- | --- |
| `discover_projects`, 8 zips (read + parse + assemble) | 16 ms | Inflation plus `GerberFile::bounds()`, which builds the geometry of every object. |
| `object_geometry` over all 5,472 objects | 9.0 ms | The boolean work: regions, macro apertures, stroke buffers. |
| `detect_pads` over all 16 sides | 6.1 ms | One `RTree` per side plus one intersection per pad/opening pair. |
| `pack` over 13 sides | 161 µs (`slots`), 197 µs (`holes`), 136 µs (`none`) | Three heuristics are run over the full cell list each time. |
| `render_preview` at 20 px/mm → 7834 x 5859 px | 220 ms | |
| The whole `--batch --no-open` run | 0.52 s, peak RSS 370 MiB | Two renders (first preview and final) are 85 % of it. |

The practical consequences: packing is cheap enough to redo on every keystroke
that changes something (the TUI does), and filling the Stencil page's fit
column costs 16 packs (8 sizes x 2 orientations), still a few milliseconds.
Rendering is not cheap, which is why the PNG is only redrawn on `p` and why the
status line says `rendering…` first; the terminal preview behind `v` exists
because it is cheap enough to redraw on every cursor move.
[benchmarks.md](benchmarks.md) has the method, the spread and the memory
probes.
