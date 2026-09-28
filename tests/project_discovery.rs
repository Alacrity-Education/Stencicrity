//! Layer classification, source loading and project discovery.
//!
//! The classification tests run everywhere; the rest need the sample gerbers
//! next to the crate (and, for the parity checks, an importable `pcbstencil`)
//! and skip themselves when they are not there.

use std::path::{Path, PathBuf};
use std::process::Command;

use stencicrity::project::{
    classify_layer, discover_projects, is_own_output, load_source, DiscoverOptions,
};

// --------------------------------------------------------------------------- //
// Classification (no data needed)
// --------------------------------------------------------------------------- //

#[test]
fn file_function_header_wins() {
    let copper_top = "%TF.GenerationSoftware,KiCad,Pcbnew,8.0.5*%\n%TF.FileFunction,Copper,L1,Top*%\n%FSLAX46Y46*%\n";
    // the name says bottom paste, the header says top copper
    assert_eq!(
        classify_layer("board-B_Paste.gbr", copper_top),
        Some("copper_top")
    );

    let cases: [(&str, Option<&str>); 9] = [
        ("%TF.FileFunction,Copper,L1,Top*%", Some("copper_top")),
        ("%TF.FileFunction,Copper,L4,Bot*%", Some("copper_bottom")),
        ("%TF.FileFunction,Copper,L2,Inr*%", None),
        ("%TF.FileFunction,SolderPaste,Top*%", Some("paste_top")),
        ("%TF.FileFunction,Paste,Bot*%", Some("paste_bottom")),
        ("%TF.FileFunction,Profile,NP*%", Some("outline")),
        ("%TF.FileFunction,Soldermask,Top*%", None),
        ("%TF.FileFunction,Legend,Top*%", None),
        ("%TF.FileFunction,Copper*%", None),
    ];
    for (header, want) in cases {
        assert_eq!(classify_layer("whatever.gbr", header), want, "{header}");
    }
}

#[test]
fn filename_fallback_patterns() {
    let cases: [(&str, Option<&str>); 14] = [
        ("board-F_Paste.gbr", Some("paste_top")),
        ("board-PasteTop.gbr", Some("paste_top")),
        ("board.GTP", Some("paste_top")),
        ("board-B_Paste.gbr", Some("paste_bottom")),
        ("board-PasteBottom.gbr", Some("paste_bottom")),
        ("board.gbp", Some("paste_bottom")),
        ("board-F_Cu.gbr", Some("copper_top")),
        ("board-CuTop.gbr", Some("copper_top")),
        ("board.gtl", Some("copper_top")),
        ("board-B_Cu.gbr", Some("copper_bottom")),
        ("board.gbl", Some("copper_bottom")),
        ("board-Edge_Cuts.gbr", Some("outline")),
        ("board-outline.gbr", Some("outline")),
        ("board-F_Mask.gbr", None),
    ];
    for (name, want) in cases {
        assert_eq!(classify_layer(name, "G04 no X2 header*\n"), want, "{name}");
    }
    // sub-directories are stripped before matching
    assert_eq!(
        classify_layer("gerbers/sub/board.gtl", "G04*"),
        Some("copper_top")
    );
}

// --------------------------------------------------------------------------- //
// Data-gated helpers
// --------------------------------------------------------------------------- //

fn data_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

fn zips() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(data_dir())
        .map(|d| {
            d.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .map(|e| e.eq_ignore_ascii_case("zip"))
                        .unwrap_or(false)
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

fn python() -> Option<PathBuf> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
    if !repo.join("pcbstencil/project.py").exists() {
        return None;
    }
    let ok = Command::new("python3")
        .arg("-c")
        .arg(format!(
            "import sys; sys.path.insert(0, {:?}); import pcbstencil.project",
            repo.display().to_string()
        ))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    ok.then(|| repo.to_path_buf())
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

/// `<name>\t<length>\t<classified role>` for every loaded member, sorted.
fn source_digest(files: &[(String, String)]) -> Vec<String> {
    let mut lines: Vec<String> = files
        .iter()
        .map(|(name, text)| {
            format!(
                "{name}\t{}\t{}",
                text.chars().count(),
                classify_layer(name, text).unwrap_or("-")
            )
        })
        .collect();
    lines.sort();
    lines
}

const PY_SOURCE_DIGEST: &str = r#"
import os
from pcbstencil.project import load_source, classify_layer
files = load_source(os.environ["STENCIL_SOURCE"])
lines = sorted("%s\t%d\t%s" % (n, len(t), classify_layer(n, t) or "-") for n, t in files.items())
sys.stdout.write("\n".join(lines))
"#;

// --------------------------------------------------------------------------- //
// load_source
// --------------------------------------------------------------------------- //

#[test]
fn load_source_of_the_sample_zips_matches_python() {
    let archives = zips();
    if archives.is_empty() {
        eprintln!("skipped: no sample zips next to the crate");
        return;
    }
    let Some(repo) = python() else {
        eprintln!("skipped: no python3 with pcbstencil importable");
        return;
    };
    for zip in &archives {
        let ours = source_digest(&load_source(&zip.to_string_lossy()).expect("load_source"));
        assert!(!ours.is_empty(), "{}: nothing loaded", zip.display());
        std::env::set_var("STENCIL_SOURCE", zip);
        let theirs: Vec<String> = run_python(&repo, PY_SOURCE_DIGEST)
            .split('\n')
            .map(str::to_string)
            .collect();
        assert_eq!(ours, theirs, "{}", zip.display());
    }
}

#[test]
fn load_source_of_a_directory_equals_the_zip() {
    let archives = zips();
    if archives.is_empty() {
        eprintln!("skipped: no sample zips next to the crate");
        return;
    }
    for zip in archives.iter().take(3) {
        let from_zip = load_source(&zip.to_string_lossy()).expect("load_source zip");
        let dir = tempfile::tempdir().expect("tempdir");
        let file = std::fs::File::open(zip).expect("open zip");
        zip::ZipArchive::new(file)
            .expect("read zip")
            .extract(dir.path())
            .expect("extract");
        let from_dir = load_source(&dir.path().to_string_lossy()).expect("load_source dir");
        assert_eq!(
            source_digest(&from_dir),
            source_digest(&from_zip),
            "{}",
            zip.display()
        );
    }
}

/// Extracted gerber folders, e.g. the ones a previous run left in a scratch
/// directory. Point `STENCICRITY_GERBER_DIR` at their parent to include them.
#[test]
fn load_source_of_extracted_folders() {
    let Ok(root) = std::env::var("STENCICRITY_GERBER_DIR") else {
        eprintln!("skipped: STENCICRITY_GERBER_DIR is not set");
        return;
    };
    let Ok(entries) = std::fs::read_dir(&root) else {
        eprintln!("skipped: {root} cannot be listed");
        return;
    };
    let mut folders: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    folders.sort();
    assert!(!folders.is_empty(), "{root} holds no directories");
    let repo = python();
    for folder in folders {
        let files = load_source(&folder.to_string_lossy()).expect("load_source");
        assert!(
            files.iter().any(|(n, t)| classify_layer(n, t).is_some()),
            "{}: no classifiable gerber",
            folder.display()
        );
        if let Some(repo) = repo.as_ref() {
            std::env::set_var("STENCIL_SOURCE", &folder);
            let theirs: Vec<String> = run_python(repo, PY_SOURCE_DIGEST)
                .split('\n')
                .map(str::to_string)
                .collect();
            assert_eq!(source_digest(&files), theirs, "{}", folder.display());
        }
    }
}

#[test]
fn load_source_rejects_a_plain_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("not-an-archive.bin");
    std::fs::write(&path, b"just some bytes").expect("write");
    let err = load_source(&path.to_string_lossy()).expect_err("must fail");
    assert!(
        err.to_string()
            .contains("is neither a zip archive nor a directory"),
        "{err}"
    );
}

#[test]
fn skipped_members_never_load() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("__MACOSX")).unwrap();
    std::fs::create_dir_all(root.join(".hidden")).unwrap();
    std::fs::create_dir_all(root.join("sub")).unwrap();
    for (path, body) in [
        ("board.gtl", "G04 copper*"),
        ("board.drl", "drill"),
        ("notes.txt", "text"),
        ("stack.json", "{}"),
        ("board.gbrjob", "job"),
        (".dotfile.gbr", "hidden"),
        ("__MACOSX/board.gbr", "resource fork"),
        (".hidden/board.gbr", "hidden dir"),
        ("sub/board.gbp", "G04 paste*"),
    ] {
        std::fs::write(root.join(path), body).unwrap();
    }
    let mut names: Vec<String> = load_source(&root.to_string_lossy())
        .unwrap()
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec!["board.gtl".to_string(), "sub/board.gbp".to_string()]
    );
}

// --------------------------------------------------------------------------- //
// discover_projects
// --------------------------------------------------------------------------- //

fn discover(exclude: Vec<String>) -> Vec<stencicrity::model::Project> {
    let cwd = data_dir();
    let opts = DiscoverOptions {
        mirror_bottom: true,
        exclude_dirs: exclude,
    };
    discover_projects(&[], &cwd.to_string_lossy(), &opts, &mut |_| {}).expect("discovery")
}

fn excluded_outputs() -> Vec<String> {
    let cwd = data_dir();
    vec![
        cwd.join("stencil-out").to_string_lossy().into_owned(),
        cwd.join("Stencicrity").to_string_lossy().into_owned(),
    ]
}

#[test]
fn discovers_the_eight_sample_projects() {
    if zips().is_empty() {
        eprintln!("skipped: no sample zips next to the crate");
        return;
    }
    let projects = discover(excluded_outputs());
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
        ],
        "zips come first in case-insensitive name order, then gerber directories"
    );
    // the GERBER- prefix of a zip name never reaches a project name
    assert!(names.iter().all(|n| !n.starts_with("GERBER-")));
    // every project has at least one side, top before bottom
    for project in &projects {
        assert!(!project.sides.is_empty(), "{}", project.name);
        let sides: Vec<&str> = project.sides.iter().map(|s| s.name.as_str()).collect();
        assert!(
            sides == ["top"] || sides == ["bottom"] || sides == ["top", "bottom"],
            "{sides:?}"
        );
        for side in &project.sides {
            assert_eq!(side.project_name, project.name);
            assert_eq!(side.board_bbox, project.bbox);
            assert_eq!(side.mirror, side.name == "bottom");
            assert!(side.enabled);
        }
        assert!(
            project.width() > 0.0 && project.height() > 0.0,
            "{}",
            project.name
        );
    }
}

const PY_DISCOVER: &str = r#"
import os
from pcbstencil.project import discover_projects
cwd = os.environ["STENCIL_DATA"]
projects = discover_projects(None, cwd, mirror_bottom=True,
                             exclude_dirs=[os.path.join(cwd, "stencil-out"),
                                           os.path.join(cwd, "Stencicrity")],
                             warn=lambda m: None)
lines = []
for p in projects:
    lines.append("%s\t%s\t%.6f\t%.6f\t%.6f\t%.6f\t%s" % (
        p.name, os.path.basename(p.source), *p.bbox,
        ",".join("%s:%d" % (s.name, s.mirror) for s in p.sides())))
sys.stdout.write("\n".join(lines))
"#;

#[test]
fn discovery_matches_python() {
    if zips().is_empty() {
        eprintln!("skipped: no sample zips next to the crate");
        return;
    }
    let Some(repo) = python() else {
        eprintln!("skipped: no python3 with pcbstencil importable");
        return;
    };
    let projects = discover(excluded_outputs());
    let ours: Vec<String> = projects
        .iter()
        .map(|p| {
            let sides: Vec<String> = p
                .sides
                .iter()
                .map(|s| format!("{}:{}", s.name, u8::from(s.mirror)))
                .collect();
            format!(
                "{}\t{}\t{:.6}\t{:.6}\t{:.6}\t{:.6}\t{}",
                p.name,
                Path::new(&p.source).file_name().unwrap().to_string_lossy(),
                p.bbox.0,
                p.bbox.1,
                p.bbox.2,
                p.bbox.3,
                sides.join(",")
            )
        })
        .collect();
    std::env::set_var("STENCIL_DATA", data_dir());
    let theirs: Vec<String> = run_python(&repo, PY_DISCOVER)
        .split('\n')
        .map(str::to_string)
        .collect();
    assert_eq!(ours, theirs);
}

#[test]
fn excluded_directories_are_never_scanned() {
    if zips().is_empty() {
        eprintln!("skipped: no sample zips next to the crate");
        return;
    }
    // without the exclusion the generated stencil output must not become a
    // project either: its gerbers carry our own GenerationSoftware.
    let projects = discover(vec![data_dir()
        .join("Stencicrity")
        .to_string_lossy()
        .into_owned()]);
    assert!(
        projects.iter().all(|p| !p.source.ends_with("stencil-out")),
        "own output was picked up"
    );
}

#[test]
fn own_output_is_recognised() {
    let generated = data_dir().join("stencil-out/stencil-F_Paste.gbr");
    if !generated.exists() {
        eprintln!("skipped: no generated stencil output to check");
        return;
    }
    let text = std::fs::read_to_string(&generated).expect("read");
    let gf = stencicrity::gerber::parse_gerber(&text, "stencil-F_Paste.gbr").expect("parse");
    assert!(is_own_output(&gf));
    // a KiCad file is not our own output
    let kicad = "%TF.GenerationSoftware,KiCad,Pcbnew,8.0.5*%\n%TF.FileFunction,Copper,L1,Top*%\n";
    let other = stencicrity::gerber::parse_gerber(kicad, "x.gbr").expect("parse");
    assert!(!is_own_output(&other));
}

#[test]
fn duplicate_project_names_are_numbered() {
    let archives = zips();
    if archives.is_empty() {
        eprintln!("skipped: no sample zips next to the crate");
        return;
    }
    let source = archives[0].to_string_lossy().into_owned();
    let projects = discover_projects(
        &[source.clone(), source.clone(), source],
        &data_dir().to_string_lossy(),
        &DiscoverOptions {
            mirror_bottom: true,
            exclude_dirs: Vec::new(),
        },
        &mut |_| {},
    )
    .expect("discovery");
    assert_eq!(projects.len(), 3);
    let base = projects[0].name.clone();
    assert_eq!(projects[1].name, format!("{base} (2)"));
    assert_eq!(projects[2].name, format!("{base} (3)"));
    // the sides carry the renamed project
    assert_eq!(projects[2].sides[0].project_name, format!("{base} (3)"));
    assert!(projects[2].sides[0]
        .key()
        .starts_with(&format!("{base} (3)/")));
}

#[test]
fn missing_inputs_warn_and_are_skipped() {
    let mut warnings = Vec::new();
    let projects = discover_projects(
        &["/nonexistent/path/to/a.zip".to_string()],
        &data_dir().to_string_lossy(),
        &DiscoverOptions::default(),
        &mut |m| warnings.push(m.to_string()),
    )
    .expect("discovery");
    assert!(projects.is_empty());
    assert_eq!(
        warnings,
        vec!["/nonexistent/path/to/a.zip: no such file or directory, skipped".to_string()]
    );
}
