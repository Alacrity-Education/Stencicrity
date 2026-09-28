//! The TUI's public surface, driven from outside the crate: one synthetic
//! project, a `TestBackend` and a handful of keys.

use std::collections::BTreeMap;
use std::rc::Rc;

use geo::{LineString, MultiPolygon, Polygon};
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;

use stencicrity::gerber::{Aperture, Flash};
use stencicrity::model::{
    all_sides, Config, Layout, Pad, Project, Side, SideId, SIDE_TOP, STATE_OPEN, STATE_UNDEFINED,
};
use stencicrity::tui::{
    self, count_states, fit_line, page_items, wrap_items, Action, App, PadId, Row, StateCounts,
    TuiHooks, FOOTER_LINES, PAGE_PADS, PAGE_SIDES,
};

fn pad(ref_: &str, pin: &str, x: f64, y: f64, state: &str) -> Pad {
    let aperture = Rc::new(Aperture::new(
        10,
        "R",
        vec![1.0, 0.5],
        BTreeMap::new(),
        None,
    ));
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
            aperture,
            attrs: BTreeMap::new(),
            dark: true,
        },
        // built by hand: Aperture::geometry() is not needed for the TUI
        geom: MultiPolygon::new(vec![Polygon::new(
            LineString::from(vec![
                (x - 0.5, y - 0.25),
                (x + 0.5, y - 0.25),
                (x + 0.5, y + 0.25),
                (x - 0.5, y + 0.25),
                (x - 0.5, y - 0.25),
            ]),
            vec![],
        )]),
        has_paste: false,
        state: state.to_string(),
        paste_indices: Vec::new(),
    }
}

fn projects() -> Vec<Project> {
    let side = Side {
        project_name: "demo".to_string(),
        board_bbox: (0.0, 0.0, 25.0, 20.0),
        name: SIDE_TOP.to_string(),
        copper: None,
        paste: None,
        mirror: false,
        enabled: true,
        pads: vec![
            pad("U1", "1", 1.0, 1.0, STATE_UNDEFINED),
            pad("U1", "2", 2.0, 1.0, STATE_UNDEFINED),
            pad("R7", "1", 3.0, 2.0, STATE_UNDEFINED),
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

fn layout_for(config: &Config) -> Layout {
    let (width, height) = config.sheet_size();
    Layout {
        params: config.layout.clone(),
        areas: Vec::new(),
        width,
        height,
        block: (0.0, 0.0, 100.0, 80.0),
        fits: true,
        dots: Vec::new(),
        dividers: Vec::new(),
        heuristic: "test".to_string(),
        overflow: 0,
        dots_dropped: 0,
    }
}

fn ch(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

#[test]
fn the_public_api_drives_a_whole_session() {
    let mut model = projects();
    let sides = all_sides(&model);
    let order: Vec<PadId> = (0..3).map(|i| (sides[0], i)).collect();
    let mut config = Config::default();
    let mut previews: Vec<bool> = Vec::new();
    let mut on_preview = |_: &[Project], _: &Config, _: Option<PadId>, open: bool| -> String {
        previews.push(open);
        "/tmp/demo.png".to_string()
    };
    let mut compute_layout = |_: &[Project], cfg: &Config| -> Layout { layout_for(cfg) };
    let hooks = TuiHooks {
        on_preview: &mut on_preview,
        compute_layout: &mut compute_layout,
    };
    let mut app = App::new(&mut model, &sides, order, &mut config, hooks, "demo");
    app.refresh();

    assert_eq!(app.page(), PAGE_PADS);
    assert_eq!(app.rows_list().len(), 3);

    // move, open a pad, search and keep the filter
    assert_eq!(app.handle_key(ch('j')), Action::Continue);
    assert_eq!(app.index(), 1);
    app.handle_key(ch('o'));
    assert!(app.status().starts_with("U1.2"));
    app.handle_key(ch('/'));
    for c in "r7".chars() {
        app.handle_key(ch(c));
    }
    assert_eq!(app.rows_list().len(), 1);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.active_filter(), Some("r7"));
    assert!(matches!(app.current_row(), Some(Row::Pad(_))));
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.active_filter(), None);

    // the preview runs after the "rendering…" frame
    app.handle_key(ch('p'));
    assert_eq!(app.status(), "rendering…");
    assert!(app.run_preview());
    assert_eq!(app.status(), "preview opened: /tmp/demo.png");

    // render on a test backend and read the buffer back
    let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
    app.before_draw();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    let buf = terminal.backend().buffer().clone();
    let row = |y: u16| -> String {
        (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol())
            .collect::<String>()
            .trim_end()
            .to_string()
    };
    assert!(row(0).starts_with("demo    1 Pads"));
    // Esc put the cursor back on the row the filter had left it on
    assert!(row(1).starts_with("pads 3/3"), "{:?}", row(1));
    assert!(row(4).contains("U1.2"));
    assert!(row(5).contains("R7.1"));
    assert!(row(14).contains("FITS"));

    // the layout fits, so a single w generates
    app.handle_key(ch('2'));
    assert_eq!(app.page(), PAGE_SIDES);
    assert_eq!(app.handle_key(ch('w')), Action::Generate);
    assert_eq!(app.handle_key(ch('q')), Action::Quit);

    drop(app);
    assert_eq!(previews, vec![true]);
    assert_eq!(model[0].sides[0].pads[1].state, STATE_OPEN);
}

#[test]
fn pure_helpers_are_reusable() {
    let items = page_items(PAGE_PADS, false);
    assert_eq!(wrap_items(&items, 150, FOOTER_LINES).len(), 1);
    assert_eq!(wrap_items(&items, 80, FOOTER_LINES).len(), 2);

    let model = projects();
    let counts = count_states(model[0].sides[0].pads.iter());
    assert_eq!(
        counts,
        StateCounts {
            undefined: 3,
            open: 0,
            ignore: 0,
            closed: 0
        }
    );

    let config = Config::default();
    let layout = layout_for(&config);
    assert_eq!(
        fit_line(&config, Some(&layout), counts),
        "stencil 380x280 landscape · block 100.0 x 80.0 mm · FITS \
         · pads: undefined 3  open 0  ignore 0  closed 0"
    );
}

#[test]
fn nothing_to_show_never_touches_the_terminal() {
    let mut config = Config::default();
    let mut on_preview =
        |_: &[Project], _: &Config, _: Option<PadId>, _: bool| -> String { String::new() };
    let mut compute_layout = |_: &[Project], cfg: &Config| -> Layout { layout_for(cfg) };
    let hooks = TuiHooks {
        on_preview: &mut on_preview,
        compute_layout: &mut compute_layout,
    };
    let empty: [SideId; 0] = [];
    assert!(tui::run_tui(&mut [], &empty, &mut config, hooks, "").unwrap());
}
