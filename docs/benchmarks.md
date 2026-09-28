# Benchmarks: python vs rust

## Purpose

The python package (`pcbstencil/`, driven by `stencicrity.py`) is being
retired; the rust crate replaces it. Both produce the same gerbers — the merged
paste layer, the copper reference and the report are byte for byte identical on
the example boards, modulo the version string and the creation timestamp in the
gerber header. This document records where the time and the memory went before
the python side disappears, so the comparison is not lost with it. The short
answer: the whole batch run is **4.1x** faster and needs **27% less** peak
memory; the geometry-heavy stages (pad detection, `object_geometry`) are 14–21x
faster, and rendering — which dominates a run in both — is 3–4x faster.

## Method

The same eight example gerber sets and the same `.stencicrity` are used by both
harnesses, copied into a scratch directory so neither writes into the working
tree. The inputs are 8 zips → 8 projects → 16 sides → 40 classified
copper/paste/outline layers (1,568,188 characters of RS-274X) → 5,472 graphic
objects → 685 pads. 13 sides are relevant (have paste openings or pads to
decide on) and 12 of those are enabled, so `pack` places 12 cells and lays 757
border dots on a 380 x 280 mm sheet.

Each stage is called through the same library function the CLI calls, timed
with `std::time::Instant` (rust) and `time.perf_counter` (python), **10
repetitions after 2 warm-ups**; the tables give the median, plus the minimum
and the sample standard deviation over the ten. Page cache warm throughout.
The rust side is a `--release` build (`lto = "thin"`, `codegen-units = 1`).
Memory is the peak RSS of one process — `/proc/self/status:VmHWM` (rust),
`resource.getrusage` (python), `/usr/bin/time -v` for the whole run.

Both implementations are single-threaded; nothing here uses more than one core.

Machine: Intel Core Ultra 9 285K (24 cores), 30 GiB RAM, Linux 7.2.4-3-cachyos,
Python 3.14.7, rustc 1.98.1. Library versions: shapely 2.1.2 (GEOS 3.14.1),
Pillow 12.3.0; geo 0.33.1, geo-types 0.7.20, i_overlay 9.0.0, rstar 0.13.0,
tiny-skia 0.12.0. Package versions: pcbstencil 0.1.5, stencicrity 0.2.0.

## Processing

| Stage | Python median | Rust median | Speed-up | Python min / sd | Rust min / sd |
| --- | --- | --- | --- | --- | --- |
| gerber parsing, 40 layers (text → objects) | 62.44 ms | 7.68 ms | 8.1x | 61.95 ms / 1.17 ms | 7.60 ms / 108 us |
| `object_geometry`, all 5,472 objects | 186.09 ms | 8.98 ms | 20.7x | 185.26 ms / 469 us | 8.89 ms / 57 us |
| `discover_projects`, 8 zips (read + parse + assemble) | 73.20 ms | 15.83 ms | 4.6x | 69.43 ms / 13.91 ms | 15.64 ms / 145 us |
| `detect_pads`, 16 sides (incl. the paste overlap test) | 86.35 ms | 6.05 ms | 14.3x | 85.57 ms / 474 us | 6.00 ms / 28 us |

`discover_projects` is the noisiest measurement on the python side (sd 13.9 ms,
19% of the median) because it is the one that goes through zlib and the file
system; the rust spread is 1% of its median.

## Layout

| Stage | Python median | Rust median | Speed-up | Python min / sd | Rust min / sd |
| --- | --- | --- | --- | --- | --- |
| `pack`, 13 sides, datum `slots` (default) | 1.56 ms | 161 us | 9.7x | 1.53 ms / 34 us | 152 us / 5 us |
| `pack`, 13 sides, datum `holes` | 1.92 ms | 197 us | 9.8x | 1.88 ms / 48 us | 187 us / 10 us |
| `pack`, 13 sides, datum `none` | 1.05 ms | 136 us | 7.7x | 1.02 ms / 35 us | 131 us / 5 us |
| `layout_report` | 274 us | 107 us | 2.6x | 267 us / 6 us | 104 us / 2 us |

Packing is irrelevant to the wall clock in both implementations — a millisecond
and a half at worst. The `holes` datum costs ~25% more than `slots` in both
(four holes per cell snapped to the 8 mm grid), and `none` is the cheapest
because no datum features are generated and no divider dots are dropped.

## Rendering and writing

| Stage | Python median | Rust median | Speed-up | Python min / sd | Rust min / sd |
| --- | --- | --- | --- | --- | --- |
| `render_preview`, 20 px/mm → 7834 x 5859 px | 838.63 ms | 220.22 ms | 3.8x | 817.34 ms / 12.66 ms | 214.67 ms / 2.62 ms |
| `render_preview`, 40 px/mm capped to 12000 px → 12000 x 8972 px | 1.479 s | 479.88 ms | 3.1x | 1.474 s / 5.95 ms | 467.42 ms / 6.13 ms |
| paste gerber, `GerberWriter` over all 12 areas | 7.71 ms | 3.54 ms | 2.2x | 7.51 ms / 140 us | 3.52 ms / 13 us |

The paste gerber is 110,678 bytes and identical from both. The PNGs are not:
python writes 1,862,262 / 3,973,132 bytes, rust 1,042,366 / 2,349,402 bytes for
the two sizes — see the caveats.

## Memory

Peak RSS of one process, measured at increasing amounts of the pipeline
(N = 1, no warm-ups, so each figure is a cold process).

| Probe | Python | Rust | Ratio |
| --- | --- | --- | --- |
| interpreter / binary start-up (`--help`) | 35 MiB | 4.3 MiB | 8.2x |
| discovery + pad detection + `pack`, no preview | 62 MiB | 15 MiB | 4.2x |
| … plus one 7834 x 5859 preview | 501 MiB | 367 MiB | 1.4x |
| … plus one 12000 x 8972 preview | 1072 MiB | 839 MiB | 1.3x |

The parsed geometry of all eight boards costs python ~27 MiB above its
interpreter floor and rust ~11 MiB. Everything above that is the preview
bitmap: a 12000 x 8972 RGBA pixmap is 411 MiB on its own, and both
implementations hold roughly two of those (canvas plus the encoder's or the
compositor's working copy) at the peak.

## Whole run

`--batch --no-open` in a directory with the eight zips and the config,
10 repetitions after 2 warm-ups, via `/usr/bin/time -v`.

| Measure | Python | Rust | Ratio |
| --- | --- | --- | --- |
| wall clock, median | 2.11 s | 0.52 s | 4.1x |
| wall clock, min | 2.06 s | 0.50 s | |
| wall clock, sd | 0.019 s | 0.007 s | |
| cpu (user + sys), median | 2.10 s | 0.51 s | 4.1x |
| peak RSS, median | 508 MiB | 370 MiB | 1.4x |

## Discussion

**Rendering dominates both.** A batch run renders the preview *twice* — once
after the first `pack` (step 4 of the pipeline, so the user sees something
before the TUI would open) and once after the final pack in step 7. At 20 px/mm
that is 2 x 839 ms = 1.68 s of python's 2.11 s (80%) and 2 x 220 ms = 440 ms of
rust's 520 ms (85%). Everything else — discovery, pad detection, packing,
writing three gerbers and zipping them — is 0.4 s in python and 0.08 s in rust.
If a run has to get faster, the second render is the thing to remove, in either
language.

**Where rust gains most.** The two stages that are pure geometry — building a
shapely/`geo` polygon per graphic object, and testing every copper pad against
the paste openings through an R-tree — are 21x and 14x. Both are per-object
python interpreter overhead plus a GEOS call per object, against monomorphised
`geo` code with no FFI boundary and no allocation per call. Parsing is "only"
8x because it is string handling in both, and python's regex and `str` slicing
are C code already. Discovery at 4.6x is held back by zlib inflation, which
both delegate to the same kind of C/rust deflate implementation.

**Where the gain is smallest.** `GerberWriter` (2.2x) and `layout_report`
(2.6x) are string formatting; python's `str.join` and `%`-formatting are close
to memcpy, and rust's `format!` is not dramatically better. `render_preview`
(3.1–3.8x) is bounded by the per-pixel work, which both push into compiled code
(Pillow's C rasteriser, tiny-skia's). Nothing measured is slower in rust.

**The preview PNG is the one output that differs.** Python rasterises with
`ImageDraw` into 8-bit class masks and composites them — aliased edges; rust
fills anti-aliased paths straight into an RGBA pixmap. So the rust renderer
does strictly more work per edge pixel and is still 3.8x faster, but the two
PNGs are not pixel-identical and the encoders make different filter/compression
choices — the rust file is 56% of the python one at 20 px/mm. The gerbers,
which are what gets ordered, *are* identical.

**The 12000 px cap.** `render_preview` reduces `px_per_mm` until the longer
side fits `max_px`, so asking for 40 px/mm on a 380 mm sheet gives a 12000 x
8972 image, not 15200 px. That is 2.35x the pixels of the default and costs
2.18x the time in rust and 1.76x in python — rendering is close to pixel-bound
in both, with python's fixed per-shape cost (one interpreter loop per polygon,
whatever the resolution) paying for a larger share of the small image. It is
also where the peak memory of the whole tool lives.

**First run vs warm.** The numbers above are warm. A cold process pays more,
and unevenly: `detect_pads` takes 32 ms on its first repetition in rust against
6 ms warm (allocator and page faults), and 189 ms against 86 ms in python
(GEOS/STRtree first-touch on top of that). `render_preview` shows no cold
penalty worth reporting — it allocates its pixmap either way. Python also pays
a fixed ~70 ms of interpreter and import start-up per invocation (shapely alone
is 39 ms, Pillow 13 ms) that rust does not; that is 3% of its batch run.

**Caveats.** Single machine, single run of each harness, desktop under light
load; the sd columns are the honest bound on how much to trust the third digit.
`/usr/bin/time` resolves the wall clock to 10 ms, which is 2% of the rust run.
The stage harnesses time 13 sides through `pack` while the batch CLI drops
`PhotoAmp bottom` (no openings at all) and places 11 — the stage numbers are
therefore a hair above what the CLI does, in both implementations equally.

## Reproducing

The rust harness is a cargo example in this crate and stays with it:

```sh
cargo run --release --example bench                     # 10 reps, 2 warm-ups
STENCIL_BENCH_DATA=/path/to/zips \
STENCIL_BENCH_N=20 cargo run --release --example bench  # other inputs
STENCIL_BENCH_ONLY=detect_pads,pack_slots \
STENCIL_BENCH_N=1 STENCIL_BENCH_WARM=0 \
    cargo run --release --example bench                 # memory probe
```

It expects the example zips and a `.stencicrity` in `STENCIL_BENCH_DATA`
(default: the scratch copy this document was measured from) and prints one TSV
line per repetition plus `#` lines with the input sizes and the peak RSS.

The python harness (`bench_py.py`), the aggregator that turns both TSVs into
the tables above (`aggregate.py`) and the raw results (`raw-python.tsv`,
`raw-rust.tsv`, `raw-wholerun.tsv`, `raw-startup.tsv`, `raw-memory.tsv`,
`raw-coldwarm.tsv`, `machine.txt`) live in the scratch directory
`scratchpad/bench/`. **The python harness stops working the moment the
`pcbstencil` package is removed** — it imports it directly. The raw TSVs and
the tables in this document are the record; the python column cannot be
re-measured afterwards.

The whole-run figures come from a copy of the example directory per
implementation:

```sh
cd <copy>/ && /usr/bin/time -v stencicrity --batch --no-open
cd <copy>/ && /usr/bin/time -v python3 stencicrity.py --batch --no-open
```
