# Stencicrity developer documentation

How the crate is built, for anyone changing it. `../README.md` is the user
manual; start there if you only want to run it.

| Document | Contents |
| --- | --- |
| [architecture.md](architecture.md) | What the program does, the pipeline of `cli::run` step by step, the module map and its dependency graph, the two copies of the state, exit codes and where the time goes. |
| [data-model.md](data-model.md) | Every type in `model.rs` with its fields and invariants, the constants, the three coordinate systems and how the types reference each other. |
| [gerber.md](gerber.md) | The RS-274X subset the reader accepts, the macro evaluator, how apertures become `geo` geometry, aperture baking, and what the writer emits. |
| [layout.md](layout.md) | Cells and the jig raster, the MaxRects packer and its heuristics, dividers, dots and the clearance rule, the alignment datum (slots, holes, none), the report, and a worked example with real numbers. |
| [pads-and-config.md](pads-and-config.md) | Pad detection and the paste overlap test, default states, what open and close mean for the output, the `.stencicrity` file format, the legacy mappings and the option precedence. |
| [tui.md](tui.md) | The four ratatui pages, their keys, the search and its ranking, the all-pads mode, the split view, inline editing, the caches and the generate confirmation. |
| [render.md](render.md) | The preview pipeline: frame geometry, the reused class mask, colours, labels, rulers, the embedded font and `open_file`. |
| [benchmarks.md](benchmarks.md) | Measured stage timings and peak memory of the crate, with the comparison against the retired Python implementation it replaced. |
| [development.md](development.md) | Building and running from source, the test layout, clippy and fmt, coding conventions, adding a layout parameter end to end, the release process, the packaging files, CI, and the known rough edges. |
| [kinematic-alignment.md](kinematic-alignment.md) | Design study on locating cut stencil pieces on a pin jig; the `slots` datum comes from it. |
| [distribution-research.md](distribution-research.md) | Research report on shipping the tool to a Windows and Arch Linux fleet. Predates the Rust port; its Python-specific parts are historical. |
