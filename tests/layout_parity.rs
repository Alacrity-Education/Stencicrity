//! Real-data parity of `layout::pack` / `layout::layout_report` against the
//! python original.
//!
//! The reference is produced by `dump_reference.py` in the scratchpad (see
//! `REFERENCE`): it discovers the example gerber sets next to the repository,
//! detects their pads and dumps, for a list of parameter sets, the board
//! bounding boxes plus the whole resulting layout (areas, datum features,
//! dividers, dots, block, fits, heuristic) and the report text.
//!
//! This test rebuilds the sides from the dumped bounding boxes - the packer
//! only ever looks at those, at `enabled`, `mirror` and at the names - and
//! asserts that the rust layout is identical. It is data gated: without the
//! JSON file it prints a note and passes, so the suite still runs on a machine
//! that has no python, no shapely and no example gerbers.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;
use stencicrity::gerber::{Aperture, Flash};
use stencicrity::layout::{layout_report, pack};
use stencicrity::model::{
    all_sides, Config, LayoutParams, Pad, Project, Side, DATUM_SLOTS, SIDE_TOP, STATE_IGNORE,
    STATE_OPEN, STATE_UNDEFINED,
};

/// Where `dump_reference.py` leaves its JSON.
const REFERENCE: &str = concat!(
    "/tmp/claude-2017/-home-alex-lucaci-comanda-stencil/",
    "d66c9e56-b68d-4402-9c4d-b6e685a5f7d2/scratchpad/rust-layout/layout_reference.json"
);

/// Every coordinate has to agree to this (mm).
const TOL: f64 = 1e-6;

fn reference() -> Option<Value> {
    let path: PathBuf = std::env::var("STENCICRITY_LAYOUT_REFERENCE")
        .unwrap_or_else(|_| REFERENCE.to_string())
        .into();
    let text = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&text).ok()
}

fn f(v: &Value) -> f64 {
    v.as_f64().expect("number")
}

fn s(v: &Value) -> &str {
    v.as_str().expect("string")
}

fn arr(v: &Value) -> &Vec<Value> {
    v.as_array().expect("array")
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= TOL
}

/// A pad that only carries the state the report counts.
fn dummy_pad(project: &str, side: &str, state: &str, has_paste: bool) -> Pad {
    Pad {
        key: format!("{project}/{side}/X.1@0.000,0.000"),
        project: project.to_string(),
        side: side.to_string(),
        ref_: "X".to_string(),
        pin: "1".to_string(),
        x: 0.0,
        y: 0.0,
        function: String::new(),
        shape: String::new(),
        flash: Flash {
            x: 0.0,
            y: 0.0,
            aperture: std::rc::Rc::new(Aperture::new(10, "C", vec![1.0], Default::default(), None)),
            attrs: Default::default(),
            dark: true,
        },
        geom: geo::MultiPolygon::new(Vec::new()),
        has_paste,
        state: state.to_string(),
        paste_indices: Vec::new(),
    }
}

/// Rebuild the projects of the dump: one project per name, its sides in order.
///
/// Only what `pack` and `layout_report` read is filled in - the bounding box,
/// the names, `mirror`, `enabled`, the pad states and the number of paste
/// objects (which the report prints; that one line is normalised away against
/// the dumped count, see `normalise_paste_lines`).
fn build(sides_json: &[Value]) -> (Vec<Project>, Vec<usize>) {
    // project name -> index, keeping the order the dump has.
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    let mut projects: Vec<Project> = Vec::new();
    // dump order -> (project, side) so a case's `enabled` mask maps back.
    let mut order: Vec<usize> = Vec::new();
    for side in sides_json {
        let name = s(&side["project"]).to_string();
        let bbox = arr(&side["bbox"]);
        let bounds = (f(&bbox[0]), f(&bbox[1]), f(&bbox[2]), f(&bbox[3]));
        let pi = *index.entry(name.clone()).or_insert_with(|| {
            projects.push(Project {
                name: name.clone(),
                source: String::new(),
                outline: None,
                bbox: bounds,
                sides: Vec::new(),
            });
            projects.len() - 1
        });
        let side_name = s(&side["name"]).to_string();
        let states = &side["states"];
        let mut pads = Vec::new();
        for (state, key) in [
            (STATE_OPEN, "open"),
            (STATE_IGNORE, "ignore"),
            (STATE_UNDEFINED, "undefined"),
        ] {
            for _ in 0..states[key].as_u64().unwrap_or(0) {
                pads.push(dummy_pad(&name, &side_name, state, false));
            }
        }
        for _ in 0..side["closed_pads"].as_u64().unwrap_or(0) {
            pads.push(dummy_pad(&name, &side_name, STATE_IGNORE, true));
        }
        projects[pi].sides.push(Side {
            project_name: name,
            board_bbox: bounds,
            name: side_name,
            copper: None,
            paste: None,
            mirror: side["mirror"].as_bool().unwrap(),
            enabled: side["enabled"].as_bool().unwrap(),
            pads,
        });
        order.push(pi);
    }
    (projects, order)
}

/// `layout_report` prints `len(side.paste_objects)`; the rebuilt sides have no
/// gerber, so that line is replaced by the dumped count on both texts.
fn normalise_paste_lines(report: &str, counts: &[usize]) -> String {
    let mut seen = 0usize;
    report
        .lines()
        .map(|line| {
            if line.starts_with("   paste openings: ") {
                let text = format!("   paste openings: {}", counts.get(seen).unwrap_or(&0));
                seen += 1;
                return text;
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn params_of(case: &Value) -> LayoutParams {
    let p = &case["params"];
    LayoutParams {
        gap: f(&p["gap"]),
        datum: s(&p["datum"]).to_string(),
        hole_dia: f(&p["hole_dia"]),
        hole_inset: f(&p["hole_inset"]),
        slot_width: f(&p["slot_width"]),
        slot_length: f(&p["slot_length"]),
        slot_offset: f(&p["slot_offset"]),
        slot_pitch: f(&p["slot_pitch"]),
        slot_web: f(&p["slot_web"]),
        pin_dia: f(&p["pin_dia"]),
        marker: p["marker"].as_bool().unwrap(),
        marker_size: f(&p["marker_size"]),
        dot_dia: f(&p["dot_dia"]),
        dot_pitch: f(&p["dot_pitch"]),
        dot_line_gap: f(&p["dot_line_gap"]),
        dot_clearance: p
            .get("dot_clearance")
            .map(f)
            .unwrap_or(LayoutParams::default().dot_clearance),
        hole_grid: f(&p["hole_grid"]),
        outer_border: p["outer_border"].as_bool().unwrap(),
        sort: s(&p["sort"]).to_string(),
    }
}

fn assert_points(case: &str, what: &str, got: &[(f64, f64)], want: &[Value]) {
    assert_eq!(got.len(), want.len(), "{case}: number of {what}");
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        let w = arr(w);
        assert!(
            close(g.0, f(&w[0])) && close(g.1, f(&w[1])),
            "{case}: {what}[{i}] {:?} != ({}, {})",
            g,
            f(&w[0]),
            f(&w[1])
        );
    }
}

#[test]
fn matches_the_python_layout_on_the_example_gerbers() {
    let Some(data) = reference() else {
        eprintln!("layout_parity: no reference data at {REFERENCE}, skipping");
        return;
    };
    let sides_json = arr(&data["sides"]).clone();
    let (mut projects, order) = build(&sides_json);
    let paste_counts: Vec<usize> = sides_json
        .iter()
        .map(|s| s["paste_objects"].as_u64().unwrap_or(0) as usize)
        .collect();

    let cases = arr(&data["cases"]);
    assert!(!cases.is_empty(), "the reference has no cases");
    for case in cases {
        let name = s(&case["name"]);
        // The dump's `enabled` mask, back onto the rebuilt sides.
        let mask = arr(&case["enabled"]);
        let mut cursor: BTreeMap<usize, usize> = BTreeMap::new();
        for (i, &pi) in order.iter().enumerate() {
            let si = *cursor.entry(pi).and_modify(|c| *c += 1).or_insert(0);
            projects[pi].sides[si].enabled = mask[i].as_bool().unwrap();
        }

        let size = arr(&case["size"]);
        let config = Config {
            size: (
                size[0].as_u64().unwrap() as u32,
                size[1].as_u64().unwrap() as u32,
            ),
            orientation: s(&case["orientation"]).to_string(),
            layout: params_of(case),
            ..Config::default()
        };
        let sides = all_sides(&projects);
        let layout = pack(&projects, &sides, &config);

        // -- the sheet, the block and the packing verdict --------------------
        assert!(close(layout.width, f(&case["width"])), "{name}: width");
        assert!(close(layout.height, f(&case["height"])), "{name}: height");
        let block = arr(&case["block"]);
        for (i, got) in [
            layout.block.0,
            layout.block.1,
            layout.block.2,
            layout.block.3,
        ]
        .iter()
        .enumerate()
        {
            assert!(close(*got, f(&block[i])), "{name}: block[{i}]");
        }
        assert_eq!(layout.fits, case["fits"].as_bool().unwrap(), "{name}: fits");
        assert_eq!(layout.heuristic, s(&case["heuristic"]), "{name}: heuristic");
        assert_eq!(
            layout.overflow,
            case["overflow"].as_u64().unwrap() as usize,
            "{name}: overflow"
        );

        // -- every cell ------------------------------------------------------
        let areas = arr(&case["areas"]);
        assert_eq!(layout.areas.len(), areas.len(), "{name}: number of cells");
        for (i, (got, want)) in layout.areas.iter().zip(areas).enumerate() {
            let side = got.side.get(&projects);
            let tag = format!("{name}: cell {i} ({} {})", side.project_name, side.name);
            assert_eq!(side.project_name, s(&want["project"]), "{tag}: project");
            assert_eq!(side.name, s(&want["side"]), "{tag}: side");
            for (label, g, w) in [
                ("x", got.x, f(&want["x"])),
                ("y", got.y, f(&want["y"])),
                ("w", got.w, f(&want["w"])),
                ("h", got.h, f(&want["h"])),
            ] {
                assert!(close(g, w), "{tag}: {label} {g} != {w}");
            }
            let rect = arr(&want["board_rect"]);
            for (j, g) in [
                got.board_rect.0,
                got.board_rect.1,
                got.board_rect.2,
                got.board_rect.3,
            ]
            .iter()
            .enumerate()
            {
                assert!(close(*g, f(&rect[j])), "{tag}: board_rect[{j}]");
            }
            let tr = arr(&want["transform"]);
            assert_eq!(
                got.transform.mirror,
                tr[0].as_bool().unwrap(),
                "{tag}: mirror"
            );
            assert!(close(got.transform.dx, f(&tr[1])), "{tag}: dx");
            assert!(close(got.transform.dy, f(&tr[2])), "{tag}: dy");
            assert_eq!(
                got.row,
                want["row"].as_u64().unwrap() as usize,
                "{tag}: row"
            );
            assert_eq!(
                got.overflow,
                want["overflow"].as_bool().unwrap(),
                "{tag}: overflow"
            );
            assert_points(&tag, "holes", &got.holes, arr(&want["holes"]));
            assert_points(&tag, "pins", &got.pins, arr(&want["pins"]));
            let slots = arr(&want["slots"]);
            assert_eq!(got.slots.len(), slots.len(), "{tag}: number of slots");
            for (j, (g, w)) in got.slots.iter().zip(slots).enumerate() {
                let w = arr(w);
                assert!(
                    close(g.0, f(&w[0]))
                        && close(g.1, f(&w[1]))
                        && close(g.2, f(&w[2]))
                        && close(g.3, f(&w[3])),
                    "{tag}: slot[{j}] {g:?}"
                );
            }
            let corner = arr(&want["datum_corner"]);
            assert!(
                close(got.datum_corner.0, f(&corner[0]))
                    && close(got.datum_corner.1, f(&corner[1])),
                "{tag}: datum_corner"
            );
            match (got.marker, want["marker"].as_array()) {
                (None, None) => {}
                (Some(m), Some(w)) => assert!(
                    close(m.0, f(&w[0])) && close(m.1, f(&w[1])),
                    "{tag}: marker"
                ),
                (g, w) => panic!("{tag}: marker {g:?} vs {w:?}"),
            }
        }

        // -- dividers and dots ----------------------------------------------
        let dividers = arr(&case["dividers"]);
        assert_eq!(
            layout.dividers.len(),
            dividers.len(),
            "{name}: number of dividers"
        );
        for (i, (g, w)) in layout.dividers.iter().zip(dividers).enumerate() {
            let w = arr(w);
            assert!(
                close(g.0, f(&w[0]))
                    && close(g.1, f(&w[1]))
                    && close(g.2, f(&w[2]))
                    && close(g.3, f(&w[3])),
                "{name}: divider[{i}] {g:?}"
            );
        }
        assert_points(name, "dots", &layout.dots, arr(&case["dots"]));

        // -- the report ------------------------------------------------------
        let report = layout_report(&projects, &layout, &config);
        let want = normalise_paste_lines(s(&case["report"]), &paste_counts);
        let got = normalise_paste_lines(&report, &paste_counts);
        if got != want {
            for (i, (a, b)) in got.lines().zip(want.lines()).enumerate() {
                assert_eq!(a, b, "{name}: report line {}", i + 1);
            }
            panic!(
                "{name}: report length {} != {}",
                got.lines().count(),
                want.lines().count()
            );
        }
    }
}

/// `pack` is called on every keystroke of the TUI layout page, so it has to be
/// quick: 13 cells well under 20 ms (release; a debug build is ~20x slower).
#[test]
fn packs_thirteen_cells_quickly() {
    let mut projects: Vec<Project> = Vec::new();
    for i in 0..13 {
        let w = 20.0 + (i as f64) * 7.5;
        let h = 15.0 + ((i * 5) % 9) as f64 * 6.0;
        projects.push(Project {
            name: format!("p{i}"),
            source: String::new(),
            outline: None,
            bbox: (0.0, 0.0, w, h),
            sides: vec![Side {
                project_name: format!("p{i}"),
                board_bbox: (0.0, 0.0, w, h),
                name: SIDE_TOP.to_string(),
                copper: None,
                paste: None,
                mirror: false,
                enabled: true,
                pads: Vec::new(),
            }],
        });
    }
    let config = Config {
        size: (700, 600),
        layout: LayoutParams {
            datum: DATUM_SLOTS.to_string(),
            ..LayoutParams::default()
        },
        ..Config::default()
    };
    let sides = all_sides(&projects);
    // Warm up, then time a handful of runs.
    let layout = pack(&projects, &sides, &config);
    assert_eq!(layout.areas.len(), 13);
    let start = std::time::Instant::now();
    const RUNS: u32 = 5;
    for _ in 0..RUNS {
        let l = pack(&projects, &sides, &config);
        assert_eq!(l.areas.len(), 13);
    }
    let each = start.elapsed() / RUNS;
    eprintln!("pack of 13 cells: {each:?} each");
    let budget = if cfg!(debug_assertions) { 400 } else { 20 };
    assert!(
        each.as_millis() < budget,
        "pack of 13 cells took {each:?} (budget {budget} ms)"
    );
}
