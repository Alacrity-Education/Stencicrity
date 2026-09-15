# Stencicrity developer documentation

How the tool is built, for anyone changing it. `../README.md` is the user
manual; start there if you only want to run it.

| Document | Contents |
| --- | --- |
| [architecture.md](architecture.md) | What the program does, the pipeline of `cli.main()` step by step, a module map, the two copies of the state, exit codes and measured timings. |
| [data-model.md](data-model.md) | Every dataclass in `model.py` with its fields and invariants, the constants, the three coordinate systems and a class diagram. |
| [gerber.md](gerber.md) | The RS-274X subset the parser accepts, the macro evaluator, aperture baking, and what the writer emits. |
| [layout.md](layout.md) | Cells and the hole grid, the MaxRects packer and its heuristics, dividers, dots, dowel holes, the report, and a worked example with real numbers. |
| [pads-and-config.md](pads-and-config.md) | Pad detection and the paste overlap test, default states, what open and close mean for the output, the `.stencicrity` file format, and the option precedence. |
| [tui.md](tui.md) | The four curses pages, their keys, the search, the all-pads mode, inline editing and the generate confirmation. |
| [render.md](render.md) | The preview pipeline: frame geometry, class masks, colours, labels, rulers and `open_file`. |
| [development.md](development.md) | Running from source, smoke checks, the missing test suite, coding conventions, adding a layout parameter, the release process, packaging and CI, and the known rough edges. |
| [kinematic-alignment.md](kinematic-alignment.md) | Design study on locating cut stencil pieces on a pin jig. |
| [distribution-research.md](distribution-research.md) | Research report on shipping the tool to a Windows and Arch Linux fleet. |
