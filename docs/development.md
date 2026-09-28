# Development

## Building and running from source

One crate, no build script, no code generation. Cargo 1.85 or newer
(`rust-version` in `Cargo.toml`, edition 2021):

```sh
cargo build --release              # ./target/release/stencicrity
cargo install --path .             # a stencicrity on the PATH
cargo run --release -- --help
```

Run it in a **copy** of a gerber folder, never in the original: the tool writes
`.stencicrity` and `stencil-out/` into the working directory.

```sh
cd /tmp/scratch-copy-of-the-zips
/path/to/target/release/stencicrity --batch --no-open
```

`--batch --no-open` exercises discovery, parsing, pad detection, the
configuration round trip, packing, both renders and both writers without
touching a terminal. On the eight sample projects it takes about half a second
and prints the side table, the block size, the pad counts and the file list.
Useful follow-ups on the output:

```sh
grep -c 'D03\*' stencil-out/stencil-F_Paste.gbr
grep -c '^%ADD' stencil-out/stencil-F_Paste.gbr
head -30 stencil-out/stencil-report.txt
```

The release profile is `lto = "thin"`, `codegen-units = 1`, `strip = true`. A
debug build works but the geometry is slow enough to be annoying, which is why
the tests run in release too.

## Tests

```sh
cargo test --release                      # everything but the ignored ones
cargo test --release -- --ignored         # the two that need real gerbers
cargo clippy --all-targets -- -D warnings
cargo fmt --all --check
```

All four are what CI runs, and all four have to pass before a release.

Unit tests live in a `#[cfg(test)] mod tests` at the bottom of the module they
cover — that is where they can reach the private frame geometry, the private
packing helpers and the private parser. The integration tests in `tests/` go
through the public API, or through the built binary:

| File | What it covers |
| --- | --- |
| `cli_args.rs` | the real executable: `--version`, `--help` naming all 34 long options, the exit codes of the paths that never reach the pipeline, the mutually exclusive groups, `--overrides` as a hidden alias of `--config` |
| `cli_end_to_end.rs` | a whole run over a four-pad KiCad-style board written by the test itself, so it needs no fixtures: the two merged gerbers (re-parsed with `gerber::parse_gerber`), the zip, the report, the preview and the `.stencicrity` |
| `config_file.rs` | `.stencicrity` parsing and writing, the legacy formats, stale entries, atomic saving, and `apply_config` / `collect_config` over the example projects |
| `pads_detection.rs` | totals, the NT/TP rule and the ordering, on the example gerber sets |
| `project_discovery.rs` | layer classification (no data needed), `load_source` over zips and directories, discovery, own-output detection, duplicate names |
| `render_preview.rs` | `render_preview` from outside the crate: the PNG exists, carries every colour class, honours `selected`, and the cap shrinks the image without losing the bands |
| `render_examples.rs` | `#[ignore]`: a preview of a real gerber set, for eyeballing and for timing |
| `tui_api.rs` | the TUI's public surface driven with a `TestBackend` and a handful of keys |
| `tui_split.rs` | the split view: `split_plan`, `panel_rows`, the raster, and an `#[ignore]` timing run over a real gerber set |

Several tests need data that is not in the repository and **skip themselves**
with a printed note when it is not there, so a bare checkout still passes:

| Needs | How it is found |
| --- | --- |
| the example gerber zips | any `*.zip` in the crate's parent directory |
| a generated `stencil-out/` to recognise as own output | the parent directory again |
| extracted gerber folders | `STENCICRITY_GERBER_DIR`, else skipped |

The two `#[ignore]` tests are the ones that need a real gerber set and are
therefore never run by `cargo test` on its own:

```sh
STENCICRITY_EXAMPLES=/path/to/zips \
STENCICRITY_PREVIEW=/tmp/preview.png \
STENCICRITY_PPMM=20 \
    cargo test --release --test render_examples -- --ignored --nocapture

STENCICRITY_EXAMPLES=/path/to/zips \
    cargo test --release --test tui_split -- --ignored --nocapture
```

`examples/bench.rs` is the timing harness behind [benchmarks.md](benchmarks.md):

```sh
cargo run --release --example bench
STENCIL_BENCH_DATA=/path/to/zips STENCIL_BENCH_N=20 \
    cargo run --release --example bench
```

It takes `STENCIL_BENCH_DATA`, `STENCIL_BENCH_OUT`, `STENCIL_BENCH_N`,
`STENCIL_BENCH_WARM` and `STENCIL_BENCH_ONLY`, and prints one TSV line per
repetition plus `#` lines with the input sizes and the peak RSS.

## Conventions in the code

* Module comments carry the design. `model.rs` documents the coordinate
  systems, `layout.rs` the packing and the dotted border, `config.rs` the file
  format, `tui.rs` the pages and the pure/impure split, `render.rs` the
  drawing order and the memory budget. Keep them current — they are the first
  thing a reader sees.
* Every public item has a one-line doc comment, with the details below it.
* Pure logic is separated from I/O so it can be tested: everything in `tui.rs`
  above `App`, all of `layout.rs` except `layout_report`, all of `pads.rs`.
* Numeric tolerances are named constants with a comment explaining what they
  are for (`EPS`, `FIT_EPS`, `MERGE_EPS`, `ALIGN_EPS`, `HOLE_EPS`, `GRID_EPS`,
  `ARC_TOLERANCE`). Do not inline a new magic epsilon.
* Warnings are a `&mut dyn FnMut(&str)` passed in, never a bare `eprintln!`,
  so the CLI can keep stdout and stderr in order and a caller can capture
  them.
* Nothing in the loading path fails for user error: it warns and keeps a
  default. Only a `GerberError` or an io error reaches `main`.
* `unwrap` / `expect` only where the invariant is local and obvious; the
  library returns `Result` and `cli` turns it into an exit code.
* British spelling in prose ("centred", "millimetres"), lower case in
  messages, no exclamation marks.

## Adding a layout parameter end to end

Say you want `frame_width`. Six places, in this order. (The `datum` switch, its
`slot_*` numbers and the `marker` / `marker_size` pair went through exactly
these steps; grep for `slot_pitch` to see one number in all six at once, for
`marker` to see a `bool` and for `datum` to see a choice.)

1. **`model.rs`, `LayoutParams`** — add the field with a doc comment giving the
   unit, and its value in the `Default` impl. Add a derived method if other
   code would repeat the same arithmetic (like `pad()` or `hole_offset()`).
2. **`config.rs`** — add the key to `LAYOUT_FLOATS` (config key, field name,
   `FloatCheck`) and to the `layout_float` match, or write a branch in
   `read_layout` for a bool or a choice; add an old spelling to
   `LAYOUT_ALIASES` if you are renaming something. Add an
   `entry("frame_width", …)` line to the `[layout]` block in `format_config`,
   with its comment — a parameter that is not written there is silently lost
   on the next save.
3. **`cli.rs`** — add `--frame-width` to the `Args` struct with
   `Option<f64>` and the layout `help_heading` (an `Option` is what makes the
   file win when the option is not given), a line in `apply_cli_config`, and a
   row in the `check_args` table when it has a valid range.
4. **`tui.rs`** — add an `Attr` variant, its arms in `field_number` /
   `set_field_number` (or the flag / choice pair), and a `Field` to
   `LAYOUT_FIELDS`. The Layout page, the stepping, the inline editor and the
   validation all come from that one entry. A row that only applies to one
   datum belongs in `datum_row` as well, so `field_applies` dims it when it
   does not apply.
5. **`layout.rs`** — use it in `pack` / `cell_of` / `dividers_of` / `dots_of` /
   `datum_of`, and report it in `layout_report`.
6. **Docs** — `README.md` (the `[layout]` example block, the option list under
   "Options worth knowing", the Layout row of the TUI table), the
   `LayoutParams` table in [data-model.md](data-model.md), the mechanism in
   [layout.md](layout.md), the `[layout]` key table in
   [pads-and-config.md](pads-and-config.md), the field table in
   [tui.md](tui.md) and, when it shows up in the preview,
   [render.md](render.md).

Then add a unit test to the module that uses it, re-run the batch smoke check
and diff the generated `.stencicrity`.

**Renaming or replacing one.** `datum` replaced a boolean `holes` field. The
rule is that an existing `.stencicrity` must keep behaving the way it did:
`format_config` stops writing the old key, `read_layout` keeps a branch that
maps it onto the new one (and lets the new key win when both are present), and
the old command line spelling stays as an alias (`--holes` / `--no-holes` set
`--datum`).

**Dropping one.** `slot_corner` (the distance from the cell corner to the two
bottom slots) died when the slots moved onto the `slot_pitch` raster: there is
nothing left to map it onto. The field, the `LAYOUT_FLOATS` entry, the
`format_config` line, the option and the TUI row all go, and the key is added
to `OBSOLETE_LAYOUT_KEYS` instead, which makes `load_config` read and drop it
**without a warning** — an old file loads silently and simply loses the key
the next time it is written. A parameter that still means something to the
user gets an alias; one that no longer exists gets a place on that list.

## Release process

The version lives in exactly one place, `Cargo.toml`:

```toml
version = "0.2.0"
```

`--version` prints it (clap reads `CARGO_PKG_VERSION`), `lib::VERSION` is the
same string, the gerber header carries it, and the release workflow refuses to
build if the tag does not match it. To cut a release:

1. Bump `version` in `Cargo.toml` (and let `cargo build` update `Cargo.lock`).
2. Commit and push to `main`.
3. Publish a GitHub release tagged `vX.Y.Z` (or `X.Y.Z`) on that commit.

Publishing fires `.github/workflows/release.yml`. Only the `published` event is
used, so a draft that is created and published later builds exactly once. The
`version` job strips a leading `v`, checks the shape, reads the crate version
with `cargo metadata | jq` and fails loudly when the two differ — nothing is
ever rewritten at build time, the source is the single source of truth. Two
jobs then run in parallel:

* **arch** — in an `archlinux:base-devel` container: installs `git rust sudo
  github-cli`, `git archive --prefix=stencicrity-$VERSION/` into
  `packaging/arch/`, `sed`s `pkgver` in the PKGBUILD, builds with `makepkg -f`
  as an unprivileged `builder` user, then `pacman -Qip` and `pacman -U` the
  result and runs `stencicrity --version`. The asset is
  `stencicrity-<version>-1-x86_64.pkg.tar.zst`.
* **deb** — in a `debian:trixie` container: installs rustup non-interactively
  (trixie's own rustc is exactly 1.85, the minimum, so the build uses the same
  stable toolchain the developers do), copies `packaging/debian` to `./debian`
  (dpkg only looks there), sets the changelog version with
  `dch --newversion $VERSION-1 --force-bad-version`, builds with
  `dpkg-buildpackage -us -uc -b -d`, inspects with `dpkg-deb -I` / `-c` and
  installs the `.deb` with apt. The asset is
  `stencicrity_<version>-1_amd64.deb`.

Both upload an artifact always, and `gh release upload "$TAG" … --clobber` the
package onto the release only when the run was triggered by publishing one.
`workflow_dispatch` (with the version as an input) builds and keeps the
artifacts without touching any release, which is the way to rehearse a
release.

The `pkgver=` in `packaging/arch/PKGBUILD` and the version in
`packaging/debian/changelog` are **not** kept in sync by hand: the workflow
rewrites both. Do not bump them in a commit.

The `-d` on `dpkg-buildpackage` is there because cargo comes from rustup and
is not a dpkg-installed build-dependency; see
`packaging/debian/README.Debian`.

## Packaging files

| Path | Purpose |
| --- | --- |
| `Cargo.toml` | the version, `rust-version = "1.85"`, `default-run = "stencicrity"`, AGPL-3.0-or-later, the dependency list and the release profile. |
| `Cargo.lock` | committed, and authoritative: both package builds pass `--locked` / `--frozen`, so a build resolves to exactly the versions the crate was tested against. |
| `packaging/arch/PKGBUILD` | `arch=(x86_64)`, `depends=(gcc-libs glibc)`, `optdepends` on `xdg-utils`, `options=(!debug)` because the release profile already strips. `prepare`/`build`/`check`/`package` are plain cargo calls; the source tarball is produced next to it by the workflow, so `sha256sums=('SKIP')`. Installs the binary, `LICENSE`, `README.md` and `docs/*.md` (the figures are left out). |
| `packaging/debian/control` | source and binary `stencicrity`, `Architecture: any`, `Build-Depends: debhelper-compat (= 13)` only, `Recommends: xdg-utils`; `${shlibs:Depends}` resolves to libc6 and libgcc-s1. |
| `packaging/debian/rules` | `dh $@` with `override_dh_auto_build/test/install/clean` driving cargo directly, `CARGO_HOME = debian/cargo` so nothing is written outside the source tree. |
| `packaging/debian/source/format` | `3.0 (native)`: the packaging lives in the upstream tree, there is no separate upstream tarball. |
| `packaging/debian/changelog`, `copyright`, `docs`, `README.Debian` | the changelog is rewritten by `dch` at release time; `README.Debian` explains the static binary, the rustup toolchain and the `-d`. |
| `.gitignore` | `stencil-out/`, `/target/`, the package outputs and the build-time `./debian` copy. |

## CI

`.github/workflows/ci.yml` runs on pushes to `main` and on pull requests:
`cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test --release`, `cargo build --release`, then `--version` and `--help`
on the built binary. Two further jobs mirror the release workflow's package
builds, minus the version check and the upload, so a change to `packaging/` is
caught by the pull request that makes it rather than by the release that needs
it.

## Known rough edges

Things found while reading the code. None of them break a normal run; they are
listed so nobody has to rediscover them.

* **Macro modifiers are not unit-scaled.** `define_aperture` scales standard
  aperture modifiers by the current unit but deliberately leaves macro
  parameters alone, because "KiCad only uses mm". A `MOIN` file using macro
  apertures would be read at 1/25.4 of its size.
* **`min_overlap` and `max_px` are not reachable.** The paste overlap
  threshold (0.10) and the preview pixel cap (12000) are fields of
  `DetectOptions` / `RenderOptions` with no command line switch.
* **The final layout is packed twice.** `run` packs once for the preview and
  the TUI over every enabled relevant side, and again over only the sides that
  still have openings. Dropping a side changes the input list, so the final
  arrangement can differ from what the user confirmed. The final preview is
  re-rendered from the final layout, so the PNG and the gerbers always agree —
  but the image the user looked at during the session may not be the one on
  disk afterwards.
* **`handle_sides` and `handle_stencil` differ.** `handle_sides` takes the row
  under the cursor from `current_row()`, but `handle_stencil` indexes
  `STENCIL_SIZES[self.index()]` directly. That is correct today only because
  the Stencil page cannot be filtered; enabling `/` on it would break it.
* **Keys of pads without `%TO.P` are unstable.** They get the reference `?`
  and a per-side running index, so adding one such flash renumbers the rest
  and their decisions in `[pads]` become stale entries.
* **Only one process may write a `.stencicrity`.** The write itself is atomic,
  but there is no lock: two runs in the same folder will not corrupt the file,
  and the second one simply wins.
* **Linux only.** `config.rs` uses `std::os::unix::fs` for the file mode and
  reads the umask from `/proc/self/status`; `render::open_file` calls
  `xdg-open`. The crate does not build for Windows as it stands, and the
  release packages are x86_64 only.
* **`--px-per-mm` is an upper bound, not a setting.** `frame_for` reduces it
  until the image fits `max_px`, so on a large sheet the number asked for and
  the number used differ, and only the legend and the report say what the
  image really is.
