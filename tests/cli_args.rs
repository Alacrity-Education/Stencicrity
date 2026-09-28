//! Command line surface of the binary: version, help, and the exit codes of
//! the paths that never reach the pipeline.
//!
//! Everything here drives the real executable, so it also proves that
//! `main` maps the outcome onto the right process exit code.

use std::path::Path;
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_stencicrity");

/// Every long option `cli::Args` defines, `--overrides` included.
const OPTIONS: [&str; 34] = [
    "--out",
    "--name",
    "--config",
    "--size",
    "--landscape",
    "--portrait",
    "--gap",
    "--datum",
    "--holes",
    "--no-holes",
    "--hole-dia",
    "--hole-inset",
    "--slot-width",
    "--slot-length",
    "--slot-offset",
    "--slot-pitch",
    "--slot-web",
    "--pin-dia",
    "--marker",
    "--no-marker",
    "--marker-size",
    "--dot-dia",
    "--dot-pitch",
    "--dot-line-gap",
    "--dot-clearance",
    "--hole-grid",
    "--outer-border",
    "--no-outer-border",
    "--sort",
    "--no-mirror-bottom",
    "--include-tht",
    "--open-shrink",
    "--ignore-prefix",
    "--exclude",
];

fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .current_dir(dir)
        .env("COLUMNS", "200")
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

#[test]
fn version_prints_the_crate_version() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = run(dir.path(), &["--version"]);
    assert_eq!(code(&out), 0);
    assert_eq!(
        stdout(&out).trim_end(),
        format!("stencicrity {}", env!("CARGO_PKG_VERSION"))
    );
    // `-V` is clap's short form and must agree.
    assert_eq!(stdout(&run(dir.path(), &["-V"])), stdout(&out));
}

#[test]
fn help_mentions_every_option() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = run(dir.path(), &["--help"]);
    assert_eq!(code(&out), 0);
    let text = stdout(&out);
    for option in OPTIONS {
        assert!(text.contains(option), "--help does not mention {option}");
    }
    // `--only` would also match `--only` inside "--exclude ... only", so it is
    // checked on its own as a whole word at the start of a column.
    assert!(text.contains("--only <PROJECT[:top|bottom]>"), "{text}");
    for group in [
        "stencil sheet:",
        "layout (mm; from the .stencicrity file):",
        "pads:",
        "selection:",
        "output:",
    ] {
        assert!(
            text.contains(group),
            "--help is missing the {group:?} group"
        );
    }
    for choice in ["270x270", "700x600", "slots", "holes", "none", "height"] {
        assert!(
            text.contains(choice),
            "--help is missing the {choice} choice"
        );
    }
    // The hidden alias of --config is accepted but not advertised.
    assert!(!text.contains("--overrides"));
}

#[test]
fn an_empty_folder_has_no_projects() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = run(dir.path(), &["--batch", "--no-open", "--out", "./out"]);
    assert_eq!(code(&out), 2);
    assert_eq!(
        stderr(&out).trim_end(),
        "error: no gerber projects found (pass zip files or directories)"
    );
    assert_eq!(stdout(&out), "");
}

#[test]
fn a_missing_input_is_a_warning_and_then_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = run(dir.path(), &["nosuch.zip", "--batch", "--no-open"]);
    assert_eq!(code(&out), 2);
    let err = stderr(&out);
    assert!(
        err.contains("nosuch.zip: no such file or directory, skipped"),
        "{err}"
    );
    assert!(err.contains("error: no gerber projects found"), "{err}");
}

#[test]
fn out_of_range_numbers_exit_two() {
    let dir = tempfile::tempdir().expect("tempdir");
    for (args, message) in [
        (["--gap", "-1"], "--gap must not be negative"),
        (["--hole-dia", "0"], "--hole-dia must be positive"),
        (["--slot-width", "-2"], "--slot-width must be positive"),
        (["--slot-length", "0"], "--slot-length must be positive"),
        (
            ["--slot-offset", "-0.1"],
            "--slot-offset must not be negative",
        ),
        (["--slot-pitch", "0"], "--slot-pitch must be positive"),
        (["--slot-web", "0"], "--slot-web must be positive"),
        (["--pin-dia", "0"], "--pin-dia must be positive"),
        (["--marker-size", "0"], "--marker-size must be positive"),
        (["--dot-dia", "0"], "--dot-dia must be positive"),
        (["--dot-pitch", "0"], "--dot-pitch must be positive"),
        (
            ["--dot-line-gap", "-1"],
            "--dot-line-gap must not be negative",
        ),
        (
            ["--dot-clearance", "-1"],
            "--dot-clearance must not be negative",
        ),
        (["--hole-grid", "-1"], "--hole-grid must not be negative"),
        (["--px-per-mm", "0"], "--px-per-mm must be positive"),
        (
            ["--open-shrink", "-0.5"],
            "--open-shrink must not be negative",
        ),
    ] {
        let out = run(dir.path(), &[args[0], args[1], "--batch"]);
        assert_eq!(code(&out), 2, "{args:?}");
        let err = stderr(&out);
        assert!(err.contains(message), "{args:?}: {err}");
        // The range check runs before anything is loaded, so nothing is said
        // about the (empty) folder.
        assert!(!err.contains("no gerber projects"), "{args:?}: {err}");
    }
}

#[test]
fn negative_values_that_are_allowed_get_through() {
    let dir = tempfile::tempdir().expect("tempdir");
    // --hole-inset may be negative; the run then fails on the empty folder.
    let out = run(dir.path(), &["--hole-inset", "-3", "--batch", "--no-open"]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("no gerber projects found"));
}

#[test]
fn bad_choices_exit_two() {
    let dir = tempfile::tempdir().expect("tempdir");
    for args in [
        vec!["--size", "123x456"],
        vec!["--datum", "pins"],
        vec!["--sort", "area"],
        vec!["--gap", "wide"],
        vec!["--nonsense"],
    ] {
        let out = run(dir.path(), &args);
        assert_eq!(code(&out), 2, "{args:?}");
        assert!(stderr(&out).contains("error:"), "{args:?}");
    }
}

#[test]
fn the_mutually_exclusive_groups_exit_two() {
    let dir = tempfile::tempdir().expect("tempdir");
    for args in [
        ["--landscape", "--portrait"],
        ["--marker", "--no-marker"],
        ["--outer-border", "--no-outer-border"],
        ["--holes", "--no-holes"],
    ] {
        let out = run(dir.path(), &args);
        assert_eq!(code(&out), 2, "{args:?}");
        assert!(
            stderr(&out).contains("cannot be used with"),
            "{args:?}: {}",
            stderr(&out)
        );
    }
}

#[test]
fn overrides_is_still_accepted_as_config() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Nothing to discover, so both spellings end in the same error - what
    // matters is that neither is rejected as an unknown option.
    for flag in ["--config", "--overrides"] {
        let out = run(dir.path(), &[flag, "elsewhere.cfg", "--batch", "--no-open"]);
        assert_eq!(code(&out), 2);
        assert!(stderr(&out).contains("no gerber projects found"), "{flag}");
    }
}
