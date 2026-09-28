//! The split view (`v`) from outside the crate: one synthetic project packed
//! by the real packer, a `TestBackend`, and a timing run over a real gerber set.

use std::collections::BTreeMap;
use std::rc::Rc;

use geo::{LineString, MultiPolygon, Polygon};
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Color;
use ratatui::Terminal;

use stencicrity::ascii::{self, Class, GeomCache};
use stencicrity::gerber::{Aperture, Flash, GerberFile, GraphicObject, Region, Segment};
use stencicrity::layout::pack;
use stencicrity::model::{
    all_sides, Config, Layout, Pad, Project, Side, SideId, SIDE_TOP, STATE_IGNORE, STATE_OPEN,
    STATE_UNDEFINED,
};
use stencicrity::tui::{
    panel_rows, split_plan, App, PadId, SplitPlan, TuiHooks, LEGEND_MIN_HEIGHT, NO_PAD, SEPARATOR,
    SPLIT_LEFT_MIN, SPLIT_LEFT_WIDTH, SPLIT_MIN_WIDTH, SPLIT_PROMPT,
};

// --------------------------------------------------------------------------- //
// fixtures
// --------------------------------------------------------------------------- //

fn aperture() -> Rc<Aperture> {
    Rc::new(Aperture::new(
        10,
        "R",
        vec![1.0, 0.5],
        BTreeMap::new(),
        None,
    ))
}

fn rect_geom(cx: f64, cy: f64, w: f64, h: f64) -> MultiPolygon<f64> {
    let (hw, hh) = (w / 2.0, h / 2.0);
    MultiPolygon::new(vec![Polygon::new(
        LineString::from(vec![
            (cx - hw, cy - hh),
            (cx + hw, cy - hh),
            (cx + hw, cy + hh),
            (cx - hw, cy + hh),
            (cx - hw, cy - hh),
        ]),
        vec![],
    )])
}

fn pad(ref_: &str, pin: &str, x: f64, y: f64, state: &str, has_paste: bool) -> Pad {
    Pad {
        key: format!("demo/top/{ref_}.{pin}@{x:.3},{y:.3}"),
        project: "demo".to_string(),
        side: SIDE_TOP.to_string(),
        ref_: ref_.to_string(),
        pin: pin.to_string(),
        x,
        y,
        function: "SMDPad".to_string(),
        shape: "R 1.00x0.50".to_string(),
        flash: Flash {
            x,
            y,
            aperture: aperture(),
            attrs: BTreeMap::new(),
            dark: true,
        },
        geom: rect_geom(x, y, 1.0, 0.5),
        has_paste,
        state: state.to_string(),
        paste_indices: if has_paste { vec![0] } else { Vec::new() },
    }
}

/// A paste layer with one square opening, built from region contours.
fn paste_layer(cx: f64, cy: f64, size: f64) -> GerberFile {
    let h = size / 2.0;
    let corners = [
        (cx - h, cy - h),
        (cx + h, cy - h),
        (cx + h, cy + h),
        (cx - h, cy + h),
    ];
    let mut contour = Vec::new();
    for i in 0..corners.len() {
        let a = corners[i];
        let b = corners[(i + 1) % corners.len()];
        contour.push(Segment::line(a.0, a.1, b.0, b.1));
    }
    let mut file = GerberFile::default();
    file.objects.push(GraphicObject::Region(Region {
        contours: vec![contour],
        attrs: BTreeMap::new(),
        dark: true,
    }));
    file
}

/// One 25 x 20 mm board with everything the picture can show packed into its
/// bottom left corner: an undefined pad, an open one, an ignored one, a real
/// paste opening two millimetres away and the board edge right next to them.
fn projects() -> Vec<Project> {
    let side = Side {
        project_name: "demo".to_string(),
        board_bbox: (0.0, 0.0, 25.0, 20.0),
        name: SIDE_TOP.to_string(),
        copper: None,
        paste: Some(paste_layer(3.0, 1.0, 0.8)),
        mirror: false,
        enabled: true,
        pads: vec![
            pad("U1", "1", 1.0, 1.0, STATE_UNDEFINED, false),
            pad("U1", "2", 2.0, 1.0, STATE_UNDEFINED, false),
            pad("U1", "3", 1.0, 2.0, STATE_OPEN, false),
            pad("U1", "4", 2.0, 2.0, STATE_IGNORE, false),
        ],
    };
    vec![Project {
        name: "demo".to_string(),
        source: "demo.zip".to_string(),
        outline: None,
        bbox: (0.0, 0.0, 25.0, 20.0),
        sides: vec![side],
    }]
}

fn ch(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

macro_rules! split_app {
    ($app:ident, $model:ident) => {
        let mut $model = projects();
        let sides = all_sides(&$model);
        let order: Vec<PadId> = (0..$model[0].sides[0].pads.len())
            .map(|i| (sides[0], i))
            .collect();
        let mut config = Config::default();
        config.size = (600, 600);
        let mut on_preview =
            |_: &[Project], _: &Config, _: Option<PadId>, _: bool| -> String { String::new() };
        let mut compute_layout = |ps: &[Project], cfg: &Config| -> Layout {
            let on: Vec<SideId> = all_sides(ps)
                .into_iter()
                .filter(|id| id.get(ps).enabled)
                .collect();
            pack(ps, &on, cfg)
        };
        let hooks = TuiHooks {
            on_preview: &mut on_preview,
            compute_layout: &mut compute_layout,
        };
        let mut $app = App::new(&mut $model, &sides, order, &mut config, hooks, "demo");
        $app.refresh();
    };
}

fn draw(app: &mut App<'_, '_>, width: u16, height: u16) -> ratatui::buffer::Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    app.before_draw();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    terminal.backend().buffer().clone()
}

fn band(buf: &ratatui::buffer::Buffer, y: u16, x0: u16, x1: u16) -> String {
    (x0..x1.min(buf.area.width))
        .map(|x| buf[(x, y)].symbol())
        .collect::<String>()
        .trim_end()
        .to_string()
}

fn colours(buf: &ratatui::buffer::Buffer, x0: u16, x1: u16) -> Vec<Color> {
    let last = buf
        .area
        .height
        .saturating_sub(u16::from(buf.area.height >= LEGEND_MIN_HEIGHT));
    let mut out = Vec::new();
    for y in 1..last {
        for x in x0..x1.min(buf.area.width) {
            let c = &buf[(x, y)];
            if c.symbol() != " " {
                out.push(c.fg);
                out.push(c.bg);
            }
        }
    }
    out
}

// --------------------------------------------------------------------------- //
// tests
// --------------------------------------------------------------------------- //

#[test]
fn v_toggles_a_split_that_draws_the_pad_under_the_cursor() {
    split_app!(app, _model);
    assert!(!app.split());
    app.handle_key(ch('v'));
    assert!(app.split());

    let buf = draw(&mut app, 150, 40);
    assert_eq!(
        split_plan(150),
        SplitPlan {
            left: SPLIT_LEFT_WIDTH,
            right_x: SPLIT_LEFT_WIDTH + 1,
            right_w: 150 - SPLIT_LEFT_WIDTH - 1,
            preview: true,
        }
    );
    for y in 0..40 {
        assert_eq!(buf[(90, y)].symbol(), SEPARATOR, "row {y}");
    }
    // the caption names the pad, the window and the zoom
    let caption = band(&buf, 0, 91, 150);
    assert!(
        caption.starts_with("U1.1 · demo top · window 4.00 x 5.15 mm · 1 col = 0.07 mm"),
        "{caption:?}"
    );
    // the picture is centred on the pad: the middle cell is the pad itself
    let middle = &buf[(91 + 29, 1 + panel_rows(40) as u16 / 2)];
    assert_eq!(middle.symbol(), ascii::FULL_BLOCK);
    assert_eq!(middle.fg, ascii::class_color(Class::Selected));

    let seen = colours(&buf, 91, 150);
    for class in [
        Class::Selected,  // U1.1, the cursor
        Class::Undefined, // U1.2, right next to it
        Class::Opening,   // U1.3 (open) and the paste region
        Class::Ignored,   // U1.4
        Class::Outline,   // the board's bounding box
    ] {
        assert!(
            seen.contains(&ascii::class_color(class)),
            "{class:?} is missing from the picture"
        );
    }
    // the legend closes the panel
    assert!(band(&buf, 39, 91, 150).starts_with("red opening  yellow undefined"));

    app.handle_key(ch('v'));
    assert!(!app.split());
    let buf = draw(&mut app, 150, 40);
    assert_ne!(buf[(90, 0)].symbol(), SEPARATOR);
}

#[test]
fn below_150_columns_the_right_half_only_asks_for_more() {
    split_app!(app, _model);
    app.handle_key(ch('v'));
    let plan = split_plan(120);
    assert!(!plan.preview);
    assert_eq!(plan.left, SPLIT_LEFT_MIN);

    let buf = draw(&mut app, 120, 40);
    for y in 0..40 {
        assert_eq!(buf[(60, y)].symbol(), SEPARATOR, "row {y}");
    }
    assert_eq!(band(&buf, 20, 61, 120), SPLIT_PROMPT);
    for y in 0..40 {
        if y != 20 {
            assert_eq!(band(&buf, y, 61, 120), "", "row {y}");
        }
    }
    // nothing was rasterised
    assert!(colours(&buf, 61, 120)
        .iter()
        .all(|c| *c == Color::Reset || *c == Color::Gray));

    // the app half still takes keys
    assert_eq!(app.index(), 0);
    app.handle_key(ch('j'));
    assert_eq!(app.index(), 1);
    assert!(band(&draw(&mut app, 120, 40), 1, 0, 60).starts_with("pads 2/4"));
}

#[test]
fn the_cursor_and_the_states_drive_the_picture() {
    split_app!(app, _model);
    app.handle_key(ch('v'));
    assert!(band(&draw(&mut app, 150, 40), 0, 91, 150).starts_with("U1.1 · demo top"));

    app.handle_key(ch('j'));
    assert!(band(&draw(&mut app, 150, 40), 0, 91, 150).starts_with("U1.2 · demo top"));

    // U1.1 is undefined (yellow) next to the cursor; ignoring it turns it blue
    let yellow = ascii::class_color(Class::Undefined);
    assert!(colours(&draw(&mut app, 150, 40), 91, 150).contains(&yellow));
    app.handle_key(ch('k'));
    app.handle_key(ch('j')); // back on U1.2, so U1.1 is a neighbour again
    app.handle_key(ch('k'));
    app.handle_key(ch('i')); // U1.1 -> ignore
    app.handle_key(ch('j')); // and look at it from U1.2
    let seen = colours(&draw(&mut app, 150, 40), 91, 150);
    assert!(!seen.contains(&yellow));
    assert!(seen.contains(&ascii::class_color(Class::Ignored)));
}

#[test]
fn the_panel_says_so_without_a_pad() {
    split_app!(app, _model);
    app.handle_key(ch('v'));
    app.handle_key(ch('2'));
    app.handle_key(ch('N')); // every side off
    let buf = draw(&mut app, 150, 40);
    assert_eq!(band(&buf, 20, 91, 150).trim(), NO_PAD);
}

#[test]
fn no_terminal_size_panics() {
    split_app!(app, _model);
    app.handle_key(ch('v'));
    for (w, h) in [
        (2u16, 8u16),
        (60, 10),
        (149, 40),
        (400, 100),
        (1, 1),
        (SPLIT_MIN_WIDTH, 1),
        (SPLIT_MIN_WIDTH, 3),
    ] {
        let buf = draw(&mut app, w, h);
        assert_eq!((buf.area.width, buf.area.height), (w, h));
    }
}

/// Timing over a real gerber set; see `tests/render_examples.rs` for the setup.
///
/// ```text
/// STENCICRITY_EXAMPLES=/path/to/zips \
///     cargo test --release --test tui_split -- --ignored --nocapture
/// ```
#[test]
#[ignore = "needs a directory of gerber zips in STENCICRITY_EXAMPLES"]
fn rasterising_a_real_board_is_fast() {
    use std::time::Instant;
    use stencicrity::pads::{detect_pads, DetectOptions};
    use stencicrity::project::{discover_projects, DiscoverOptions};

    let Ok(dir) = std::env::var("STENCICRITY_EXAMPLES") else {
        eprintln!("STENCICRITY_EXAMPLES is not set, skipping");
        return;
    };
    let mut warn = |m: &str| eprintln!("warning: {m}");
    let mut model = discover_projects(
        &[],
        &dir,
        &DiscoverOptions {
            mirror_bottom: true,
            exclude_dirs: vec![],
        },
        &mut warn,
    )
    .expect("discover");
    assert!(!model.is_empty(), "no gerber projects found in {dir}");
    let config = Config::default();
    let opts = DetectOptions {
        ignore_prefixes: config.ignore_prefixes.clone(),
        ..DetectOptions::default()
    };
    for project in &mut model {
        for side in &mut project.sides {
            detect_pads(side, &opts);
        }
    }
    let sides = all_sides(&model);
    let layout = pack(&model, &sides, &config);

    let mut cache = GeomCache::new();
    for (cols, rows) in [(60usize, 40usize), (309, 98)] {
        for id in &sides {
            let side = id.get(&model);
            if side.pads.is_empty() {
                continue;
            }
            // the cold pass fills the cache, the warm ones are what a frame costs
            let start = Instant::now();
            ascii::render_pad(&model, &layout, (*id, 0), &mut cache, cols, rows);
            let cold = start.elapsed();
            let start = Instant::now();
            let n = side.pads.len().min(20);
            for index in 0..n {
                ascii::render_pad(&model, &layout, (*id, index), &mut cache, cols, rows);
            }
            let warm = start.elapsed() / n as u32;
            eprintln!(
                "{:<24} {:>5} pads  {cols}x{rows}  cold {cold:>10.2?}  frame {warm:>10.2?}",
                side.label(),
                side.pads.len(),
            );
            assert!(
                warm.as_millis() < 50,
                "{} took {warm:?} per frame",
                side.label()
            );
        }
    }
}
