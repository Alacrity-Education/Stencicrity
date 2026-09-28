//! Pad detection parity with the python original, on the real gerber sets.
//!
//! The heavy tests need both the sample gerbers next to the crate (the
//! `*.zip` files in the crate's parent directory) and a working `python3`
//! with the `pcbstencil` package importable; they skip themselves otherwise.

use std::path::{Path, PathBuf};
use std::process::Command;

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

/// `python3` with an importable `pcbstencil` (it lives inside the crate root).
fn python() -> Option<PathBuf> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    if !repo.join("pcbstencil/pads.py").exists() {
        return None;
    }
    let ok = Command::new("python3")
        .arg("-c")
        .arg(format!(
            "import sys; sys.path.insert(0, {:?}); import pcbstencil.pads",
            repo.display().to_string()
        ))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if ok {
        Some(repo.to_path_buf())
    } else {
        None
    }
}

fn run_python(repo: &Path, script: &str) -> String {
    let out = Command::new("python3")
        .arg("-c")
        .arg(format!(
            "import sys; sys.path.insert(0, {:?})\n{script}",
            repo.display().to_string()
        ))
        .env("PYTHONIOENCODING", "utf-8")
        .output()
        .expect("python3 must run");
    assert!(
        out.status.success(),
        "python failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
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

/// One canonical line per pad; the python dump below prints exactly the same.
fn pad_line(project: &str, side: &str, pad: &Pad) -> String {
    let indices: Vec<String> = pad.paste_indices.iter().map(|i| i.to_string()).collect();
    format!(
        "{project}\t{side}\t{}\t{}\t{}\t{:.6}\t{:.6}\t{}\t{}\t{}\t{}\t{}",
        pad.key,
        pad.ref_,
        pad.pin,
        pad.x,
        pad.y,
        pad.function,
        pad.shape,
        u8::from(pad.has_paste),
        pad.state,
        indices.join(",")
    )
}

fn rust_dump(projects: &[Project]) -> Vec<String> {
    let mut lines = Vec::new();
    for project in projects {
        for side in &project.sides {
            lines.push(format!(
                "SIDE\t{}\t{}\t{}\t{:.6}\t{:.6}\t{:.6}\t{:.6}",
                project.name,
                side.name,
                side.paste_objects().len(),
                project.bbox.0,
                project.bbox.1,
                project.bbox.2,
                project.bbox.3
            ));
            for pad in &side.pads {
                lines.push(pad_line(&project.name, &side.name, pad));
            }
        }
    }
    lines
}

const PY_DUMP: &str = r#"
from pcbstencil.project import discover_projects
from pcbstencil.pads import detect_pads
import os
cwd = os.environ["STENCIL_DATA"]
projects = discover_projects(None, cwd, mirror_bottom=True,
                             exclude_dirs=[os.path.join(cwd, "stencil-out"),
                                           os.path.join(cwd, "Stencicrity")],
                             warn=lambda m: None)
out = []
for p in projects:
    for s in p.sides():
        detect_pads(s)
        out.append("SIDE\t%s\t%s\t%d\t%.6f\t%.6f\t%.6f\t%.6f"
                   % (p.name, s.name, len(s.paste_objects), *p.bbox))
        for pad in s.pads:
            out.append("\t".join([
                p.name, s.name, pad.key, pad.ref, pad.pin,
                "%.6f" % pad.x, "%.6f" % pad.y, pad.function, pad.shape,
                "1" if pad.has_paste else "0", pad.state,
                ",".join(str(i) for i in pad.paste_indices)]))
sys.stdout.write("\n".join(out))
"#;

#[test]
fn totals_match_the_reference_run() {
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
fn per_pad_parity_with_python() {
    if !have_data() {
        eprintln!("skipped: no sample gerbers next to the crate");
        return;
    }
    let Some(repo) = python() else {
        eprintln!("skipped: no python3 with pcbstencil importable");
        return;
    };
    let mut projects = discover();
    detect_all(&mut projects);
    let ours = rust_dump(&projects);

    std::env::set_var("STENCIL_DATA", data_dir());
    let theirs_text = run_python(&repo, PY_DUMP);
    let theirs: Vec<&str> = theirs_text.split('\n').collect();

    for (i, (a, b)) in ours.iter().zip(theirs.iter()).enumerate() {
        if a == b {
            continue;
        }
        assert!(
            same_but_for_the_shape_text(a, b),
            "line {i} differs\n  rust:   {a}\n  python: {b}"
        );
    }
    assert_eq!(ours.len(), theirs.len(), "line count");
}

/// The aperture description (field 9) is rendered by `gerber::Aperture::describe`
/// with `{:.2}`; a macro shape whose exact size lands on a half-cent boundary
/// (the 1.325 mm wide KiCad RoundRect) may round the other way when the two
/// implementations build the corner circles with different vertices. Everything
/// `pads` itself produces must still be identical, and the size may differ by
/// at most one unit in the last printed decimal.
fn same_but_for_the_shape_text(rust: &str, python: &str) -> bool {
    let (a, b): (Vec<&str>, Vec<&str>) = (rust.split('\t').collect(), python.split('\t').collect());
    if a.len() != b.len() || a.len() < 12 {
        return false;
    }
    if a.iter()
        .zip(b.iter())
        .enumerate()
        .any(|(i, (x, y))| i != 8 && x != y)
    {
        return false;
    }
    let numbers = |shape: &str| -> Option<(String, Vec<f64>)> {
        let (template, rest) = shape.split_once(' ')?;
        let values: Option<Vec<f64>> = rest
            .trim_start_matches('\u{2300}')
            .split('x')
            .map(|v| v.trim_start_matches('\u{2300}').parse().ok())
            .collect();
        Some((template.to_string(), values?))
    };
    match (numbers(a[8]), numbers(b[8])) {
        (Some((ta, va)), Some((tb, vb))) => {
            ta == tb
                && va.len() == vb.len()
                && va
                    .iter()
                    .zip(vb.iter())
                    .all(|(x, y)| (x - y).abs() <= 0.0101)
        }
        _ => false,
    }
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
