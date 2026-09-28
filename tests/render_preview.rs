//! Public-API checks for the preview renderer.
//!
//! The unit tests inside `src/render.rs` can reach the private frame geometry
//! and therefore assert individual pixels; these go through `render_preview`
//! the way the CLI and the TUI do and only look at the finished PNG.

use std::collections::BTreeMap;
use std::rc::Rc;

use stencicrity::gerber::{Aperture, Flash};
use stencicrity::model::{
    Area, Geom, Layout, LayoutParams, Pad, Project, Side, SideId, Transform, DATUM_SLOTS,
    STATE_OPEN, STATE_UNDEFINED,
};
use stencicrity::render::{render_preview, RenderOptions};
use tiny_skia::Pixmap;

fn square(x: f64, y: f64) -> Geom {
    geo::MultiPolygon::new(vec![geo::Polygon::new(
        geo::LineString::from(vec![
            (x, y),
            (x + 1.0, y),
            (x + 1.0, y + 1.0),
            (x, y + 1.0),
            (x, y),
        ]),
        vec![],
    )])
}

fn pad(ref_: &str, x: f64, y: f64, state: &str) -> Pad {
    Pad {
        key: format!("demo/top/{ref_}.1@{x:.3},{y:.3}"),
        project: "demo".to_string(),
        side: "top".to_string(),
        ref_: ref_.to_string(),
        pin: "1".to_string(),
        x,
        y,
        function: "SMDPad".to_string(),
        shape: "R 1.00x1.00".to_string(),
        flash: Flash {
            x,
            y,
            aperture: Rc::new(Aperture::new(10, "C", vec![0.6], BTreeMap::new(), None)),
            attrs: BTreeMap::new(),
            dark: true,
        },
        geom: square(x, y),
        has_paste: false,
        state: state.to_string(),
        paste_indices: Vec::new(),
    }
}

/// One 40 x 30 mm cell with two pads on a 100 x 70 mm sheet.
fn demo() -> (Vec<Project>, Layout) {
    let projects = vec![Project {
        name: "demo".to_string(),
        source: "demo.zip".to_string(),
        outline: None,
        bbox: (20.0, 20.0, 50.0, 40.0),
        sides: vec![Side {
            project_name: "demo".to_string(),
            board_bbox: (20.0, 20.0, 50.0, 40.0),
            name: "top".to_string(),
            copper: None,
            paste: None,
            mirror: false,
            enabled: true,
            pads: vec![
                pad("U1", 25.0, 25.0, STATE_UNDEFINED),
                pad("R1", 30.0, 25.0, STATE_OPEN),
            ],
        }],
    }];
    let layout = Layout {
        params: LayoutParams {
            datum: DATUM_SLOTS.to_string(),
            ..LayoutParams::default()
        },
        areas: vec![Area {
            side: SideId {
                project: 0,
                side: 0,
            },
            x: 10.0,
            y: 10.0,
            w: 50.0,
            h: 40.0,
            board_rect: (20.0, 20.0, 50.0, 40.0),
            transform: Transform::default(),
            row: 0,
            overflow: false,
            holes: Vec::new(),
            slots: vec![(20.0, 11.0, 8.0, 4.5)],
            pins: vec![(20.0, 13.0)],
            datum_corner: (10.0, 10.0),
            marker: Some((15.0, 15.0)),
        }],
        width: 100.0,
        height: 70.0,
        block: (10.0, 10.0, 60.0, 50.0),
        fits: true,
        dots: vec![(30.0, 60.0)],
        dividers: vec![(10.0, 10.0, 60.0, 10.0)],
        heuristic: "bottom-left".to_string(),
        overflow: 0,
    };
    (projects, layout)
}

fn colors(pm: &Pixmap) -> std::collections::HashSet<[u8; 3]> {
    pm.pixels()
        .iter()
        .map(|p| [p.red(), p.green(), p.blue()])
        .collect()
}

#[test]
fn a_preview_png_is_written_and_carries_every_colour_class() {
    let (projects, layout) = demo();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("stencil-preview.png");
    let opts = RenderOptions {
        px_per_mm: 12.0,
        max_px: 4000,
        title: "stencil".to_string(),
        ..Default::default()
    };

    render_preview(&projects, &layout, &path, &opts).expect("render");
    assert!(
        path.is_file(),
        "the png (and its directory) must be created"
    );

    let pm = Pixmap::load_png(&path).expect("a valid png");
    assert!(
        pm.width() > (100.0 * 12.0) as u32,
        "sheet width plus the left ruler band"
    );
    assert!(
        pm.height() > (70.0 * 12.0) as u32,
        "sheet height plus the bottom bands"
    );
    assert!(pm.width().max(pm.height()) <= 4000);

    let seen = colors(&pm);
    for (name, color) in [
        ("background", [0x14, 0x14, 0x14]),
        ("stencil boundary", [0x8a, 0x8a, 0x8a]),
        ("board outline", [0xc8, 0xc8, 0xc8]),
        ("copper pad", [0x5a, 0x5a, 0x5a]),
        ("opening", [0xff, 0x2a, 0x2a]),
        ("undefined pad", [0xff, 0xd0, 0x00]),
        ("ruler band", [0x1c, 0x1c, 0x1c]),
        ("ruler ink", [0xb0, 0xb0, 0xb0]),
    ] {
        assert!(
            seen.contains(&color),
            "{name} ({color:02x?}) is missing from the preview"
        );
    }
}

#[test]
fn the_cap_shrinks_the_image_without_losing_the_bands() {
    let (projects, layout) = demo();
    let dir = tempfile::tempdir().unwrap();
    let mut previous = u32::MAX;
    for max_px in [3000u32, 900, 500, 300] {
        let path = dir.path().join(format!("p{max_px}.png"));
        let opts = RenderOptions {
            px_per_mm: 30.0,
            max_px,
            ..Default::default()
        };
        render_preview(&projects, &layout, &path, &opts).expect("render");
        let pm = Pixmap::load_png(&path).expect("a valid png");
        let longest = pm.width().max(pm.height());
        assert!(longest <= max_px, "cap {max_px}: got {longest}");
        assert!(
            longest < previous,
            "a tighter cap must give a smaller image"
        );
        previous = longest;
        let seen = colors(&pm);
        assert!(
            seen.contains(&[0x1c, 0x1c, 0x1c]),
            "cap {max_px}: no ruler band"
        );
        assert!(
            seen.contains(&[0x8a, 0x8a, 0x8a]),
            "cap {max_px}: no stencil boundary"
        );
    }
}

#[test]
fn a_selected_pad_is_highlighted_in_cyan() {
    let (projects, layout) = demo();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("selected.png");
    let opts = RenderOptions {
        px_per_mm: 12.0,
        max_px: 4000,
        selected: Some((
            SideId {
                project: 0,
                side: 0,
            },
            0,
        )),
        ..Default::default()
    };
    render_preview(&projects, &layout, &path, &opts).expect("render");
    let pm = Pixmap::load_png(&path).expect("a valid png");
    assert!(
        colors(&pm).contains(&[0x00, 0xe5, 0xff]),
        "no cyan selection box"
    );
}
