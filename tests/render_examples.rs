//! End-to-end preview of a real gerber set, for eyeballing and for timing.
//!
//! Ignored by default because it needs gerber inputs that are not part of the
//! repository.  Point it at a directory of KiCad zips (and, optionally, a
//! `.stencicrity` next to them) and run it explicitly:
//!
//! ```text
//! STENCICRITY_EXAMPLES=/path/to/zips \
//! STENCICRITY_PREVIEW=/tmp/preview.png \
//!     cargo test --release --test render_examples -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use std::time::Instant;

use stencicrity::config::{apply_config, load_config, CONFIG_FILENAME};
use stencicrity::layout::pack;
use stencicrity::model::{all_sides, Config};
use stencicrity::pads::{detect_pads, DetectOptions};
use stencicrity::project::{discover_projects, DiscoverOptions};
use stencicrity::render::{render_preview, RenderOptions};

#[test]
#[ignore = "needs a directory of gerber zips in STENCICRITY_EXAMPLES"]
fn render_the_example_set() {
    let Ok(dir) = std::env::var("STENCICRITY_EXAMPLES") else {
        eprintln!("STENCICRITY_EXAMPLES is not set, skipping");
        return;
    };
    let out = PathBuf::from(
        std::env::var("STENCICRITY_PREVIEW").unwrap_or_else(|_| "stencil-preview.png".to_string()),
    );
    let ppmm: f64 = std::env::var("STENCICRITY_PPMM")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20.0);

    let mut warn = |m: &str| eprintln!("warning: {m}");

    let started = Instant::now();
    let mut projects = discover_projects(
        &[],
        &dir,
        &DiscoverOptions {
            mirror_bottom: true,
            exclude_dirs: vec![],
        },
        &mut warn,
    )
    .expect("discover");
    assert!(!projects.is_empty(), "no gerber projects found in {dir}");
    let discovered = started.elapsed();

    let config_path = PathBuf::from(&dir).join(CONFIG_FILENAME);
    let config: Config = if config_path.is_file() {
        load_config(&config_path, &mut warn).expect("config")
    } else {
        Config::default()
    };

    let detecting = Instant::now();
    let opts = DetectOptions {
        ignore_prefixes: config.ignore_prefixes.clone(),
        ..DetectOptions::default()
    };
    for project in &mut projects {
        for side in &mut project.sides {
            detect_pads(side, &opts);
        }
    }
    apply_config(&config, &mut projects);
    let detected = detecting.elapsed();

    let sides: Vec<_> = all_sides(&projects)
        .into_iter()
        .filter(|id| {
            let side = id.get(&projects);
            side.enabled && side.is_relevant() && side.has_openings()
        })
        .collect();
    assert!(!sides.is_empty(), "no side has an opening");

    let packing = Instant::now();
    let layout = pack(&projects, &sides, &config);
    let packed = packing.elapsed();

    let rendering = Instant::now();
    let render_opts = RenderOptions {
        px_per_mm: ppmm,
        title: "stencil".to_string(),
        ..RenderOptions::default()
    };
    render_preview(&projects, &layout, &out, &render_opts).expect("render");
    let rendered = rendering.elapsed();

    let pm = tiny_skia::Pixmap::load_png(&out).expect("a valid png");
    println!(
        "{} project(s), {} cell(s) -> {} ({}x{} px)\n  \
         discover {:.2}s   detect {:.2}s   pack {:.2}s   render {:.2}s   total {:.2}s",
        projects.len(),
        layout.areas.len(),
        out.display(),
        pm.width(),
        pm.height(),
        discovered.as_secs_f64(),
        detected.as_secs_f64(),
        packed.as_secs_f64(),
        rendered.as_secs_f64(),
        started.elapsed().as_secs_f64(),
    );
    assert!(pm.width() > 100 && pm.height() > 100);
}
