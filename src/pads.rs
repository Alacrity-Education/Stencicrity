//! Pad detection on copper layers, default states and ordering.
//!
//! A pad is a flash on a copper layer that belongs to a component footprint.
//! Pads the paste layer already covers are copied to the stencil as they are
//! (state `open`); the user may still *close* one, which drops its paste
//! openings. Pads without paste ("candidates") are offered to the user, who
//! marks them `open` or `ignore`. Candidates whose reference starts with one
//! of the configured ignore prefixes plus a digit (`TP3`, `NT12`) default to
//! `ignore`.

use std::cmp::Ordering;

use geo::{Area, Centroid, Contains};
use rstar::primitives::{GeomWithData, Rectangle};
use rstar::{RTree, AABB};

use crate::gerber::{object_geometry, GraphicObject};
use crate::model::{
    Geom, Pad, Project, Side, SideId, SIDE_TOP, STATES, STATE_IGNORE, STATE_OPEN, STATE_UNDEFINED,
};
use crate::util::natural_key;

pub const SMD_FUNCTIONS: [&str; 6] = [
    "SMDPad",
    "BGAPad",
    "HeatsinkPad",
    "FiducialPad",
    "TestPad",
    "ConnectorPad",
];
pub const THT_FUNCTIONS: [&str; 2] = ["ComponentPad", "CastellatedPad"];
pub const NON_PAD_FUNCTIONS: [&str; 9] = [
    "ViaPad",
    "Conductor",
    "NonConductor",
    "EtchedComponent",
    "Profile",
    "WasherPad",
    "AntiPad",
    "Other",
    "Drawing",
];

pub fn pad_key(project: &str, side: &str, ref_: &str, pin: &str, x: f64, y: f64) -> String {
    format!("{project}/{side}/{ref_}.{pin}@{x:.3},{y:.3}")
}

/// `^(?:PREFIX)\d`, case-insensitive.
pub fn matches_prefix(ref_: &str, prefixes: &[String]) -> bool {
    let lower = ref_.to_lowercase();
    for prefix in prefixes {
        let prefix = prefix.trim();
        if prefix.is_empty() {
            continue;
        }
        let Some(rest) = lower.strip_prefix(prefix.to_lowercase().as_str()) else {
            continue;
        };
        if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return true;
        }
    }
    false
}

/// Pasted pad -> open; candidate whose ref matches an ignore prefix -> ignore; else undefined.
pub fn default_state(pad: &Pad, ignore_prefixes: &[String]) -> &'static str {
    if pad.has_paste {
        return STATE_OPEN;
    }
    if matches_prefix(&pad.ref_, ignore_prefixes) {
        return STATE_IGNORE;
    }
    STATE_UNDEFINED
}

#[derive(Clone, Debug)]
pub struct DetectOptions {
    pub include_tht: bool,
    /// paste must cover at least this fraction of the pad area (or its centroid) to count
    pub min_overlap: f64,
    pub ignore_prefixes: Vec<String>,
}

impl Default for DetectOptions {
    fn default() -> Self {
        Self {
            include_tht: false,
            min_overlap: 0.10,
            ignore_prefixes: crate::model::DEFAULT_IGNORE_PREFIXES
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

/// Decide whether a copper flash is a component pad.
fn is_pad(function: &str, attrs_p: Option<&String>, include_tht: bool) -> bool {
    if NON_PAD_FUNCTIONS.contains(&function) {
        return false;
    }
    if SMD_FUNCTIONS.contains(&function) {
        return true;
    }
    if THT_FUNCTIONS.contains(&function) {
        return include_tht;
    }
    // Unknown or missing AperFunction: trust the object attribute instead.
    attrs_p.is_some_and(|v| !v.is_empty())
}

/// Component reference and pin from the `%TO.P` object attribute.
fn ref_pin(raw: Option<&String>, fallback_index: usize) -> (String, String) {
    let raw = raw.map(String::as_str).unwrap_or("");
    if !raw.is_empty() {
        let parts: Vec<&str> = raw.split(',').map(|p| p.trim()).collect();
        let ref_ = if parts[0].is_empty() {
            "?".to_string()
        } else {
            parts[0].to_string()
        };
        let pin = match parts.get(1) {
            Some(p) if !p.is_empty() => (*p).to_string(),
            _ => fallback_index.to_string(),
        };
        return (ref_, pin);
    }
    ("?".to_string(), fallback_index.to_string())
}

type PasteIndex = GeomWithData<Rectangle<[f64; 2]>, usize>;

fn envelope(geom: &Geom) -> Option<AABB<[f64; 2]>> {
    crate::gerber::geom_bounds(geom).map(|(x0, y0, x1, y1)| AABB::from_corners([x0, y0], [x1, y1]))
}

/// `(the paste layer opens this pad, indices of the openings on it)`.
fn paste_hits(
    geom: &Geom,
    paste_geoms: &[Geom],
    paste_indices: &[usize],
    tree: Option<&RTree<PasteIndex>>,
    min_overlap: f64,
) -> (bool, Vec<usize>) {
    let (Some(tree), false) = (tree, geom.0.is_empty()) else {
        return (false, Vec::new());
    };
    let Some(env) = envelope(geom) else {
        return (false, Vec::new());
    };
    let area = geom.unsigned_area();
    let centroid = geom.centroid();
    let mut covered = 0.0f64;
    let mut centred = false;
    let mut hits: Vec<usize> = Vec::new();

    let mut candidates: Vec<usize> = tree
        .locate_in_envelope_intersecting(env)
        .map(|item| item.data)
        .collect();
    candidates.sort_unstable();
    for index in candidates {
        let paste = &paste_geoms[index];
        let overlap = if area > 0.0 {
            use geo::BooleanOps;
            geom.intersection(paste).unsigned_area()
        } else {
            0.0
        };
        let inside = centroid.is_some_and(|c| paste.contains(&c));
        if inside {
            centred = true;
        }
        if inside || overlap > 0.0 {
            hits.push(paste_indices[index]);
        }
        covered += overlap;
    }
    let has_paste = centred || (area > 0.0 && covered >= min_overlap * area);
    hits.sort_unstable();
    (has_paste, hits)
}

/// Fill `side.pads` from the copper flashes (with paste_indices and default states).
pub fn detect_pads(side: &mut Side, opts: &DetectOptions) {
    // Usable paste geometries and, for each, its index in `side.paste_objects()`.
    let mut paste_geoms: Vec<Geom> = Vec::new();
    let mut paste_indices: Vec<usize> = Vec::new();
    for (index, obj) in side.paste_objects().iter().enumerate() {
        let geom = object_geometry(obj);
        if !geom.0.is_empty() {
            paste_geoms.push(geom);
            paste_indices.push(index);
        }
    }
    let tree: Option<RTree<PasteIndex>> = if paste_geoms.is_empty() {
        None
    } else {
        let items: Vec<PasteIndex> = paste_geoms
            .iter()
            .enumerate()
            .filter_map(|(i, g)| envelope(g).map(|e| GeomWithData::new(Rectangle::from_aabb(e), i)))
            .collect();
        Some(RTree::bulk_load(items))
    };

    let project_name = side.project_name.clone();
    let side_name = side.name.clone();
    let mut pads: Vec<Pad> = Vec::new();
    let mut unnamed = 0usize;
    for obj in side.copper_objects() {
        let GraphicObject::Flash(flash) = obj else {
            continue;
        };
        if !flash.dark {
            continue;
        }
        let function = flash.aperture.function();
        let attrs_p = flash.attrs.get("P");
        if !is_pad(&function, attrs_p, opts.include_tht) {
            continue;
        }
        if attrs_p.map(|v| v.is_empty()).unwrap_or(true) {
            unnamed += 1;
        }
        let (ref_, pin) = ref_pin(flash.attrs.get("P"), unnamed);
        let geom = object_geometry(obj);
        let (has_paste, covering) = paste_hits(
            &geom,
            &paste_geoms,
            &paste_indices,
            tree.as_ref(),
            opts.min_overlap,
        );
        let mut pad = Pad {
            key: pad_key(&project_name, &side_name, &ref_, &pin, flash.x, flash.y),
            project: project_name.clone(),
            side: side_name.clone(),
            ref_,
            pin,
            x: flash.x,
            y: flash.y,
            function: flash.aperture.function(),
            shape: flash.aperture.describe(),
            flash: flash.clone(),
            geom,
            has_paste,
            state: STATE_UNDEFINED.to_string(),
            paste_indices: if has_paste { covering } else { Vec::new() },
        };
        pad.state = default_state(&pad, &opts.ignore_prefixes).to_string();
        pads.push(pad);
    }

    pads.sort_by(|a, b| {
        natural_key(&a.ref_)
            .cmp(&natural_key(&b.ref_))
            .then_with(|| natural_key(&a.pin).cmp(&natural_key(&b.pin)))
            .then_with(|| a.x.total_cmp(&b.x))
            .then_with(|| a.y.total_cmp(&b.y))
    });
    side.pads = pads;
}

fn side_order(name: &str) -> u8 {
    if name == SIDE_TOP {
        0
    } else {
        1
    }
}

fn pad_order(
    pad: &Pad,
) -> (
    Vec<crate::util::Chunk>,
    u8,
    Vec<crate::util::Chunk>,
    Vec<crate::util::Chunk>,
) {
    (
        natural_key(&pad.project),
        side_order(&pad.side),
        natural_key(&pad.ref_),
        natural_key(&pad.pin),
    )
}

/// Every pad (or only candidates) of the given sides, ordered by
/// (natural project name, top before bottom, natural ref, natural pin, x, y).
pub fn sorted_pads(projects: &[Project], sides: &[SideId], all_pads: bool) -> Vec<(SideId, usize)> {
    let mut out: Vec<(SideId, usize)> = Vec::new();
    for id in sides {
        let side = id.get(projects);
        for (index, pad) in side.pads.iter().enumerate() {
            if all_pads || pad.is_candidate() {
                out.push((*id, index));
            }
        }
    }
    out.sort_by(|a, b| {
        let pa = &a.0.get(projects).pads[a.1];
        let pb = &b.0.get(projects).pads[b.1];
        pad_order(pa)
            .cmp(&pad_order(pb))
            .then_with(|| pa.x.total_cmp(&pb.x))
            .then_with(|| pa.y.total_cmp(&pb.y))
            .then(Ordering::Equal)
    });
    out
}

/// (undefined, open, ignore) counts over candidates.
pub fn state_counts<'a>(pads: impl Iterator<Item = &'a Pad>) -> (usize, usize, usize) {
    let mut counts = [0usize; 3];
    for pad in pads {
        match STATES.iter().position(|s| *s == pad.state) {
            Some(i) => counts[i] += 1,
            None => counts[0] += 1,
        }
    }
    (counts[0], counts[1], counts[2])
}

/// How many of `pads` are pasted pads the user closed (state ignore).
pub fn closed_count<'a>(pads: impl Iterator<Item = &'a Pad>) -> usize {
    pads.filter(|p| p.is_closed()).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefixes(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn prefix_matching() {
        let p = prefixes(&["NT", "TP"]);
        assert!(matches_prefix("TP1", &p));
        assert!(matches_prefix("NT12", &p));
        assert!(matches_prefix("tp3", &p));
        assert!(!matches_prefix("TPS1", &p));
        assert!(!matches_prefix("T1", &p));
        assert!(!matches_prefix("R1", &p));
        assert!(!matches_prefix("TP", &p));
        assert!(!matches_prefix("TP1", &[]));
        // blank entries never match
        assert!(!matches_prefix("R1", &prefixes(&["", "   "])));
    }

    fn pad_with(ref_: &str, has_paste: bool) -> Pad {
        Pad {
            key: String::new(),
            project: "p".into(),
            side: "top".into(),
            ref_: ref_.into(),
            pin: "1".into(),
            x: 0.0,
            y: 0.0,
            function: String::new(),
            shape: String::new(),
            flash: crate::gerber::Flash {
                x: 0.0,
                y: 0.0,
                aperture: std::rc::Rc::new(crate::gerber::Aperture::new(
                    10,
                    "C",
                    vec![1.0],
                    Default::default(),
                    None,
                )),
                attrs: Default::default(),
                dark: true,
            },
            geom: Geom::new(Vec::new()),
            has_paste,
            state: STATE_UNDEFINED.to_string(),
            paste_indices: Vec::new(),
        }
    }

    #[test]
    fn states() {
        let p = prefixes(&["NT", "TP"]);
        assert_eq!(default_state(&pad_with("R1", true), &p), STATE_OPEN);
        assert_eq!(default_state(&pad_with("TP1", true), &p), STATE_OPEN);
        assert_eq!(default_state(&pad_with("TP1", false), &p), STATE_IGNORE);
        assert_eq!(default_state(&pad_with("NT9", false), &p), STATE_IGNORE);
        assert_eq!(default_state(&pad_with("R1", false), &p), STATE_UNDEFINED);
        assert_eq!(default_state(&pad_with("TPS1", false), &p), STATE_UNDEFINED);
    }

    #[test]
    fn counting_states() {
        let mut pads = [
            pad_with("R1", false),
            pad_with("R2", false),
            pad_with("R3", false),
        ];
        pads[1].state = STATE_OPEN.to_string();
        pads[2].state = STATE_IGNORE.to_string();
        assert_eq!(state_counts(pads.iter()), (1, 1, 1));
        // an unknown state counts as undefined
        pads[0].state = "nonsense".to_string();
        assert_eq!(state_counts(pads.iter()), (1, 1, 1));
    }

    #[test]
    fn pad_keys() {
        assert_eq!(
            pad_key("RBARF", "bottom", "TP1", "1", 148.0815, -99.5678),
            "RBARF/bottom/TP1.1@148.082,-99.568"
        );
    }

    #[test]
    fn ref_pin_fallbacks() {
        assert_eq!(
            ref_pin(Some(&"U1,3,VDD".to_string()), 7),
            ("U1".to_string(), "3".to_string())
        );
        assert_eq!(
            ref_pin(Some(&"U1".to_string()), 7),
            ("U1".to_string(), "7".to_string())
        );
        assert_eq!(
            ref_pin(Some(&"U1,".to_string()), 7),
            ("U1".to_string(), "7".to_string())
        );
        assert_eq!(
            ref_pin(Some(&",2".to_string()), 7),
            ("?".to_string(), "2".to_string())
        );
        assert_eq!(ref_pin(None, 7), ("?".to_string(), "7".to_string()));
    }

    #[test]
    fn natural_pad_ordering() {
        let mut refs = vec!["R10", "R2", "C1", "TP12", "TP3", "r1"];
        refs.sort_by_key(|r| natural_key(r));
        assert_eq!(refs, vec!["C1", "r1", "R2", "R10", "TP3", "TP12"]);
    }
}
