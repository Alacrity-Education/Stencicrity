//! Parity of the Rust gerber reader with the Python original.
//!
//! The reference data is produced by `pcbstencil` (see
//! `scratchpad/rust-gerber/make_refs.py`): for every example gerber it holds
//! the object count per type, each object's geometry area, bounds and `%TO`
//! attributes, every aperture's `describe()` and both `bake_aperture(..).key()`
//! variants, and the file bounds.
//!
//! Point `STENCICRITY_GERBER_REF` at the directory holding `gb/` and
//! `rust-gerber/ref/`; without it the tests skip, so a checkout without the
//! extracted gerbers still passes.

#[path = "gerber_common/mod.rs"]
mod common;

use std::time::Instant;

use common::{approx, bound_tol, bounds_of, kind_of, Ref};
use stencicrity::gerber::{bake_aperture, object_geometry, parse_gerber};

#[test]
fn gerber_reader_matches_python() {
    let r = match Ref::open() {
        Some(r) => r,
        None => {
            eprintln!("{}", common::SKIP);
            return;
        }
    };

    let mut files = 0usize;
    let mut objects = 0usize;
    let mut apertures = 0usize;
    let mut worst_area: f64 = 0.0;
    let mut worst_bound: f64 = 0.0;
    let mut t_parse = 0.0f64;
    let mut t_geom = 0.0f64;

    for entry in r.files() {
        let rel = entry.0.as_str();
        let want = &entry.1;
        let text = r.gerber_text(rel);

        let t0 = Instant::now();
        let gf = parse_gerber(&text, rel).unwrap_or_else(|e| panic!("{rel}: {e}"));
        t_parse += t0.elapsed().as_secs_f64();

        // ---- file attributes ----------------------------------------------
        for (k, v) in want["file_attrs"].as_object().unwrap() {
            assert_eq!(
                gf.file_attrs.get(k).map(String::as_str),
                Some(v.as_str().unwrap()),
                "{rel}: file attribute {k}"
            );
        }

        // ---- object counts -------------------------------------------------
        let wobjs = want["objects"].as_array().unwrap();
        assert_eq!(gf.objects.len(), wobjs.len(), "{rel}: object count");
        for (kind, n) in want["counts"].as_object().unwrap() {
            let got = gf.objects.iter().filter(|o| kind_of(o) == kind).count();
            assert_eq!(got, n.as_u64().unwrap() as usize, "{rel}: {kind} count");
        }

        // ---- per object geometry -------------------------------------------
        let t0 = Instant::now();
        let geoms: Vec<_> = gf.objects.iter().map(object_geometry).collect();
        t_geom += t0.elapsed().as_secs_f64();

        for (i, (obj, g)) in gf.objects.iter().zip(&geoms).enumerate() {
            let w = &wobjs[i];
            assert_eq!(
                kind_of(obj),
                w["k"].as_str().unwrap(),
                "{rel}[{i}]: object kind"
            );

            let want_area = w["a"].as_f64().unwrap();
            let got_area = {
                use geo::Area;
                g.unsigned_area()
            };
            let d = (got_area - want_area).abs();
            assert!(
                approx(got_area, want_area),
                "{rel}[{i}] ({}): area {got_area} != {want_area} (delta {d})",
                kind_of(obj)
            );
            if want_area > 0.0 {
                worst_area = worst_area.max(d / want_area);
            }

            match (bounds_of(g), w["b"].as_array()) {
                (None, None) => {}
                (Some(got), Some(wb)) => {
                    let wv: Vec<f64> = wb.iter().map(|v| v.as_f64().unwrap()).collect();
                    let got = [got.0, got.1, got.2, got.3];
                    for (a, b) in got.iter().zip(&wv) {
                        let d = (a - b).abs();
                        worst_bound = worst_bound.max(d);
                        assert!(
                            d <= bound_tol(obj),
                            "{rel}[{i}] ({}): bounds {got:?} != {wv:?}",
                            kind_of(obj)
                        );
                    }
                }
                (got, _) => panic!("{rel}[{i}]: empty mismatch, got {got:?}"),
            }

            // %TO object attributes
            let wa = w["t"].as_object().unwrap();
            assert_eq!(obj.attrs().len(), wa.len(), "{rel}[{i}]: attribute count");
            for (k, v) in wa {
                assert_eq!(
                    obj.attrs().get(k).map(String::as_str),
                    Some(v.as_str().unwrap()),
                    "{rel}[{i}]: attribute {k}"
                );
            }
            objects += 1;
        }

        // ---- apertures -----------------------------------------------------
        let waps = want["apertures"].as_object().unwrap();
        assert_eq!(gf.apertures.len(), waps.len(), "{rel}: aperture count");
        for (code, wap) in waps {
            let code: u32 = code.parse().unwrap();
            let ap = gf
                .apertures
                .get(&code)
                .unwrap_or_else(|| panic!("{rel}: no D{code}"));
            assert_eq!(
                ap.template,
                wap["template"].as_str().unwrap(),
                "{rel}: D{code} template"
            );
            assert_eq!(
                ap.describe(),
                wap["describe"].as_str().unwrap(),
                "{rel}: D{code} describe"
            );
            assert_eq!(
                bake_aperture(ap, false).key(),
                wap["key"].as_str().unwrap(),
                "{rel}: D{code} key"
            );
            assert_eq!(
                bake_aperture(ap, true).key(),
                wap["key_mirror"].as_str().unwrap(),
                "{rel}: D{code} mirrored key"
            );
            apertures += 1;
        }

        // ---- file bounds ----------------------------------------------------
        match (gf.bounds(), want["bounds"].as_array()) {
            (None, None) => {}
            (Some(got), Some(wb)) => {
                let wv: Vec<f64> = wb.iter().map(|v| v.as_f64().unwrap()).collect();
                for (a, b) in [got.0, got.1, got.2, got.3].iter().zip(&wv) {
                    // the outline layers are stroked arcs, see ARC_BOUND_TOL
                    assert!(
                        (a - b).abs() <= common::ARC_BOUND_TOL,
                        "{rel}: file bounds {got:?} != {wv:?}"
                    );
                }
            }
            (got, _) => panic!("{rel}: file bounds mismatch, got {got:?}"),
        }

        files += 1;
    }

    eprintln!(
        "gerber parity: {files} files, {objects} objects, {apertures} apertures; \
         worst relative area {worst_area:.3e}, worst bound {worst_bound:.3e}; \
         parse {t_parse:.3}s + geometry {t_geom:.3}s"
    );
    assert!(
        files >= 40,
        "expected the full example set, got {files} files"
    );
}
