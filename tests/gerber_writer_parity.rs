//! Parity of the Rust gerber writer with the Python original.
//!
//! For every example gerber the Python `GerberWriter("Paste,Top")` was fed
//! every parsed object under two transforms; the Rust writer must produce the
//! same text, line for line, apart from the `%TF.CreationDate` header. The
//! output is then read back with the Rust parser and its geometry compared
//! with the transformed input.
//!
//! Point `STENCICRITY_GERBER_REF` at the directory holding `gb/` and
//! `rust-gerber/ref/`; without it the test skips.

#[path = "gerber_common/mod.rs"]
mod common;

use common::{kind_of, Ref};
use stencicrity::gerber::{geom_bounds, object_geometry, parse_gerber};
use stencicrity::model::Transform;
use stencicrity::writer::GerberWriter;

/// Areas after a round trip differ by the 1 nm coordinate quantisation.
const RT_AREA_ABS: f64 = 1e-5;
const RT_AREA_REL: f64 = 1e-3;
/// Bounds after a round trip: 1 nm quantisation plus the arc-join residue.
const RT_BOUND: f64 = 1e-5;

fn transforms() -> Vec<(&'static str, Transform)> {
    vec![
        ("plain", Transform::default()),
        (
            "mirror",
            Transform {
                mirror: true,
                dx: 200.0,
                dy: 50.0,
            },
        ),
    ]
}

/// Compare two gerber texts line by line, ignoring the creation timestamp.
fn assert_same_text(got: &str, want: &str, what: &str) {
    let g: Vec<&str> = got.lines().collect();
    let w: Vec<&str> = want.lines().collect();
    for (i, (a, b)) in g.iter().zip(&w).enumerate() {
        if a.starts_with("%TF.CreationDate,") && b.starts_with("%TF.CreationDate,") {
            continue;
        }
        assert_eq!(a, b, "{what}: line {}", i + 1);
    }
    assert_eq!(g.len(), w.len(), "{what}: line count");
    assert!(got.ends_with("M02*\n"), "{what}: missing trailer");
}

#[test]
fn gerber_writer_matches_python() {
    let r = match Ref::open() {
        Some(r) => r,
        None => {
            eprintln!("{}", common::SKIP);
            return;
        }
    };
    let version = r.version();

    let mut files = 0usize;
    let mut lines = 0usize;
    let mut objects = 0usize;
    let mut worst_area: f64 = 0.0;
    let mut worst_bound: f64 = 0.0;

    for (rel, _) in r.files() {
        let text = r.gerber_text(&rel);
        let gf = parse_gerber(&text, &rel).unwrap_or_else(|e| panic!("{rel}: {e}"));

        for (tag, tr) in transforms() {
            let mut w = GerberWriter::new("Paste,Top");
            w.set_software("stencicrity", &version);
            for obj in &gf.objects {
                w.add_object(obj, &tr);
            }
            let got = w.render();
            let want = r.writer_text(&rel, tag);
            assert_same_text(&got, &want, &format!("{rel} [{tag}]"));
            lines += got.lines().count();

            // idempotent
            assert_eq!(got, w.render(), "{rel} [{tag}]: render is not idempotent");

            // ---- read the output back and compare against the input --------
            let back = parse_gerber(&got, "roundtrip")
                .unwrap_or_else(|e| panic!("{rel} [{tag}] reparse: {e}"));
            assert_eq!(
                back.objects.len(),
                gf.objects.len(),
                "{rel} [{tag}]: object count"
            );
            for (i, (src, out)) in gf.objects.iter().zip(&back.objects).enumerate() {
                assert_eq!(kind_of(src), kind_of(out), "{rel} [{tag}][{i}]: kind");
                assert_eq!(src.dark(), out.dark(), "{rel} [{tag}][{i}]: polarity");

                let want_g = tr.apply_geom(&object_geometry(src));
                let got_g = object_geometry(out);
                let (wa, ga) = {
                    use geo::Area;
                    (want_g.unsigned_area(), got_g.unsigned_area())
                };
                let d = (ga - wa).abs();
                assert!(
                    d <= RT_AREA_ABS || d <= RT_AREA_REL * wa.abs(),
                    "{rel} [{tag}][{i}] ({}): area {ga} != {wa}",
                    kind_of(src)
                );
                if wa > 0.0 {
                    worst_area = worst_area.max(d / wa);
                }

                if let (Some(a), Some(b)) = (geom_bounds(&got_g), geom_bounds(&want_g)) {
                    for (x, y) in [a.0, a.1, a.2, a.3].iter().zip([b.0, b.1, b.2, b.3].iter()) {
                        let d = (x - y).abs();
                        worst_bound = worst_bound.max(d);
                        assert!(
                            d <= RT_BOUND,
                            "{rel} [{tag}][{i}] ({}): bounds {a:?} != {b:?}",
                            kind_of(src)
                        );
                    }
                } else {
                    assert_eq!(
                        geom_bounds(&got_g).is_none(),
                        geom_bounds(&want_g).is_none(),
                        "{rel} [{tag}][{i}]: empty mismatch"
                    );
                }
                objects += 1;
            }
        }
        files += 1;
    }

    eprintln!(
        "writer parity: {files} files x {} transforms, {lines} identical lines, \
         {objects} objects round-tripped; worst relative area {worst_area:.3e}, \
         worst bound {worst_bound:.3e}",
        transforms().len()
    );
    assert!(
        files >= 40,
        "expected the full example set, got {files} files"
    );
}

#[test]
fn gerber_writer_primitives_round_trip() {
    let mut w = GerberWriter::new("Paste,Top");
    w.add_circle(1.0, 2.0, 0.6, None);
    w.add_obround(5.0, 5.0, 8.0, 4.5, None);
    w.add_polygon(&[(0.0, 0.0), (3.0, 0.0), (3.0, 2.0), (0.0, 2.0)]);
    w.add_line(-1.0, -1.0, -1.0, 4.0, 0.5);
    w.add_rect_outline(10.0, 10.0, 14.0, 12.0, 0.3);
    let text = w.render();

    let gf = parse_gerber(&text, "written").unwrap();
    // circle + obround + region + 5 strokes
    assert_eq!(gf.objects.len(), 8);
    let total = {
        use geo::Area;
        gf.objects
            .iter()
            .map(|o| object_geometry(o).unsigned_area())
            .sum::<f64>()
    };
    assert!(total > 0.0);
    assert_eq!(gf.apertures.len(), 4); // C0.6, O8x4.5, C0.5, C0.3
    assert_eq!(gf.apertures[&10].describe(), "C ⌀0.60");
    assert_eq!(gf.apertures[&11].describe(), "O 8.00x4.50");
}
