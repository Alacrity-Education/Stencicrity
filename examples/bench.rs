//! Stage timings for the rust library, for `docs/benchmarks.md`.
//!
//! Runs the same stages the batch CLI runs, on the example gerber sets, and
//! prints one TSV line per repetition:
//!
//! ```text
//! rust<TAB><stage><TAB><rep><TAB><seconds>
//! ```
//!
//! plus `#` lines with the input sizes and the peak RSS of the process. The
//! python counterpart (`scratchpad/bench/bench_py.py`) prints the same format,
//! and `aggregate.py` turns both into the tables in the document.
//!
//! ```sh
//! cargo run --release --example bench
//! STENCIL_BENCH_DATA=/path/to/zips STENCIL_BENCH_N=10 cargo run --release --example bench
//! ```
//!
//! Environment:
//!   `STENCIL_BENCH_DATA`  directory with the example `*.zip` and `.stencicrity`
//!   `STENCIL_BENCH_OUT`   scratch directory for the png/gerber the stages write
//!   `STENCIL_BENCH_N`     repetitions (default 10)
//!   `STENCIL_BENCH_WARM`  warm-up repetitions (default 2)
//!   `STENCIL_BENCH_ONLY`  comma separated stage names; the rest are skipped
//!                         (used for the memory probe, which must not hold on
//!                         to the parsed layers or allocate a preview pixmap)

use std::path::{Path, PathBuf};
use std::time::Instant;

use stencicrity::config::{apply_config, load_config};
use stencicrity::gerber::{object_geometry, parse_gerber, GraphicObject};
use stencicrity::layout::{layout_report, marker_strokes, pack};
use stencicrity::model::{
    all_sides, Config, Layout, Project, SideId, DATUM_HOLES, DATUM_NONE, DATUM_SLOTS,
};
use stencicrity::pads::{detect_pads, DetectOptions};
use stencicrity::project::{classify_layer, discover_projects, load_source, DiscoverOptions};
use stencicrity::render::{render_preview, RenderOptions};
use stencicrity::writer::GerberWriter;

const DEFAULT_DATA: &str = concat!(
    "/tmp/claude-2017/-home-alex-lucaci-comanda-stencil/",
    "d66c9e56-b68d-4402-9c4d-b6e685a5f7d2/scratchpad/bench/data"
);

fn env_path(key: &str, fallback: &str) -> PathBuf {
    PathBuf::from(std::env::var(key).unwrap_or_else(|_| fallback.to_string()))
}

fn env_usize(key: &str, fallback: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(fallback)
}

/// Peak resident set size of this process (KiB), from `/proc/self/status`.
fn peak_rss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .and_then(|l| l.split_whitespace().nth(1).and_then(|v| v.parse().ok()))
        })
        .unwrap_or(0)
}

/// Stage names to run; empty means all of them (`STENCIL_BENCH_ONLY`).
fn selected() -> Vec<String> {
    std::env::var("STENCIL_BENCH_ONLY")
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn wanted(only: &[String], name: &str) -> bool {
    only.is_empty() || only.iter().any(|s| s == name)
}

/// Time `f` `reps` times after `warm` warm-ups, printing one line per rep.
fn bench<F: FnMut()>(only: &[String], name: &str, warm: usize, reps: usize, mut f: F) {
    if !wanted(only, name) {
        return;
    }
    for _ in 0..warm {
        f();
    }
    for rep in 0..reps {
        let t0 = Instant::now();
        f();
        println!("rust\t{name}\t{rep}\t{:.9}", t0.elapsed().as_secs_f64());
    }
}

/// Every classified copper/paste/outline layer of every example zip, as text.
fn layer_texts(data: &Path) -> Vec<(String, String)> {
    let mut zips: Vec<PathBuf> = std::fs::read_dir(data)
        .expect("data directory")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .map(|e| e.eq_ignore_ascii_case("zip"))
                .unwrap_or(false)
        })
        .collect();
    zips.sort();
    let mut out = Vec::new();
    for zip in zips {
        for (name, text) in load_source(&zip.to_string_lossy()).expect("load_source") {
            if classify_layer(&name, &text).is_some() {
                out.push((name, text));
            }
        }
    }
    out
}

fn discover(data: &Path) -> Vec<Project> {
    discover_projects(
        &[],
        &data.to_string_lossy(),
        &DiscoverOptions {
            mirror_bottom: true,
            exclude_dirs: Vec::new(),
        },
        &mut |_| {},
    )
    .expect("discovery")
}

/// The paste gerber the CLI writes, with `--open-shrink 0` (the default).
fn write_paste(projects: &[Project], layout: &Layout, path: &Path) {
    let mut writer = GerberWriter::new("Paste,Top");
    writer.comment("stencil bench - merged paste layer");
    for area in &layout.areas {
        let side = area.side.get(projects);
        let suffix = if area.transform.mirror {
            " (mirrored)"
        } else {
            ""
        };
        writer.comment(&format!("--- {}{suffix} ---", side.label()));
        for obj in side.active_paste_objects() {
            writer.add_object(obj, &area.transform);
        }
        for pad in side.open_pads() {
            writer.add_object(&GraphicObject::Flash(pad.flash.clone()), &area.transform);
        }
    }
    if !layout.dots.is_empty() {
        writer.comment("--- border dots ---");
        for &(x, y) in &layout.dots {
            writer.add_circle(x, y, layout.params.dot_dia, None);
        }
    }
    if layout.areas.iter().any(|a| !a.slots.is_empty()) {
        writer.comment("--- alignment slots ---");
        for area in &layout.areas {
            for &(cx, cy, w, h) in &area.slots {
                writer.add_obround(cx, cy, w, h, None);
            }
        }
    }
    if layout.areas.iter().any(|a| !a.holes.is_empty()) {
        writer.comment("--- dowel pin holes ---");
        for area in &layout.areas {
            for &(x, y) in &area.holes {
                writer.add_circle(x, y, layout.params.hole_dia, None);
            }
        }
    }
    if layout.areas.iter().any(|a| a.marker.is_some()) {
        writer.comment("--- orientation markers ---");
        for area in &layout.areas {
            let Some(marker) = area.marker else { continue };
            for (x0, y0, x1, y1) in marker_strokes(marker, layout.params.marker_size) {
                writer.add_line(x0, y0, x1, y1, layout.params.dot_dia);
            }
        }
    }
    writer.write(path).expect("write paste");
}

fn png_size(path: &Path) -> (u32, u32) {
    tiny_skia::Pixmap::load_png(path)
        .map(|p| (p.width(), p.height()))
        .unwrap_or((0, 0))
}

fn main() {
    let data = env_path("STENCIL_BENCH_DATA", DEFAULT_DATA);
    let out = env_path(
        "STENCIL_BENCH_OUT",
        &data
            .parent()
            .unwrap_or(Path::new("."))
            .join("out-rust")
            .to_string_lossy(),
    );
    std::fs::create_dir_all(&out).expect("output directory");
    let reps = env_usize("STENCIL_BENCH_N", 10);
    let warm = env_usize("STENCIL_BENCH_WARM", 2);
    let only = selected();

    println!("# data\t{}", data.display());
    println!("# reps\t{reps}\twarmups\t{warm}");
    println!("# version\t{}", stencicrity::VERSION);

    // ---- 1a/1b: parsing and geometry of the raw layers -------------------- //
    if wanted(&only, "parse") || wanted(&only, "geometry") {
        let texts = layer_texts(&data);
        let chars: usize = texts.iter().map(|(_, t)| t.chars().count()).sum();
        println!("# layers\t{}\tchars\t{chars}", texts.len());

        bench(&only, "parse", warm, reps, || {
            for (name, text) in &texts {
                std::hint::black_box(parse_gerber(text, name).expect("parse"));
            }
        });

        let parsed: Vec<_> = texts
            .iter()
            .map(|(name, text)| parse_gerber(text, name).expect("parse"))
            .collect();
        let objects: usize = parsed.iter().map(|g| g.objects.len()).sum();
        println!("# objects\t{objects}");

        bench(&only, "geometry", warm, reps, || {
            for gf in &parsed {
                for obj in &gf.objects {
                    std::hint::black_box(object_geometry(obj));
                }
            }
        });
    }

    // ---- 1c: discovery (zip reading, parsing, project assembly) ----------- //
    bench(&only, "discover", warm, reps, || {
        std::hint::black_box(discover(&data));
    });

    // ---- 1d: pad detection ------------------------------------------------ //
    let mut projects = discover(&data);
    let mut config: Config =
        load_config(&data.join(".stencicrity"), &mut |_| {}).expect("configuration");
    let opts = DetectOptions {
        ignore_prefixes: config.ignore_prefixes.clone(),
        ..DetectOptions::default()
    };
    let n_sides = all_sides(&projects).len();
    println!("# sides\t{n_sides}");

    bench(&only, "detect_pads", warm, reps, || {
        for project in projects.iter_mut() {
            for side in project.sides.iter_mut() {
                detect_pads(side, &opts);
            }
        }
    });

    apply_config(&config, &mut projects);
    let pads: usize = projects
        .iter()
        .flat_map(|p| p.sides.iter())
        .map(|s| s.pads.len())
        .sum();
    println!("# pads\t{pads}");

    // ---- 2: packing and the report ---------------------------------------- //
    let sides: Vec<SideId> = all_sides(&projects)
        .into_iter()
        .filter(|id| id.get(&projects).is_relevant())
        .collect();
    println!("# relevant_sides\t{}", sides.len());

    for (stage, datum) in [
        ("pack_slots", DATUM_SLOTS),
        ("pack_holes", DATUM_HOLES),
        ("pack_none", DATUM_NONE),
    ] {
        config.layout.datum = datum.to_string();
        bench(&only, stage, warm, reps, || {
            std::hint::black_box(pack(&projects, &sides, &config));
        });
    }
    config.layout.datum = DATUM_SLOTS.to_string();

    let layout = pack(&projects, &sides, &config);
    println!(
        "# areas\t{}\tdots\t{}",
        layout.areas.len(),
        layout.dots.len()
    );
    bench(&only, "layout_report", warm, reps, || {
        std::hint::black_box(layout_report(&projects, &layout, &config));
    });

    // ---- 3: rendering and the paste gerber -------------------------------- //
    let preview = out.join("bench-preview.png");
    let capped = out.join("bench-preview-cap.png");

    bench(&only, "render_20ppmm", warm, reps, || {
        render_preview(
            &projects,
            &layout,
            &preview,
            &RenderOptions {
                px_per_mm: 20.0,
                max_px: 12000,
                title: "bench".to_string(),
                ..RenderOptions::default()
            },
        )
        .expect("render");
    });
    if wanted(&only, "render_20ppmm") {
        let (w, h) = png_size(&preview);
        println!(
            "# render_20ppmm_px\t{w}x{h}\tbytes\t{}",
            std::fs::metadata(&preview).map(|m| m.len()).unwrap_or(0)
        );
    }

    bench(&only, "render_cap12000", warm, reps, || {
        render_preview(
            &projects,
            &layout,
            &capped,
            &RenderOptions {
                px_per_mm: 40.0,
                max_px: 12000,
                title: "bench".to_string(),
                ..RenderOptions::default()
            },
        )
        .expect("render");
    });
    if wanted(&only, "render_cap12000") {
        let (w, h) = png_size(&capped);
        println!(
            "# render_cap12000_px\t{w}x{h}\tbytes\t{}",
            std::fs::metadata(&capped).map(|m| m.len()).unwrap_or(0)
        );
    }

    let paste = out.join("bench-F_Paste.gbr");
    bench(&only, "write_paste", warm, reps, || {
        write_paste(&projects, &layout, &paste);
    });
    println!(
        "# paste_bytes\t{}",
        std::fs::metadata(&paste).map(|m| m.len()).unwrap_or(0)
    );

    println!("# peak_rss_kib\t{}", peak_rss_kib());
}
