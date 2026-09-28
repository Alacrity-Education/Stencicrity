//! A whole run over a hand written gerber set.
//!
//! The set is a four pad KiCad-style board written from this file, so the test
//! needs no example zips and works in CI. It goes through the real binary and
//! checks what the run leaves behind: the two merged gerbers (re-parsed with
//! `gerber::parse_gerber`), the zip, the report, the preview and the
//! `.stencicrity` the run wrote.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use stencicrity::gerber::{geom_bounds, object_geometry, parse_gerber};

const BIN: &str = env!("CARGO_BIN_EXE_stencicrity");

/// A KiCad X2 header for one layer of the "demo" project.
fn header(function: &str) -> String {
    format!(
        "%TF.GenerationSoftware,KiCad,Pcbnew,8.0.0*%\n\
         %TF.CreationDate,2024-01-01T00:00:00+00:00*%\n\
         %TF.ProjectId,demo,64656d6f-0000-0000-0000-000000000000,rev?*%\n\
         %TF.FileFunction,{function}*%\n\
         %TF.FilePolarity,Positive*%\n\
         %FSLAX46Y46*%\n\
         G04 Gerber Fmt 4.6, Leading zero omitted, Abs format (unit mm)*\n\
         %MOMM*%\n\
         %LPD*%\n\
         G01*\n"
    )
}

/// A 20 x 10 mm board with four 1.2 x 0.8 mm SMD pads:
/// `R1.1` and `R1.2` carry paste, `TP1.1` is a test point (the `TP` rule makes
/// it "ignore") and `U1.1` is left undefined.
fn write_board(root: &Path) {
    let dir = root.join("demo");
    std::fs::create_dir_all(&dir).expect("mkdir");

    let mut copper = header("Copper,L1,Top");
    copper.push_str("%TA.AperFunction,SMDPad,CuDef*%\n%ADD10R,1.200000X0.800000*%\n%TD*%\nD10*\n");
    for (reference, pin, x, y) in [
        ("R1", "1", 5_000_000, 5_000_000),
        ("R1", "2", 7_000_000, 5_000_000),
        ("TP1", "1", 15_000_000, 5_000_000),
        ("U1", "1", 10_000_000, 3_000_000),
    ] {
        copper.push_str(&format!("%TO.P,{reference},{pin}*%\nX{x}Y{y}D03*\n"));
    }
    copper.push_str("%TD*%\nM02*\n");
    std::fs::write(dir.join("demo-F_Cu.gbr"), copper).expect("write copper");

    let mut paste = header("Paste,Top");
    paste.push_str("%ADD10R,1.200000X0.800000*%\nD10*\n");
    paste.push_str("X5000000Y5000000D03*\nX7000000Y5000000D03*\n");
    paste.push_str("M02*\n");
    std::fs::write(dir.join("demo-F_Paste.gbr"), paste).expect("write paste");

    let mut edge = header("Profile,NP");
    edge.push_str("%ADD10C,0.100000*%\nD10*\n");
    edge.push_str("X0Y0D02*\nX20000000Y0D01*\nX20000000Y10000000D01*\nX0Y10000000D01*\nX0Y0D01*\n");
    edge.push_str("M02*\n");
    std::fs::write(dir.join("demo-Edge_Cuts.gbr"), edge).expect("write outline");
}

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run stencicrity")
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("exit code")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A small preview keeps the test fast; the geometry does not depend on it.
const BATCH: [&str; 6] = ["--batch", "--no-open", "--px-per-mm", "2", "--out", "./out"];

fn layer(path: &Path) -> stencicrity::gerber::GerberFile {
    let text = std::fs::read_to_string(path).expect("read the merged layer");
    parse_gerber(&text, &path.to_string_lossy()).expect("re-parse the merged layer")
}

fn zip_members(path: &Path) -> Vec<String> {
    let file = std::fs::File::open(path).expect("open the zip");
    let mut archive = zip::ZipArchive::new(file).expect("read the zip");
    let mut names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).expect("member").name().to_string())
        .collect();
    names.sort();
    names
}

#[test]
fn a_batch_run_writes_a_complete_order() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_board(root);

    let out = run(
        root,
        &[
            "--outline",
            BATCH[0],
            BATCH[1],
            BATCH[2],
            BATCH[3],
            BATCH[4],
            BATCH[5],
        ],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("1 project(s) found\n"), "{text}");
    assert!(text.contains("configuration: "), "{text}");
    assert!(text.contains("(new)"), "{text}");
    assert!(
        text.contains("demo                     top     on   no"),
        "{text}"
    );
    assert!(
        text.contains("stencil: 380.0 x 280.0 mm (380x280, landscape), datum: slots"),
        "{text}"
    );
    assert!(text.contains("block:   60.0 x 60.0 mm — fits"), "{text}");
    assert!(
        text.contains("candidate pads (copper without paste): 2 (undefined 1, open 0, ignore 1)"),
        "{text}"
    );
    assert!(text.contains("closed openings: 0"), "{text}");
    assert!(
        text.contains("note: 1 pad(s) are still undefined and got NO opening"),
        "{text}"
    );
    // --batch says what it did with the undefined pad.
    assert!(
        stderr(&out).contains("--batch: 1 undefined pad(s) are treated as closed"),
        "{}",
        stderr(&out)
    );

    // -- the files -------------------------------------------------------- //
    let out_dir: PathBuf = root.join("out");
    let paste = out_dir.join("stencil-F_Paste.gbr");
    let copper = out_dir.join("stencil-F_Cu.gbr");
    let edge = out_dir.join("stencil-Edge_Cuts.gbr");
    let zip = out_dir.join("stencil.zip");
    let preview = out_dir.join("stencil-preview.png");
    let report = out_dir.join("stencil-report.txt");
    for path in [&paste, &copper, &edge, &zip, &preview, &report] {
        assert!(path.exists(), "{} was not written", path.display());
    }
    assert_eq!(
        zip_members(&zip),
        vec![
            "stencil-Edge_Cuts.gbr".to_string(),
            "stencil-F_Cu.gbr".to_string(),
            "stencil-F_Paste.gbr".to_string(),
        ]
    );
    // A PNG, and one the renderer really produced.
    let png = std::fs::read(&preview).expect("read the preview");
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");

    // -- the merged layers re-parse --------------------------------------- //
    let paste_layer = layer(&paste);
    assert_eq!(paste_layer.file_function(), "Paste,Top");
    // two source openings + the divider dots + four alignment slots + the two
    // marker strokes; nothing for the pads (both candidates stay closed).
    let slots = paste_layer
        .objects
        .iter()
        .filter(|o| match o {
            stencicrity::gerber::GraphicObject::Flash(f) => f.aperture.template == "O",
            _ => false,
        })
        .count();
    assert_eq!(
        slots, 4,
        "one slot per jig raster position on the cell edges"
    );
    let openings = paste_layer
        .objects
        .iter()
        .filter(|o| match o {
            stencicrity::gerber::GraphicObject::Flash(f) => f.aperture.template == "R",
            _ => false,
        })
        .count();
    assert_eq!(openings, 2, "the two pasted pads");
    let strokes = paste_layer
        .objects
        .iter()
        .filter(|o| matches!(o, stencicrity::gerber::GraphicObject::Stroke(_)))
        .count();
    assert_eq!(strokes, 2, "the two strokes of the orientation X");
    let (minx, miny, maxx, maxy) = paste_layer.bounds().expect("paste bounds");
    assert!(minx > 150.0 && maxx < 220.0, "{minx} {maxx}");
    assert!(miny > 110.0 && maxy < 180.0, "{miny} {maxy}");

    let copper_layer = layer(&copper);
    assert_eq!(copper_layer.file_function(), "Copper,L1,Top");
    assert_eq!(copper_layer.objects.len(), 4, "every pad of the board");
    for obj in &copper_layer.objects {
        let area = geo::Area::unsigned_area(&object_geometry(obj));
        assert!((area - 1.2 * 0.8).abs() < 1e-6, "{area}");
    }

    let edge_layer = layer(&edge);
    assert_eq!(edge_layer.file_function(), "Profile,NP");
    let (x0, y0, x1, y1) =
        geom_bounds(&object_geometry(&edge_layer.objects[0])).expect("outline bounds");
    assert!(
        x0 < 0.06 && y0 < 0.06 && x1 > 0.0 && y1 > 0.0,
        "{x0} {y0} {x1} {y1}"
    );
    let (_, _, ex, ey) = edge_layer.bounds().expect("edge bounds");
    assert!(
        (ex - 380.05).abs() < 1e-6 && (ey - 280.05).abs() < 1e-6,
        "{ex} {ey}"
    );

    // -- the report and the configuration --------------------------------- //
    let report_text = std::fs::read_to_string(&report).expect("read the report");
    assert!(
        report_text.starts_with("Stencil: 380.0 x 280.0 mm (380x280 landscape)\n"),
        "{}",
        &report_text[..80]
    );
    assert!(
        report_text.contains("1. demo top   [cell 0]"),
        "{report_text}"
    );
    assert!(report_text.contains("\nfiles:\n"), "{report_text}");
    assert!(report_text.ends_with("got NO opening\n"), "{report_text}");

    let config = std::fs::read_to_string(root.join(".stencicrity")).expect("read the config");
    assert!(config.contains("demo/top = on"), "{config}");
    assert!(
        config.contains("demo/top/TP1.1@15.000,5.000 = ignore"),
        "{config}"
    );
    assert!(
        config.contains("demo/top/U1.1@10.000,3.000 = undefined"),
        "{config}"
    );

    // -- a second run reads what the first one wrote ----------------------- //
    let again = run(root, &BATCH);
    assert_eq!(code(&again), 0, "{}", stderr(&again));
    assert!(
        stdout(&again).contains("(1 side switch(es), 2 pad decision(s))"),
        "{}",
        stdout(&again)
    );
}

#[test]
fn an_open_pad_is_cut_and_open_shrink_shrinks_it() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_board(root);
    // The first run writes a configuration with both candidates closed.
    assert_eq!(code(&run(root, &BATCH)), 0);
    let path = root.join(".stencicrity");
    let config = std::fs::read_to_string(&path).expect("read the config");
    let opened = config
        .replace("U1.1@10.000,3.000 = undefined", "U1.1@10.000,3.000 = open")
        .replace("TP1.1@15.000,5.000 = ignore", "TP1.1@15.000,5.000 = open");
    std::fs::write(&path, opened).expect("write the config");

    // Without a shrink the pad's own flash is re-used: a rectangle aperture.
    let out = run(root, &BATCH);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(
        stdout(&out).contains("(undefined 0, open 2, ignore 0)"),
        "{}",
        stdout(&out)
    );
    let plain = layer(&root.join("out").join("stencil-F_Paste.gbr"));
    let rects = plain
        .objects
        .iter()
        .filter(|o| match o {
            stencicrity::gerber::GraphicObject::Flash(f) => f.aperture.template == "R",
            _ => false,
        })
        .count();
    assert_eq!(rects, 4, "two source openings plus the two opened pads");
    assert!(
        !plain
            .objects
            .iter()
            .any(|o| matches!(o, stencicrity::gerber::GraphicObject::Region(_))),
        "no regions without --open-shrink"
    );

    // With one, the opened pads become regions 0.1 mm smaller in each axis.
    let mut args = BATCH.to_vec();
    args.extend(["--open-shrink", "0.05"]);
    let out = run(root, &args);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let shrunk = layer(&root.join("out").join("stencil-F_Paste.gbr"));
    let regions: Vec<_> = shrunk
        .objects
        .iter()
        .filter(|o| matches!(o, stencicrity::gerber::GraphicObject::Region(_)))
        .collect();
    assert_eq!(regions.len(), 2, "one region per opened pad");
    for region in regions {
        let geom = object_geometry(region);
        let (x0, y0, x1, y1) = geom_bounds(&geom).expect("region bounds");
        assert!((x1 - x0 - 1.1).abs() < 1e-6, "width {}", x1 - x0);
        assert!((y1 - y0 - 0.7).abs() < 1e-6, "height {}", y1 - y0);
        let area = geo::Area::unsigned_area(&geom);
        assert!((area - 1.1 * 0.7).abs() < 1e-6, "area {area}");
    }
}

#[test]
fn switching_every_side_off_generates_nothing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_board(root);
    let mut args = BATCH.to_vec();
    args.extend(["--only", "nothing-matches-this"]);
    let out = run(root, &args);
    assert_eq!(code(&out), 2);
    let err = stderr(&out);
    assert!(
        err.contains("every side is switched off, nothing to preview"),
        "{err}"
    );
    assert!(
        err.contains("error: no enabled side has a single opening, nothing to generate"),
        "{err}"
    );
    assert!(!root.join("out").join("stencil.zip").exists());
}

#[test]
fn a_legacy_configuration_is_migrated() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_board(root);
    std::fs::write(
        root.join("stencil.stencil"),
        "[pads]\ndemo/top/U1.1@10.000,3.000 = open\n",
    )
    .expect("write the legacy file");

    let out = run(root, &BATCH);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains(&format!(
            "note: migrated {} to .stencicrity",
            root.join("stencil.stencil").display()
        )),
        "{text}"
    );
    // The decision from the old file survived into the new one ...
    assert!(text.contains("(undefined 0, open 1, ignore 1)"), "{text}");
    let config = std::fs::read_to_string(root.join(".stencicrity")).expect("read the config");
    assert!(
        config.contains("demo/top/U1.1@10.000,3.000 = open"),
        "{config}"
    );
    // ... and the old file is left alone.
    assert!(root.join("stencil.stencil").exists());
}

#[test]
fn an_explicit_config_path_is_used_as_given() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    write_board(root);
    let mut args = BATCH.to_vec();
    args.extend(["--config", "./decisions.cfg"]);
    let out = run(root, &args);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert!(root.join("decisions.cfg").exists());
    assert!(!root.join(".stencicrity").exists());
    assert!(
        stdout(&out).contains("configuration: ./decisions.cfg (new)"),
        "{}",
        stdout(&out)
    );
}
