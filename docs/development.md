# Development

## Running from source

The repository needs no build step. From a checkout:

```sh
pip install shapely pillow numpy        # or the distribution packages
cd /path/to/some/gerber/zips
python3 /path/to/Stencicrity/stencicrity.py
```

`stencicrity.py` only imports `pcbstencil.cli.main`, so the parent directory of
`pcbstencil/` has to be importable - running the script by path is enough,
Python puts the script's directory on `sys.path`. `pip install -e .` gives a
`stencicrity` console script pointing at the same function, and
`uv run --with shapely --with pillow --with numpy stencicrity.py` works without
installing anything.

Python 3.11 or newer (`requires-python = ">=3.11"`); the code uses `X | None`
annotations under `from __future__ import annotations`, `match`-free plain
control flow and nothing newer. Development happens on 3.14. Dependencies are
shapely 2, Pillow 10.1 and numpy (numpy is a declared dependency but is not
imported anywhere in `pcbstencil` - shapely pulls it in).

## Smoke checks

There is no test suite (see below), so these are the checks worth running after
a change:

```sh
python3 -m compileall pcbstencil                     # syntax
python3 stencicrity.py --version && python3 stencicrity.py --help
cd /tmp/scratch-copy-of-the-zips
python3 /path/to/stencicrity.py --batch --no-open    # the whole pipeline
```

`--batch --no-open` exercises discovery, parsing, pad detection, the
configuration round trip, packing, both renders and both writers without
touching a terminal. On the eight sample projects it takes about 2.5 s and
prints the side table, the block size, the pad counts and the file list. Useful
follow-ups on the output:

```sh
python3 -c "import re,sys; print(open(sys.argv[1]).read().count('D03*'))" stencil-out/stencil-F_Paste.gbr
grep -c '^%ADD' stencil-out/stencil-F_Paste.gbr
head -30 stencil-out/stencil-report.txt
```

Run it in a copy of the gerber folder, never in the original: the tool writes
`.stencicrity` and `stencil-out/` into the working directory.

## Tests

**The repository has no tests.** During development the checks lived as ad-hoc
scripts in the session scratchpad (a `/tmp/.../scratchpad` directory) and were
not kept: small drivers that imported `pcbstencil.*`, ran one function over the
sample gerbers and printed numbers. Nothing in `pyproject.toml`, `ci.yml` or the
packaging refers to a test runner.

A minimal `tests/` (pytest, no fixtures beyond a couple of short gerber strings)
should cover, roughly in order of value:

| Area | What to assert |
| --- | --- |
| `gerber` parser | a hand-written gerber with FS/MO/AD/D01/D02/D03/G36 parses into the expected object list; `MOIN` scales; trailing-zero omission; an arc's discretisation stays within `ARC_TOLERANCE`; `SR`, `LM`, `LR`, `LS`, an unknown macro and an unknown primitive each raise `GerberError`. |
| macros | `_Expr` arithmetic and precedence; each supported primitive's evaluated geometry area; `MacroPrim.mirrored()` and `as_gerber()` round-trip. |
| `writer` | a file written and parsed back gives the same geometry; aperture deduplication by `BakedAperture.key()`; arc direction flips under a mirroring transform; `%TD*%` only follows an aperture with attributes. |
| `pads` | `matches_prefix` on `TP1`/`NT12`/`tp3`/`TPS1`/`T1`; `default_state` for the three cases; `_paste_hits` with an opening that only holds the centroid and with several partial openings; `natural_key` ordering. |
| `config` | round trip `format_config` -> `load_config`; every warning path keeps the default; the flat legacy format; stale keys survive; `collect_config` drops an open pasted pad and keeps a closed one. |
| `layout` | `_grid_size` / `_snap_up` / `_snap_down` arithmetic; a two-cell packing's exact coordinates; `_merge` on touching intervals; `_dividers` dropping the block boundary unless `outer_border`; `_dots` count and centring; `fits` at the exact boundary. |
| `tui` pure helpers | everything above the `_App` class: `cycle_state`, `next_state`, `allowed_state`, `apply_component`, `find_undefined`, `visible_pads`, `wrap_items`, `query_words`, `match_count`, `filter_counts`, `restore_index`, `step_value`, `valid_value`, `parse_number`, `edit_buffer`. These were written to be testable and need no curses. |
| end to end | one tiny gerber zip through `cli.main(["--batch", "--no-open", "--out", tmp])`, asserting the exit code and that the five output files exist. |

`curses` and Pillow are the only parts that need care: the TUI's pure helpers
import `curses` only for key constants (`tui.py` imports the module at top
level, so a test host still needs it), and a render test should use a small
`px_per_mm` and only check the image size.

## Conventions in the code

* Module docstrings carry the design. `model.py` documents the coordinate
  systems, `layout.py` the packing and the dotted border, `config.py` the file
  format, `tui.py` the pages and the pure/impure split. Keep them current -
  they are the first thing a reader sees.
* Every public function has a one-line docstring in the imperative, with the
  details below it. Private helpers start with `_`.
* Type annotations everywhere, `from __future__ import annotations` at the top
  of every module.
* Pure logic is separated from I/O so it can be tested: all of `tui.py` above
  `_App`, all of `layout.py` except `layout_report`, `pads.py` entirely.
* Numeric tolerances are named module constants with a comment explaining what
  they are for (`_EPS`, `_FIT_EPS`, `_MERGE_EPS`, `_ALIGN_EPS`, `_HOLE_EPS`,
  `_GRID_EPS`, `ARC_TOLERANCE`). Do not inline a new magic epsilon.
* Warnings are a `warn(message)` callable passed in, never a bare `print`, so
  the CLI can keep stdout and stderr in order and a caller can capture them.
* Nothing in the loading path raises for user error: it warns and keeps a
  default. Only `GerberError` and `OSError` escape to `main()`.
* British spelling in prose ("centred", "millimetres"), lower case in messages,
  no exclamation marks.
* `cli._run()` imports the heavy modules inside the function so `--help` and
  `--version` work with a broken or missing dependency.

## Adding a layout parameter end to end

Say you want `frame_width`. Seven places, in this order:

1. **`model.LayoutParams`** - add the field with its default and a comment
   giving the unit. Add a derived property if other code would repeat the same
   arithmetic (like `pad` and `hole_offset`).
2. **`config.py`** - add the key to `_LAYOUT_FLOATS` (attribute, validity
   predicate, requirement text) or `_LAYOUT_BOOLS`, or write a branch in
   `_read_layout()` for anything else; add an old spelling to `_LAYOUT_ALIASES`
   if you are renaming something. Add an `_entry("frame_width", …)` line to the
   `[layout]` block in `format_config()` so it is written back with a comment -
   a parameter that is not written there is silently lost on the next save.
3. **`cli.build_parser()`** - add `--frame-width` to the "layout" argument group
   with `default=None` (that is what makes the file win when the option is not
   given), then add `("frame_width", args.frame_width)` to the tuple in
   `apply_cli_config()`, and a row in `_check_args()` if it has a valid range.
4. **`tui.LAYOUT_FIELDS`** - add a `Field(attr, label, kind, unit, step,
   minimum, rule)`; `kind` is `"length"`, `"bool"` or `"choice"`. The Layout
   page, the stepping, the inline editor and the validation all come from that
   one entry.
5. **`layout.py`** - use it in `pack()` / `_dividers()` / `_dots()` /
   `_holes()`, and report it in `layout_report()`.
6. **`README.md`** - the `[layout]` example block, the option list under
   "Options worth knowing", and the Layout row of the TUI table.
7. **`docs/`** - the `LayoutParams` table in
   [data-model.md](data-model.md), the mechanism in [layout.md](layout.md), the
   `[layout]` key table in [pads-and-config.md](pads-and-config.md) and the
   field table in [tui.md](tui.md).

Then re-run the batch smoke check and diff the generated `.stencicrity`.

## Release process

The version lives in exactly one place, `pcbstencil/__init__.py`:

```python
__version__ = "0.1.2"
```

`pyproject.toml` reads it (`version = { attr = "pcbstencil.__version__" }`),
`--version` prints it, and the release workflow refuses to build if the tag does
not match it. To cut a release:

1. Bump `__version__`.
2. Commit and push to `main`.
3. Publish a GitHub release tagged `vX.Y.Z` (or `X.Y.Z`) on that commit.

Publishing fires `.github/workflows/release.yml`. Only the `published` event is
used, so a draft that is created and published later builds exactly once. The
`version` job strips a leading `v`, checks the shape, reads `__version__` out of
`pcbstencil/__init__.py` with a regex and fails loudly when the two differ -
nothing is ever rewritten at build time, the source is the single source of
truth. Two jobs then run in parallel:

* **arch** - in an `archlinux:base-devel` container: installs the toolchain,
  `git archive --prefix=stencicrity-$VERSION/` into `packaging/arch/`,
  `sed`s `pkgver` in the PKGBUILD, builds with `makepkg -f` as an unprivileged
  `builder` user, then `pacman -Qip` and `pacman -U` the result and runs
  `stencicrity --version`.
* **deb** - in a `debian:trixie` container: copies `packaging/debian` to
  `./debian` (dpkg only looks there), sets the changelog version with
  `dch --newversion $VERSION-1 --force-bad-version`, builds with
  `dpkg-buildpackage -us -uc -b`, inspects with `dpkg-deb -I` / `-c` and
  installs the `.deb` with apt.

Both upload an artifact always, and `gh release upload "$TAG" … --clobber` the
package onto the release only when the run was triggered by publishing one.
`workflow_dispatch` (with the version as an input) builds and keeps the
artifacts without touching any release, which is the way to rehearse a release.

The `pkgver=0.1.0` in `packaging/arch/PKGBUILD` and the `0.1.0-1` entry in
`packaging/debian/changelog` are **not** kept in sync by hand: the workflow
rewrites both. Do not bump them in a commit.

## Packaging files

| Path | Purpose |
| --- | --- |
| `pyproject.toml` | setuptools build, dynamic version from `pcbstencil.__version__`, dependencies (`shapely>=2`, `pillow>=10.1`, `numpy`), the `stencicrity = pcbstencil.cli:main` console script, AGPL-3.0-or-later with `LICENSE` as the license file, `packages = ["pcbstencil"]`. |
| `packaging/arch/PKGBUILD` | `arch=(any)`, depends on `python-shapely` / `python-pillow` / `python-numpy`, optdepends on `xdg-utils`; builds a wheel with `python -m build --no-isolation` and installs it with `python -m installer`; the source tarball is produced next to it by the workflow, so `sha256sums=('SKIP')`. |
| `packaging/debian/control` | Source and binary `stencicrity`, `Architecture: all`, depends on `python3-shapely (>= 2)`, `python3-pil`, `python3-numpy`, recommends `xdg-utils`. |
| `packaging/debian/rules` | `dh $@ --with python3 --buildsystem=pybuild` with `PYBUILD_NAME=pcbstencil` - the importable package is `pcbstencil`, the binary package is `stencicrity`. |
| `packaging/debian/source/format` | `3.0 (native)`: the packaging lives in the upstream tree, there is no separate upstream tarball. |
| `packaging/debian/changelog`, `copyright`, `docs`, `README.Debian` | Changelog is rewritten by `dch` at release time; `README.Debian` documents which releases have a new enough Shapely (trixie and noble upwards; bookworm and jammy are too old). |
| `.gitignore` | Ignores `stencil-out/`, the build artefacts, `*.pkg.tar.zst`, `*.deb`, `pkg/`, `src/` and the `./debian` build-time copy. |

## CI

`.github/workflows/ci.yml` runs on pushes to `main` and on pull requests, with
a matrix of Python 3.12 and 3.13. It installs the package with `pip install .`,
runs `stencicrity --version` and `--help`, byte-compiles the package with
`compileall` and builds the wheel and the sdist. That is the whole gate: there
is nothing that would catch a behavioural regression, and neither 3.11 (the
declared minimum) nor 3.14 (what development uses) is covered.

## Known rough edges

Things found while reading the code. None of them break a normal run; they are
listed so nobody has to rediscover them.

* **Macro modifiers are not unit-scaled.** `_define_aperture()` scales standard
  aperture modifiers by the current unit but deliberately leaves macro
  parameters alone, because "KiCad only uses mm". A `MOIN` file using macro
  apertures would be read at 1/25.4 of its size.
* **`min_overlap` and `max_px` are not reachable.** The paste overlap threshold
  (0.10) and the preview pixel cap (12000) are keyword arguments with no command
  line switch.
* **The final layout is packed twice.** `_run()` packs once for the preview and
  the TUI over every enabled relevant side, and again over only the sides that
  still have openings. Dropping a side changes the input list, so the final
  arrangement can differ from what the user confirmed. The final preview is
  re-rendered from the final layout, so the PNG and the gerbers always agree -
  but the image the user looked at during the session may not be the one on
  disk afterwards.
* **`Layout.params` is the live `config.layout` object**, not a copy. Mutating
  the configuration after packing retroactively changes what a `Layout` reports.
* **`_handle_sides()` and `_handle_stencil()` index the unfiltered list.** They
  use `self.sides[self.index]` and `STENCIL_SIZES[self.index]` rather than the
  row under the cursor from `_rows_list()`. Correct today only because search
  mode intercepts keys before the page dispatch, so `filtered` is always `None`
  there; enabling `/` on another page would break it.
* **Keys of pads without `%TO.P` are unstable.** They get the reference `?` and
  a per-side running index, so adding one such flash renumbers the rest and
  their decisions in `[pads]` become stale entries.
* **`README.md` is drifting.** It lists seven sheet sizes; `STENCIL_SIZES` now
  has eight - 270x270 was added at the front - while the default is still
  380x280 (`DEFAULT_STENCIL_SIZE` is a literal now, no longer
  `STENCIL_SIZES[0]`). Check the README's size list, key table and option list
  against the code before a release.
* **No tests, and CI cannot catch a regression.** See the two sections above.
