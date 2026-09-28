//! Pad detection on the real gerber sets: totals, the NT/TP rule and ordering.
//!
//! These need the example gerbers next to the crate (the `*.zip` files in the
//! crate's parent directory); without them they print a note and pass.

use std::path::{Path, PathBuf};

use stencicrity::model::{Pad, Project};
use stencicrity::pads::{detect_pads, sorted_pads, state_counts, DetectOptions};
use stencicrity::project::{discover_projects, DiscoverOptions};

/// The directory holding the sample zips (the crate's parent).
fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn have_data() -> bool {
    std::fs::read_dir(data_dir())
        .map(|d| {
            d.filter_map(|e| e.ok())
                .any(|e| e.file_name().to_string_lossy().ends_with(".zip"))
        })
        .unwrap_or(false)
}

fn discover() -> Vec<Project> {
    let cwd = data_dir();
    let opts = DiscoverOptions {
        mirror_bottom: true,
        exclude_dirs: vec![
            cwd.join("stencil-out").to_string_lossy().into_owned(),
            cwd.join("Stencicrity").to_string_lossy().into_owned(),
        ],
    };
    discover_projects(&[], &cwd.to_string_lossy(), &opts, &mut |_| {}).expect("discovery")
}

fn detect_all(projects: &mut [Project]) {
    let opts = DetectOptions::default();
    for project in projects.iter_mut() {
        for side in project.sides.iter_mut() {
            detect_pads(side, &opts);
        }
    }
}

#[test]
fn totals_of_the_sample_projects() {
    if !have_data() {
        eprintln!("skipped: no sample gerbers next to the crate");
        return;
    }
    let mut projects = discover();
    detect_all(&mut projects);

    let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "airbox",
            "alacrity badge",
            "indxworks",
            "PhotoAmp",
            "KliFan",
            "LED lamp for gardening",
            "Midea WiFi Dongle",
            "RBARF",
        ]
    );

    let pads: Vec<&Pad> = projects
        .iter()
        .flat_map(|p| p.sides.iter())
        .flat_map(|s| s.pads.iter())
        .collect();
    assert_eq!(pads.len(), 685, "total pads");
    let candidates: Vec<&&Pad> = pads.iter().filter(|p| p.is_candidate()).collect();
    assert_eq!(candidates.len(), 48, "candidates");

    // every pasted pad knows which paste openings cover it
    assert!(pads
        .iter()
        .filter(|p| p.has_paste)
        .all(|p| !p.paste_indices.is_empty()));

    // the NT/TP rule: 32 ignore, 16 undefined, nothing open by default
    let (undefined, open, ignore) = state_counts(candidates.iter().map(|p| **p));
    assert_eq!((undefined, open, ignore), (16, 0, 32));
}

#[test]
fn sorted_pads_orders_project_side_ref_pin() {
    if !have_data() {
        eprintln!("skipped: no sample gerbers next to the crate");
        return;
    }
    let mut projects = discover();
    detect_all(&mut projects);
    let sides = stencicrity::model::all_sides(&projects);

    let all = sorted_pads(&projects, &sides, true);
    assert_eq!(all.len(), 685);
    let candidates = sorted_pads(&projects, &sides, false);
    assert_eq!(candidates.len(), 48);

    // natural project order, top before bottom, natural ref then pin
    let keys: Vec<(String, String, String, String)> = candidates
        .iter()
        .map(|(id, i)| {
            let pad = &id.get(&projects).pads[*i];
            (
                pad.project.clone(),
                pad.side.clone(),
                pad.ref_.clone(),
                pad.pin.clone(),
            )
        })
        .collect();
    let mut expected = keys.clone();
    expected.sort_by(|a, b| {
        stencicrity::util::natural_key(&a.0)
            .cmp(&stencicrity::util::natural_key(&b.0))
            .then_with(|| (a.1 != "top").cmp(&(b.1 != "top")))
            .then_with(|| {
                stencicrity::util::natural_key(&a.2).cmp(&stencicrity::util::natural_key(&b.2))
            })
            .then_with(|| {
                stencicrity::util::natural_key(&a.3).cmp(&stencicrity::util::natural_key(&b.3))
            })
    });
    assert_eq!(keys, expected);

    // only candidates when all_pads is false
    assert!(candidates
        .iter()
        .all(|(id, i)| id.get(&projects).pads[*i].is_candidate()));
    // an empty side list selects nothing
    assert!(sorted_pads(&projects, &[], true).is_empty());
}
