//! RS-274X (X2) writer: emits merged layers in mm with a 4.6 coordinate format.
//!
//! Emits KiCad-like output: a fixed X2 header, an aperture list (macros and
//! aperture definitions with their attributes) and a body of graphic objects.
//! Apertures are registered lazily on first use and deduplicated by
//! [`BakedAperture::key`], so the body is buffered and the whole file is
//! assembled in [`GerberWriter::render`].

use std::collections::BTreeMap;
use std::path::Path;

use crate::gerber::{bake_aperture, BakedAperture, GraphicObject, Segment};
use crate::model::Transform;

/// File units per mm (4.6 format).
const SCALE: f64 = 1_000_000.0;
const LINEAR: u8 = 1; // G01
const CW: u8 = 2; // G02
const CCW: u8 = 3; // G03

/// Millimetres to file units (integer nanometres), rounding half to even the
/// way Python's `round()` does.
fn nm(value: f64) -> i64 {
    let x = value * SCALE;
    let mut r = x.round(); // half away from zero, like C round()
    if (x - r).abs() == 0.5 {
        r = 2.0 * (x / 2.0).round();
    }
    r as i64
}

pub struct GerberWriter {
    file_function: String,
    polarity: String,
    software: String,
    version: String,
    created: String,

    // Aperture list (assembled before the body in render()).
    macro_lines: Vec<String>,
    macro_names: BTreeMap<String, String>, // macro body -> macro name
    aperture_lines: Vec<String>,
    aperture_codes: BTreeMap<String, u32>, // BakedAperture::key() -> D-code
    next_code: u32,
    next_macro: u32,

    // Body and graphics state.
    body: Vec<String>,
    aperture: Option<u32>, // selected D-code
    x: Option<i64>,        // current point, file units
    y: Option<i64>,
    interp: u8,           // interpolation mode (G01/G02/G03)
    multi_quadrant: bool, // true once an arc needs G75
    dark: bool,           // current polarity (%LPD*%)
}

impl GerberWriter {
    pub fn new(file_function: &str) -> Self {
        Self {
            file_function: file_function.to_string(),
            polarity: "Positive".to_string(),
            software: "stencicrity".to_string(),
            version: crate::VERSION.to_string(),
            created: now_iso8601(),
            macro_lines: Vec::new(),
            macro_names: BTreeMap::new(),
            aperture_lines: Vec::new(),
            aperture_codes: BTreeMap::new(),
            next_code: 10,
            next_macro: 1,
            body: Vec::new(),
            aperture: None,
            x: None,
            y: None,
            interp: LINEAR,
            multi_quadrant: false,
            dark: true,
        }
    }

    /// Override the `%TF.CreationDate` header (for reproducible output).
    pub fn set_created(&mut self, iso8601: &str) {
        self.created = iso8601.to_string();
    }

    /// Override the `%TF.GenerationSoftware` tool name and version.
    pub fn set_software(&mut self, software: &str, version: &str) {
        self.software = software.to_string();
        self.version = version.to_string();
    }

    /// Override the `%TF.FilePolarity` header.
    pub fn set_polarity(&mut self, polarity: &str) {
        self.polarity = polarity.to_string();
    }

    // ------------------------------------------------------------------ state

    /// Return the D-code for `baked`, defining it on first use.
    fn register(&mut self, baked: &BakedAperture) -> u32 {
        let key = baked.key();
        if let Some(code) = self.aperture_codes.get(&key) {
            return *code;
        }
        let code = self.next_code;
        self.next_code += 1;
        self.aperture_codes.insert(key, code);

        let definition = if baked.template == "MACRO" {
            let name = self.macro_name(baked);
            format!("%ADD{code}{name}*%")
        } else if !baked.modifiers.is_empty() {
            let mods: Vec<String> = baked.modifiers.iter().map(|m| format!("{m:.6}")).collect();
            format!("%ADD{code}{},{}*%", baked.template, mods.join("X"))
        } else {
            format!("%ADD{code}{}*%", baked.template)
        };

        for (attr, value) in &baked.attrs {
            self.aperture_lines.push(if value.is_empty() {
                format!("%TA.{attr}*%")
            } else {
                format!("%TA.{attr},{value}*%")
            });
        }
        self.aperture_lines.push(definition);
        if !baked.attrs.is_empty() {
            self.aperture_lines.push("%TD*%".to_string());
        }
        code
    }

    /// Return the macro name for a baked macro aperture, defining it once.
    fn macro_name(&mut self, baked: &BakedAperture) -> String {
        let prims: Vec<String> = baked.prims.iter().map(|p| p.as_gerber()).collect();
        let body = if prims.is_empty() {
            "0 empty*".to_string()
        } else {
            prims.join("\n")
        };
        if let Some(name) = self.macro_names.get(&body) {
            return name.clone();
        }
        let name = format!("M{}", self.next_macro);
        self.next_macro += 1;
        self.macro_names.insert(body.clone(), name.clone());
        self.macro_lines.push(format!("%AM{name}*\n{body}\n%"));
        name
    }

    /// Emit a D-code selection when the aperture changes.
    fn select(&mut self, code: u32) {
        if self.aperture != Some(code) {
            self.body.push(format!("D{code}*"));
            self.aperture = Some(code);
        }
    }

    /// Emit `%LPD*%` / `%LPC*%` when the polarity changes.
    fn set_dark(&mut self, dark: bool) {
        if dark != self.dark {
            self.body.push(if dark {
                "%LPD*%".to_string()
            } else {
                "%LPC*%".to_string()
            });
            self.dark = dark;
        }
    }

    /// Emit `G01*` / `G02*` / `G03*` when the interpolation changes.
    fn set_interp(&mut self, mode: u8) {
        if self.interp != mode {
            self.body.push(format!("G0{mode}*"));
            self.interp = mode;
        }
    }

    /// Emit a `D02` move when the current point differs (or when forced).
    fn move_to(&mut self, x: i64, y: i64, force: bool) {
        if force || self.x != Some(x) || self.y != Some(y) {
            self.body.push(format!("X{x}Y{y}D02*"));
            self.x = Some(x);
            self.y = Some(y);
        }
    }

    /// Emit one `D01` interpolation for a segment under `tr`.
    fn segment(&mut self, seg: &Segment, tr: &Transform) {
        let (ax, ay) = tr.apply(seg.x0, seg.y0);
        let (x0, y0) = (nm(ax), nm(ay));
        let (bx, by) = tr.apply(seg.x1, seg.y1);
        let (x1, y1) = (nm(bx), nm(by));
        if seg.arc {
            // Mirroring about the Y axis reverses the direction of travel.
            let clockwise = seg.clockwise != tr.mirror;
            let (mx, my) = tr.apply(seg.cx, seg.cy);
            let (cx, cy) = (nm(mx), nm(my));
            self.multi_quadrant = true;
            self.set_interp(if clockwise { CW } else { CCW });
            self.body
                .push(format!("X{x1}Y{y1}I{}J{}D01*", cx - x0, cy - y0));
        } else {
            self.set_interp(LINEAR);
            self.body.push(format!("X{x1}Y{y1}D01*"));
        }
        self.x = Some(x1);
        self.y = Some(y1);
    }

    // ----------------------------------------------------------------- public

    /// G04 comment line (`*` and `%` are stripped from the text).
    pub fn comment(&mut self, text: &str) {
        let clean = text.replace(['\r', '\n'], " ").replace(['*', '%'], "");
        self.body.push(format!("G04 {}*", clean.trim()));
    }

    /// Flash / stroke / region with the transform applied (mirror-aware apertures and arcs).
    pub fn add_object(&mut self, obj: &GraphicObject, tr: &Transform) {
        match obj {
            GraphicObject::Flash(f) => {
                let code = self.register(&bake_aperture(&f.aperture, tr.mirror));
                self.set_dark(f.dark);
                self.select(code);
                let (px, py) = tr.apply(f.x, f.y);
                let (x, y) = (nm(px), nm(py));
                self.body.push(format!("X{x}Y{y}D03*"));
                self.x = Some(x);
                self.y = Some(y);
            }
            GraphicObject::Stroke(s) => {
                let code = self.register(&bake_aperture(&s.aperture, tr.mirror));
                self.set_dark(s.dark);
                self.select(code);
                let (px, py) = tr.apply(s.seg.x0, s.seg.y0);
                self.move_to(nm(px), nm(py), false);
                self.segment(&s.seg, tr);
            }
            GraphicObject::Region(r) => {
                let contours: Vec<&Vec<Segment>> =
                    r.contours.iter().filter(|c| !c.is_empty()).collect();
                if contours.is_empty() {
                    return;
                }
                self.set_dark(r.dark);
                self.body.push("G36*".to_string());
                for contour in contours {
                    let first = contour[0];
                    let (px, py) = tr.apply(first.x0, first.y0);
                    self.move_to(nm(px), nm(py), true);
                    for seg in contour {
                        self.segment(seg, tr);
                    }
                }
                self.body.push("G37*".to_string());
            }
        }
    }

    /// Flash of a round aperture at sheet coordinates.
    pub fn add_circle(
        &mut self,
        x: f64,
        y: f64,
        dia: f64,
        attrs: Option<&BTreeMap<String, String>>,
    ) {
        let baked = BakedAperture {
            template: "C".to_string(),
            modifiers: vec![dia],
            prims: Vec::new(),
            attrs: attrs.cloned().unwrap_or_default(),
        };
        let code = self.register(&baked);
        self.set_dark(true);
        self.select(code);
        let (ix, iy) = (nm(x), nm(y));
        self.body.push(format!("X{ix}Y{iy}D03*"));
        self.x = Some(ix);
        self.y = Some(iy);
    }

    /// Flash of an obround (`O,wXh`) aperture at sheet coordinates.
    ///
    /// `w` is the size along x and `h` the size along y (the shape is a
    /// rectangle with the two short sides replaced by half circles, i.e. a
    /// circle when `w == h`), so an alignment slot along the bottom cell edge
    /// is `add_obround(cx, cy, slot_length, slot_width)` and one along the left
    /// edge `add_obround(cx, cy, slot_width, slot_length)`.
    pub fn add_obround(
        &mut self,
        cx: f64,
        cy: f64,
        w: f64,
        h: f64,
        attrs: Option<&BTreeMap<String, String>>,
    ) {
        let baked = BakedAperture {
            template: "O".to_string(),
            modifiers: vec![w, h],
            prims: Vec::new(),
            attrs: attrs.cloned().unwrap_or_default(),
        };
        let code = self.register(&baked);
        self.set_dark(true);
        self.select(code);
        let (ix, iy) = (nm(cx), nm(cy));
        self.body.push(format!("X{ix}Y{iy}D03*"));
        self.x = Some(ix);
        self.y = Some(iy);
    }

    /// Filled region from points (skipped when degenerate).
    pub fn add_polygon(&mut self, points: &[(f64, f64)]) {
        let mut pts: Vec<(i64, i64)> = Vec::with_capacity(points.len());
        for &(x, y) in points {
            let p = (nm(x), nm(y));
            if pts.last() != Some(&p) {
                pts.push(p);
            }
        }
        if pts.len() > 1 && pts[0] == pts[pts.len() - 1] {
            pts.pop();
        }
        if pts.len() < 3 {
            return;
        }
        self.set_dark(true);
        self.body.push("G36*".to_string());
        self.move_to(pts[0].0, pts[0].1, true);
        self.set_interp(LINEAR);
        let first = pts[0];
        for &(px, py) in pts[1..].iter().chain(std::iter::once(&first)) {
            self.body.push(format!("X{px}Y{py}D01*"));
            self.x = Some(px);
            self.y = Some(py);
        }
        self.body.push("G37*".to_string());
    }

    /// Stroke with a round aperture.
    pub fn add_line(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, width: f64) {
        let baked = BakedAperture {
            template: "C".to_string(),
            modifiers: vec![width],
            prims: Vec::new(),
            attrs: BTreeMap::new(),
        };
        let code = self.register(&baked);
        self.set_dark(true);
        self.select(code);
        self.move_to(nm(x0), nm(y0), false);
        self.set_interp(LINEAR);
        let (ix, iy) = (nm(x1), nm(y1));
        self.body.push(format!("X{ix}Y{iy}D01*"));
        self.x = Some(ix);
        self.y = Some(iy);
    }

    pub fn add_rect_outline(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, width: f64) {
        self.add_line(x0, y0, x1, y0, width);
        self.add_line(x1, y0, x1, y1, width);
        self.add_line(x1, y1, x0, y1, width);
        self.add_line(x0, y1, x0, y0, width);
    }

    /// The complete gerber file text.
    pub fn render(&self) -> String {
        let mut lines: Vec<String> = vec![
            format!(
                "%TF.GenerationSoftware,{},pcbstencil,{}*%",
                self.software, self.version
            ),
            format!("%TF.CreationDate,{}*%", self.created),
            format!("%TF.FileFunction,{}*%", self.file_function),
            format!("%TF.FilePolarity,{}*%", self.polarity),
            "%FSLAX46Y46*%".to_string(),
            "G04 Gerber Fmt 4.6, Leading zero omitted, Abs format (unit mm)*".to_string(),
            format!("G04 Created by {} {}*", self.software, self.version),
            "%MOMM*%".to_string(),
            "%LPD*%".to_string(),
            "G01*".to_string(),
            "G04 APERTURE LIST*".to_string(),
        ];
        lines.extend(self.macro_lines.iter().cloned());
        lines.extend(self.aperture_lines.iter().cloned());
        lines.push("G04 APERTURE END LIST*".to_string());
        if self.multi_quadrant {
            lines.push("G75*".to_string());
        }
        lines.extend(self.body.iter().cloned());
        lines.push("M02*".to_string());
        let mut out = lines.join("\n");
        out.push('\n');
        out
    }

    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, self.render())
    }
}

/// Local time as ISO 8601 with a UTC offset and seconds resolution.
fn now_iso8601() -> String {
    use chrono::{Local, SecondsFormat};
    Local::now().to_rfc3339_opts(SecondsFormat::Secs, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gerber::parse_gerber;

    fn body_of(text: &str) -> Vec<&str> {
        let start = text
            .lines()
            .position(|l| l == "G04 APERTURE END LIST*")
            .unwrap();
        text.lines().skip(start + 1).collect()
    }

    #[test]
    fn header_is_fixed_and_render_is_idempotent() {
        let mut w = GerberWriter::new("Paste,Top");
        w.set_created("2026-01-02T03:04:05+00:00");
        w.add_circle(1.0, 2.0, 0.5, None);
        let a = w.render();
        let b = w.render();
        assert_eq!(a, b);
        let lines: Vec<&str> = a.lines().collect();
        assert_eq!(
            lines[0],
            format!(
                "%TF.GenerationSoftware,stencicrity,pcbstencil,{}*%",
                crate::VERSION
            )
        );
        assert_eq!(lines[1], "%TF.CreationDate,2026-01-02T03:04:05+00:00*%");
        assert_eq!(lines[2], "%TF.FileFunction,Paste,Top*%");
        assert_eq!(lines[3], "%TF.FilePolarity,Positive*%");
        assert_eq!(lines[4], "%FSLAX46Y46*%");
        assert_eq!(
            lines[5],
            "G04 Gerber Fmt 4.6, Leading zero omitted, Abs format (unit mm)*"
        );
        assert_eq!(
            lines[6],
            format!("G04 Created by stencicrity {}*", crate::VERSION)
        );
        assert_eq!(lines[7], "%MOMM*%");
        assert_eq!(lines[8], "%LPD*%");
        assert_eq!(lines[9], "G01*");
        assert_eq!(lines[10], "G04 APERTURE LIST*");
        assert_eq!(lines[11], "%ADD10C,0.500000*%");
        assert_eq!(lines[12], "G04 APERTURE END LIST*");
        assert_eq!(lines[13], "D10*");
        assert_eq!(lines[14], "X1000000Y2000000D03*");
        assert_eq!(lines[15], "M02*");
        assert!(a.ends_with("M02*\n"));
    }

    #[test]
    fn apertures_are_deduplicated() {
        let mut w = GerberWriter::new("Paste,Top");
        w.add_circle(0.0, 0.0, 0.5, None);
        w.add_circle(1.0, 0.0, 0.5, None);
        w.add_circle(2.0, 0.0, 0.6, None);
        w.add_circle(3.0, 0.0, 0.5, None);
        let t = w.render();
        assert_eq!(t.matches("%ADD").count(), 2);
        assert_eq!(
            body_of(&t),
            vec![
                "D10*",
                "X0Y0D03*",
                "X1000000Y0D03*",
                "D11*",
                "X2000000Y0D03*",
                "D10*",
                "X3000000Y0D03*",
                "M02*",
            ]
        );
    }

    #[test]
    fn obround_and_polygon_and_line() {
        let mut w = GerberWriter::new("Paste,Top");
        w.add_obround(0.0, 0.0, 8.0, 4.5, None);
        w.add_polygon(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.0, 0.0)]);
        w.add_line(0.0, 0.0, 2.0, 0.0, 0.5);
        let t = w.render();
        assert!(t.contains("%ADD10O,8.000000X4.500000*%"));
        assert!(t.contains("%ADD11C,0.500000*%"));
        assert_eq!(
            body_of(&t),
            vec![
                "D10*",
                "X0Y0D03*",
                "G36*",
                "X0Y0D02*",
                "X1000000Y0D01*",
                "X1000000Y1000000D01*",
                "X0Y1000000D01*",
                "X0Y0D01*",
                "G37*",
                "D11*",
                "X2000000Y0D01*",
                "M02*",
            ]
        );
    }

    #[test]
    fn degenerate_polygons_are_skipped() {
        let mut w = GerberWriter::new("Paste,Top");
        w.add_polygon(&[]);
        w.add_polygon(&[(0.0, 0.0), (1.0, 0.0)]);
        w.add_polygon(&[(0.0, 0.0), (0.0, 0.0), (0.0, 0.0), (0.0, 0.0)]);
        assert_eq!(body_of(&w.render()), vec!["M02*"]);
    }

    #[test]
    fn rect_outline_strokes_four_sides() {
        let mut w = GerberWriter::new("Paste,Top");
        w.add_rect_outline(0.0, 0.0, 2.0, 1.0, 0.25);
        assert_eq!(
            body_of(&w.render()),
            vec![
                "D10*",
                "X0Y0D02*",
                "X2000000Y0D01*",
                "X2000000Y1000000D01*",
                "X0Y1000000D01*",
                "X0Y0D01*",
                "M02*",
            ]
        );
    }

    #[test]
    fn comment_strips_markers() {
        let mut w = GerberWriter::new("Paste,Top");
        w.comment("  hello *world% \n again  ");
        let t = w.render();
        assert!(t.contains("G04 hello world   again*"), "{t}");
    }

    #[test]
    fn arcs_inject_g75_and_flip_with_mirror() {
        let text = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.2*%\nD10*\nG75*\nG03*\n\
                    X1000000Y0D02*\nX0Y1000000I-1000000J0D01*\nM02*\n";
        let gf = parse_gerber(text, "t").unwrap();
        let mut w = GerberWriter::new("Paste,Top");
        w.add_object(&gf.objects[0], &Transform::default());
        let t = w.render();
        let b = body_of(&t);
        assert_eq!(b[0], "G75*");
        assert_eq!(
            b,
            vec![
                "G75*",
                "D10*",
                "X1000000Y0D02*",
                "G03*",
                "X0Y1000000I-1000000J0D01*",
                "M02*",
            ]
        );
        // mirrored: direction reverses and x is negated
        let mut w = GerberWriter::new("Paste,Top");
        w.add_object(
            &gf.objects[0],
            &Transform {
                mirror: true,
                dx: 0.0,
                dy: 0.0,
            },
        );
        assert_eq!(
            body_of(&w.render()),
            vec![
                "G75*",
                "D10*",
                "X-1000000Y0D02*",
                "G02*",
                "X0Y1000000I1000000J0D01*",
                "M02*",
            ]
        );
    }

    #[test]
    fn polarity_changes_are_emitted_once() {
        let text = "%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.2*%\nD10*\n\
                    X0Y0D03*\n%LPC*%\nX1000000Y0D03*\nX2000000Y0D03*\n%LPD*%\nX3000000Y0D03*\nM02*\n";
        let gf = parse_gerber(text, "t").unwrap();
        let mut w = GerberWriter::new("Paste,Top");
        for o in &gf.objects {
            w.add_object(o, &Transform::default());
        }
        assert_eq!(
            body_of(&w.render()),
            vec![
                "D10*",
                "X0Y0D03*",
                "%LPC*%",
                "X1000000Y0D03*",
                "X2000000Y0D03*",
                "%LPD*%",
                "X3000000Y0D03*",
                "M02*",
            ]
        );
    }

    #[test]
    fn macro_apertures_are_baked() {
        let text = "%FSLAX46Y46*%\n%MOMM*%\n\
                    %AMRR*\n4,1,4,-0.5,-0.25,0.5,-0.25,0.5,0.25,-0.5,0.25,-0.5,-0.25,0*%\n\
                    %ADD10RR*%\nD10*\nX0Y0D03*\nM02*\n";
        let gf = parse_gerber(text, "t").unwrap();
        let mut w = GerberWriter::new("Paste,Top");
        w.add_object(&gf.objects[0], &Transform::default());
        let t = w.render();
        assert!(t.contains("%AMM1*\n"), "{t}");
        assert!(t.contains("%ADD10M1*%"), "{t}");
        assert!(t.contains("4,1,4,-0.500000,-0.250000,0.500000,-0.250000,0.500000,0.250000,-0.500000,0.250000,-0.500000,-0.250000,0*"));
    }

    #[test]
    fn nm_rounds_half_to_even() {
        assert_eq!(nm(0.0000005), 0); // 0.5 -> 0
        assert_eq!(nm(0.0000015), 2); // 1.5 -> 2
        assert_eq!(nm(-0.0000005), 0);
        assert_eq!(nm(-0.0000015), -2);
        assert_eq!(nm(1.234567), 1234567);
    }
}
