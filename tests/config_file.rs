//! `.stencicrity` parsing, writing and the object <-> file round trip.
//!
//! Everything here goes through the public `config` API the way the CLI does.
//! The tests that need the example gerber sets (the `*.zip` files next to the
//! crate) print a note and pass when they are not there.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use stencicrity::config::{
    apply_config, collect_config, format_config, load_config, parse_bool, parse_config,
    parse_prefixes, save_config, CONFIG_FILENAME,
};
use stencicrity::model::{
    Config, Project, DATUM_HOLES, DATUM_NONE, DATUM_SLOTS, STATE_IGNORE, STATE_OPEN,
};
use stencicrity::pads::{detect_pads, DetectOptions};
use stencicrity::project::{discover_projects, DiscoverOptions};

// --------------------------------------------------------------------------- //
// Harness
// --------------------------------------------------------------------------- //

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

fn silent() -> impl FnMut(&str) {
    |_: &str| {}
}

// --------------------------------------------------------------------------- //
// Parsing
// --------------------------------------------------------------------------- //

#[test]
fn scalar_settings_round_trip() {
    let mut config = Config {
        size: (520, 420),
        orientation: "portrait".to_string(),
        ..Config::default()
    };
    config.layout.gap = 18.75;
    config.layout.datum = DATUM_HOLES.to_string();
    config.layout.hole_inset = -1.5;
    config.layout.dot_clearance = 0.125;
    config.layout.marker = false;
    config.layout.outer_border = true;
    config.layout.sort = "name".to_string();
    config.ignore_prefixes = vec!["NT".to_string(), "TP".to_string(), "FID".to_string()];
    config.sides.insert("Board/top".to_string(), false);
    config.pads.insert(
        "Board/top/R1.1@1.000,2.000".to_string(),
        STATE_IGNORE.to_string(),
    );

    let text = format_config(&config, &[]);
    assert_eq!(parse_config(&text, &mut silent()), config);
}

#[test]
fn legacy_flat_file_is_read_as_pads() {
    let text = "# an old file\nBoard/top/TP1.1@1.000,2.000 = ignore\nBoard/bottom/R3.2@-4.500,0.000 = open\n";
    let config = parse_config(text, &mut silent());
    assert_eq!(config.pads.len(), 2);
    assert_eq!(config.pads["Board/top/TP1.1@1.000,2.000"], "ignore");
    assert_eq!(config.pads["Board/bottom/R3.2@-4.500,0.000"], "open");
    assert!(config.sides.is_empty());
    assert_eq!(config.layout, Config::default().layout);
}

#[test]
fn legacy_holes_switch_maps_onto_datum() {
    assert_eq!(
        parse_config("[layout]\nholes = on\n", &mut silent())
            .layout
            .datum,
        DATUM_HOLES
    );
    assert_eq!(
        parse_config("[layout]\nholes = off\n", &mut silent())
            .layout
            .datum,
        DATUM_NONE
    );
    // an explicit datum wins whichever order the two come in
    assert_eq!(
        parse_config("[layout]\nholes = on\ndatum = slots\n", &mut silent())
            .layout
            .datum,
        DATUM_SLOTS
    );
    assert_eq!(
        parse_config("[layout]\ndatum = none\nholes = on\n", &mut silent())
            .layout
            .datum,
        DATUM_NONE
    );
    // a bad holes value leaves the datum alone and warns
    let mut warnings = Vec::new();
    let config = parse_config("[layout]\nholes = sometimes\n", &mut |m: &str| {
        warnings.push(m.to_string())
    });
    assert_eq!(config.layout.datum, DATUM_SLOTS);
    assert_eq!(
        warnings,
        vec!["line 2: holes = 'sometimes' is not on/off, using datum slots".to_string()]
    );
}

#[test]
fn aliases_obsolete_keys_and_odd_lines() {
    let mut warnings = Vec::new();
    let text = "[layout]\ngap = 22\nborder = yes\nslot_corner = 4\n\
                [pads]\nA/top/R1.1@0.000,0.000 = open # comment\n\
                nonsense line\n = orphan\n";
    let config = parse_config(text, &mut |m: &str| warnings.push(m.to_string()));
    assert_eq!(config.layout.gap, 22.0);
    assert!(config.layout.outer_border);
    assert_eq!(config.pads["A/top/R1.1@0.000,0.000"], "open");
    assert_eq!(
        warnings,
        vec![
            "line 7: ignoring line without '=': 'nonsense line'".to_string(),
            "line 8: ignoring line without a key".to_string(),
        ]
    );
}

#[test]
fn keys_split_on_the_last_equals() {
    // a pad key may not contain '=', but the split must still be the last one
    let config = parse_config("[sides]\nweird=name/top = off\n", &mut silent());
    assert_eq!(config.sides.get("weird=name/top"), Some(&false));
}

#[test]
fn helpers() {
    assert_eq!(parse_bool("ON"), Some(true));
    assert_eq!(parse_bool("0"), Some(false));
    assert_eq!(parse_bool(""), None);
    assert_eq!(parse_prefixes("tp , nt  fid"), vec!["TP", "NT", "FID"]);
    assert_eq!(parse_prefixes("  "), Vec::<String>::new());
}

fn discovered() -> Vec<Project> {
    let cwd = data_dir();
    let opts = DiscoverOptions {
        mirror_bottom: true,
        exclude_dirs: vec![
            cwd.join("stencil-out").to_string_lossy().into_owned(),
            cwd.join("Stencicrity").to_string_lossy().into_owned(),
        ],
    };
    let mut projects =
        discover_projects(&[], &cwd.to_string_lossy(), &opts, &mut |_| {}).expect("discovery");
    let detect = DetectOptions::default();
    for project in projects.iter_mut() {
        for side in project.sides.iter_mut() {
            detect_pads(side, &detect);
        }
    }
    projects
}

// --------------------------------------------------------------------------- //
// apply / collect / save
// --------------------------------------------------------------------------- //

#[test]
fn apply_and_collect_are_inverse_on_the_sample_projects() {
    if !have_data() {
        eprintln!("skipped: no sample gerbers next to the crate");
        return;
    }
    let mut projects = discovered();
    let mut config = Config::default();
    apply_config(&config, &mut projects);

    // fresh defaults: pasted pads open, NT/TP candidates ignore, rest undefined
    for project in &projects {
        for side in &project.sides {
            assert!(side.enabled);
            for pad in &side.pads {
                if pad.has_paste {
                    assert_eq!(pad.state, STATE_OPEN, "{}", pad.key);
                }
            }
        }
    }

    // a decision survives collect -> format -> parse -> apply
    let first: String = projects
        .iter()
        .flat_map(|p| p.sides.iter())
        .flat_map(|s| s.pads.iter())
        .find(|p| p.is_candidate())
        .map(|p| p.key.clone())
        .expect("at least one candidate");
    for project in projects.iter_mut() {
        for side in project.sides.iter_mut() {
            for pad in side.pads.iter_mut() {
                if pad.key == first {
                    pad.state = STATE_OPEN.to_string();
                }
            }
        }
    }
    collect_config(&mut config, &projects);
    assert_eq!(
        config.pads.get(&first).map(String::as_str),
        Some(STATE_OPEN)
    );
    // pads with paste that stay open are never written
    let pasted: Vec<&String> = config
        .pads
        .keys()
        .filter(|k| {
            projects
                .iter()
                .flat_map(|p| p.sides.iter())
                .flat_map(|s| s.pads.iter())
                .any(|p| &&p.key == k && p.has_paste)
        })
        .collect();
    assert!(
        pasted.is_empty(),
        "open pasted pads must not be stored: {pasted:?}"
    );

    let text = format_config(&config, &projects);
    let back = parse_config(&text, &mut silent());
    apply_config(&back, &mut projects);
    let state = projects
        .iter()
        .flat_map(|p| p.sides.iter())
        .flat_map(|s| s.pads.iter())
        .find(|p| p.key == first)
        .map(|p| p.state.clone());
    assert_eq!(state.as_deref(), Some(STATE_OPEN));
}

#[test]
fn a_closed_pad_is_written_and_read_back() {
    if !have_data() {
        eprintln!("skipped: no sample gerbers next to the crate");
        return;
    }
    let mut projects = discovered();
    let mut config = Config::default();
    apply_config(&config, &mut projects);
    let mut closed = String::new();
    'outer: for project in projects.iter_mut() {
        for side in project.sides.iter_mut() {
            for pad in side.pads.iter_mut() {
                if pad.has_paste {
                    pad.state = STATE_IGNORE.to_string();
                    closed = pad.key.clone();
                    break 'outer;
                }
            }
        }
    }
    assert!(!closed.is_empty());
    collect_config(&mut config, &projects);
    let text = format_config(&config, &projects);
    assert!(text.contains("# --- pads with paste whose opening is closed (state ignore) ---"));
    assert!(text.contains(&format!("{closed} = ignore")));

    let back = parse_config(&text, &mut silent());
    apply_config(&back, &mut projects);
    let pad = projects
        .iter()
        .flat_map(|p| p.sides.iter())
        .flat_map(|s| s.pads.iter())
        .find(|p| p.key == closed)
        .expect("pad");
    assert!(pad.is_closed());
}

#[test]
fn missing_file_gives_the_defaults() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = load_config(&dir.path().join("nothing-here"), &mut silent()).expect("load");
    assert_eq!(config, Config::default());
}

#[test]
fn save_config_writes_atomically_and_keeps_the_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(CONFIG_FILENAME);
    std::fs::write(&path, "[stencil]\nsize = 270x270\n").expect("write");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).expect("chmod");

    let mut config = load_config(&path, &mut silent()).expect("load");
    assert_eq!(config.size, (270, 270));
    config.orientation = "portrait".to_string();
    save_config(&path, &mut config, &[]).expect("save");

    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o640, "the existing mode must survive");
    let back = load_config(&path, &mut silent()).expect("reload");
    assert_eq!(back.size, (270, 270));
    assert_eq!(back.orientation, "portrait");
    // no temporary file was left behind
    let leftovers: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");

    // a brand new file in a directory that does not exist yet
    let fresh = dir.path().join("deeper/still/.stencicrity");
    let mut other = Config::default();
    save_config(&fresh, &mut other, &[]).expect("save fresh");
    assert!(fresh.exists());
}

#[test]
fn stale_entries_survive_a_rewrite() {
    let mut config = Config::default();
    config.sides.insert("Gone/top".to_string(), false);
    config.pads.insert(
        "Gone/top/R9.1@0.000,0.000".to_string(),
        STATE_IGNORE.to_string(),
    );
    let text = format_config(&config, &[]);
    assert_eq!(
        text.matches("# --- not found in the current gerbers ---")
            .count(),
        2
    );
    let back = parse_config(&text, &mut silent());
    assert_eq!(back.sides, config.sides);
    assert_eq!(back.pads, config.pads);

    let expected: BTreeMap<String, bool> = [("Gone/top".to_string(), false)].into_iter().collect();
    assert_eq!(back.sides, expected);
}
