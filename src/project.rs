//! Discovery of gerber sets (zips or directories) and layer classification.
//!
//! A "source" is one zip file or one directory holding the gerber files of a
//! single KiCad project. Every source yields at most one [`Project`] with up
//! to two [`Side`]s (top / bottom).

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use crate::gerber::{parse_gerber, GerberFile};
use crate::model::{Bounds, Project, Side, SIDE_BOTTOM, SIDE_TOP};

/// Layer roles we care about, in the order they are parsed / searched.
pub const ROLES: [&str; 5] = [
    "copper_top",
    "copper_bottom",
    "paste_top",
    "paste_bottom",
    "outline",
];

/// File extensions that are never gerber files.
pub const SKIP_EXTENSIONS: [&str; 13] = [
    ".drl", ".xln", ".pdf", ".png", ".jpg", ".zip", ".csv", ".txt", ".md", ".json", ".gbrjob",
    ".step", ".stp",
];

/// Prefixes stripped from a source basename when it is used as a project name.
const NAME_PREFIXES: [&str; 3] = ["GERBER-", "gerber-", "gerbers-"];

/// Only the first this many characters of a file are searched for X2 headers.
const HEADER_CHARS: usize = 4000;

/// role -> (file name, text) for the layers of one source.
type SourceRoles = Vec<(&'static str, (String, String))>;

/// Roles that make a source worth building a project from.
const DRAWN_ROLES: [&str; 4] = ["copper_top", "copper_bottom", "paste_top", "paste_bottom"];

// --------------------------------------------------------------------------- //
// Layer classification
// --------------------------------------------------------------------------- //

/// The first `HEADER_CHARS` *characters* of `text` (Python slices by character).
fn header(text: &str) -> &str {
    match text.char_indices().nth(HEADER_CHARS) {
        Some((i, _)) => &text[..i],
        None => text,
    }
}

/// `%TF.<name>,<value>*%` -> `value`; mirrors `re.search(r"%TF\.NAME,([^*%]*)\*?%")`.
fn tf_value(text: &str, name: &str) -> Option<String> {
    let prefix = format!("%TF.{name},");
    let bytes = text.as_bytes();
    let mut from = 0usize;
    while let Some(rel) = text[from..].find(&prefix) {
        let start = from + rel + prefix.len();
        let mut end = start;
        while end < bytes.len() && bytes[end] != b'*' && bytes[end] != b'%' {
            end += 1;
        }
        // `\*?%`: an optional star, then a percent sign.
        let mut tail = end;
        if tail < bytes.len() && bytes[tail] == b'*' {
            tail += 1;
        }
        if tail < bytes.len() && bytes[tail] == b'%' {
            return Some(text[start..end].to_string());
        }
        from = from + rel + 1;
    }
    None
}

/// Map an X2 `.FileFunction` value to a role (or None when irrelevant).
fn role_from_file_function(value: &str) -> Option<&'static str> {
    let fields: Vec<&str> = value.split(',').map(|f| f.trim()).collect();
    let kind = fields.first().map(|f| f.to_lowercase()).unwrap_or_default();
    if kind == "copper" {
        // Copper,L<n>,Top|Bot|Inr
        let side = fields.get(2).map(|f| f.to_lowercase()).unwrap_or_default();
        if side.starts_with("top") {
            return Some("copper_top");
        }
        if side.starts_with("bot") {
            return Some("copper_bottom");
        }
        return None; // inner layer
    }
    if kind == "paste" || kind == "solderpaste" {
        let side = fields.get(1).map(|f| f.to_lowercase()).unwrap_or_default();
        if side.starts_with("top") {
            return Some("paste_top");
        }
        if side.starts_with("bot") {
            return Some("paste_bottom");
        }
        return None;
    }
    if kind == "profile" {
        return Some("outline");
    }
    None
}

/// `os.path.basename` for a "/"-separated member name.
fn basename(filename: &str) -> &str {
    let normalised = filename;
    match normalised.rfind('/') {
        Some(i) => &normalised[i + 1..],
        None => normalised,
    }
}

/// `os.path.splitext`: a leading dot never starts an extension.
fn splitext(base: &str) -> (&str, &str) {
    match base.rfind('.') {
        Some(i) if i > 0 => (&base[..i], &base[i..]),
        _ => (base, ""),
    }
}

/// Guess a role from the file name for gerbers without an X2 header.
fn role_from_filename(filename: &str) -> Option<&'static str> {
    let base = basename(filename).to_lowercase();
    let (stem, ext) = splitext(&base);
    if base.contains("-f_paste") || base.contains("-pastetop") || ext == ".gtp" {
        return Some("paste_top");
    }
    if base.contains("-b_paste") || base.contains("-pastebottom") || ext == ".gbp" {
        return Some("paste_bottom");
    }
    if base.contains("-f_cu") || base.contains("-cutop") || ext == ".gtl" {
        return Some("copper_top");
    }
    if base.contains("-b_cu") || base.contains("-cubottom") || ext == ".gbl" {
        return Some("copper_bottom");
    }
    if base.contains("-edge_cuts") || base.contains("-edgecuts") || ext == ".gm1" || ext == ".gko" {
        return Some("outline");
    }
    if ext == ".gbr" && (stem.contains("outline") || stem.contains("profile")) {
        return Some("outline");
    }
    None
}

/// One of "copper_top", "copper_bottom", "paste_top", "paste_bottom", "outline", or None.
/// Prefers the X2 `%TF.FileFunction` header, falls back to KiCad file name patterns.
pub fn classify_layer(filename: &str, text: &str) -> Option<&'static str> {
    match tf_value(header(text), "FileFunction") {
        Some(value) => role_from_file_function(&value),
        None => role_from_filename(filename),
    }
}

/// True for gerber *text* produced by stencicrity (or its python ancestor).
fn is_own_output_text(text: &str) -> bool {
    let software = tf_value(header(text), "GenerationSoftware")
        .unwrap_or_default()
        .to_lowercase();
    software.contains("pcbstencil") || software.contains("stencicrity")
}

// --------------------------------------------------------------------------- //
// Loading sources
// --------------------------------------------------------------------------- //

/// True for archive/directory members that cannot be gerber files.
fn skip_member(name: &str) -> bool {
    let normalised = name.replace('\\', "/");
    let parts: Vec<&str> = normalised.split('/').filter(|p| !p.is_empty()).collect();
    let Some((base, leading)) = parts.split_last() else {
        return true;
    };
    if leading.iter().any(|p| p.starts_with('.')) || parts[0] == "__MACOSX" {
        return true;
    }
    if base.starts_with('.') {
        return true;
    }
    let ext = splitext(base).1.to_lowercase();
    SKIP_EXTENSIONS.contains(&ext.as_str())
}

/// Collect directory members recursively, hidden directories skipped, sorted.
fn walk_dir(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) -> std::io::Result<()> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            if !name.starts_with('.') {
                dirs.push(entry.path());
            }
        } else {
            files.push(entry.path());
        }
    }
    dirs.sort();
    files.sort();
    for file in files {
        let rel = file
            .strip_prefix(root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        if skip_member(&rel) {
            continue;
        }
        let bytes = std::fs::read(&file)?;
        out.push((rel, String::from_utf8_lossy(&bytes).into_owned()));
    }
    for sub in dirs {
        walk_dir(root, &sub, out)?;
    }
    Ok(())
}

/// `{relative file name: text}` for the candidate gerber files of a zip or a directory.
pub fn load_source(path: &str) -> anyhow::Result<Vec<(String, String)>> {
    let p = Path::new(path);
    let is_dir = p.is_dir();
    if !is_dir {
        if let Ok(file) = std::fs::File::open(p) {
            if let Ok(mut archive) = zip::ZipArchive::new(file) {
                let mut out: Vec<(String, String)> = Vec::new();
                let mut seen: HashMap<String, usize> = HashMap::new();
                for i in 0..archive.len() {
                    let mut member = archive.by_index(i)?;
                    if member.is_dir() {
                        continue;
                    }
                    let name = member.name().to_string();
                    if skip_member(&name) {
                        continue;
                    }
                    let mut bytes = Vec::new();
                    std::io::Read::read_to_end(&mut member, &mut bytes)?;
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    // A dict keeps the first position but the last value.
                    match seen.get(&name) {
                        Some(&at) => out[at].1 = text,
                        None => {
                            seen.insert(name.clone(), out.len());
                            out.push((name, text));
                        }
                    }
                }
                return Ok(out);
            }
        }
    }
    if is_dir {
        let mut out = Vec::new();
        walk_dir(p, p, &mut out)?;
        return Ok(out);
    }
    anyhow::bail!(
        "{} is neither a zip archive nor a directory",
        crate::config::py_repr(path)
    )
}

// --------------------------------------------------------------------------- //
// Discovery
// --------------------------------------------------------------------------- //

/// Map role -> (file name, text) for one source; first file of a role wins.
fn classify_source(path: &str, warn: &mut dyn FnMut(&str)) -> SourceRoles {
    let mut roles: SourceRoles = Vec::new();
    let files = match load_source(path) {
        Ok(files) => files,
        Err(exc) => {
            warn(&format!("{path}: cannot read source ({exc})"));
            return roles;
        }
    };
    let mut names: Vec<&(String, String)> = files.iter().collect();
    names.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, text) in names {
        if is_own_output_text(text) {
            continue; // our own stencil output
        }
        let Some(role) = classify_layer(name, text) else {
            continue;
        };
        if let Some((_, (kept, _))) = roles.iter().find(|(r, _)| *r == role) {
            warn(&format!(
                "{path}: duplicate {role} layer {}, keeping {}",
                crate::config::py_repr(name),
                crate::config::py_repr(kept)
            ));
            continue;
        }
        roles.push((role, (name.clone(), text.clone())));
    }
    roles
}

fn role_entry<'a>(
    roles: &'a [(&'static str, (String, String))],
    role: &str,
) -> Option<&'a (String, String)> {
    roles.iter().find(|(r, _)| *r == role).map(|(_, e)| e)
}

/// `~/x` -> `$HOME/x`.
fn expanduser(path: &str) -> String {
    if path == "~" {
        return std::env::var("HOME").unwrap_or_else(|_| path.to_string());
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{}/{rest}", home.trim_end_matches('/'));
        }
    }
    path.to_string()
}

/// `os.path.abspath`: join with the process cwd, then normalise lexically.
fn abspath(path: &str) -> String {
    let p = Path::new(path);
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(p)
    };
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out.to_string_lossy().into_owned()
}

/// Zips directly in `cwd` plus immediate subdirectories holding gerbers.
fn default_sources(
    cwd: &str,
    excluded: &[String],
    warn: &mut dyn FnMut(&str),
    cache: &mut HashMap<String, SourceRoles>,
) -> Vec<String> {
    let mut entries: Vec<String> = match std::fs::read_dir(cwd) {
        Ok(iter) => iter
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect(),
        Err(exc) => {
            warn(&format!("{cwd}: cannot list directory ({exc})"));
            return Vec::new();
        }
    };
    entries.sort_by_key(|a| (a.to_lowercase(), a.clone()));

    let mut zips: Vec<String> = Vec::new();
    let mut dirs: Vec<String> = Vec::new();
    for entry in entries {
        if entry.starts_with('.') {
            continue;
        }
        let full_path = Path::new(cwd).join(&entry);
        let full = full_path.to_string_lossy().into_owned();
        if excluded.contains(&abspath(&full)) {
            continue;
        }
        if full_path.is_file() && entry.to_lowercase().ends_with(".zip") {
            zips.push(full);
        } else if full_path.is_dir() {
            let roles = classify_source(&full, warn);
            if !roles.is_empty() {
                cache.insert(full.clone(), roles);
                dirs.push(full);
            }
        }
    }
    zips.extend(dirs);
    zips
}

/// Fallback project name: the source basename without extension/prefix.
fn source_name(source: &str) -> String {
    let trimmed = source.trim_end_matches('/');
    let trimmed = if trimmed.is_empty() { source } else { trimmed };
    let mut base = basename(trimmed).to_string();
    if !Path::new(source).is_dir() {
        base = splitext(&base).0.to_string();
    }
    for prefix in NAME_PREFIXES {
        if let Some(rest) = base.strip_prefix(prefix) {
            base = rest.to_string();
            break;
        }
    }
    if base.is_empty() {
        source.to_string()
    } else {
        base
    }
}

/// First field of `%TF.ProjectId` of any parsed layer.
fn project_id_name(layers: &[(&'static str, GerberFile)]) -> Option<String> {
    for role in ROLES {
        let Some((_, gf)) = layers.iter().find(|(r, _)| *r == role) else {
            continue;
        };
        let project_id = gf.file_attrs.get("ProjectId").cloned().unwrap_or_default();
        let name = project_id
            .split(',')
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        if !name.is_empty() {
            return Some(name);
        }
    }
    None
}

fn union_bounds(boxes: &[Bounds]) -> Option<Bounds> {
    if boxes.is_empty() {
        return None;
    }
    Some((
        boxes.iter().map(|b| b.0).fold(f64::INFINITY, f64::min),
        boxes.iter().map(|b| b.1).fold(f64::INFINITY, f64::min),
        boxes.iter().map(|b| b.2).fold(f64::NEG_INFINITY, f64::max),
        boxes.iter().map(|b| b.3).fold(f64::NEG_INFINITY, f64::max),
    ))
}

/// Parse the relevant layers of one source and assemble a Project.
fn build_project(
    source: &str,
    roles: &[(&'static str, (String, String))],
    mirror_bottom: bool,
    warn: &mut dyn FnMut(&str),
) -> Option<Project> {
    let mut layers: Vec<(&'static str, GerberFile)> = Vec::new();
    for role in ROLES {
        let Some((name, text)) = role_entry(roles, role) else {
            continue;
        };
        match parse_gerber(text, name) {
            Ok(gf) => layers.push((role, gf)),
            Err(exc) => warn(&format!(
                "{source}: cannot parse {} ({exc})",
                crate::config::py_repr(name)
            )),
        }
    }
    let has = |role: &str| layers.iter().any(|(r, _)| *r == role);
    if !DRAWN_ROLES.iter().any(|role| has(role)) {
        warn(&format!(
            "{source}: no copper or paste layer found, skipped"
        ));
        return None;
    }

    let outline_at = layers.iter().position(|(r, _)| *r == "outline");
    let mut bbox = outline_at.and_then(|i| layers[i].1.bounds());
    if bbox.is_none() {
        let boxes: Vec<Bounds> = DRAWN_ROLES
            .iter()
            .filter_map(|role| layers.iter().find(|(r, _)| r == role))
            .filter_map(|(_, gf)| gf.bounds())
            .collect();
        bbox = union_bounds(&boxes);
    }
    let Some(bbox) = bbox else {
        warn(&format!(
            "{source}: no drawable objects on any layer, skipped"
        ));
        return None;
    };

    let name = project_id_name(&layers).unwrap_or_else(|| source_name(source));

    // Move the parsed layers out by role.
    let mut taken: HashMap<&'static str, GerberFile> = layers.into_iter().collect();
    let outline = taken.remove("outline");
    let copper_top = taken.remove("copper_top");
    let copper_bottom = taken.remove("copper_bottom");
    let paste_top = taken.remove("paste_top");
    let paste_bottom = taken.remove("paste_bottom");

    let mut sides = Vec::new();
    if copper_top.is_some() || paste_top.is_some() {
        sides.push(Side {
            project_name: name.clone(),
            board_bbox: bbox,
            name: SIDE_TOP.to_string(),
            copper: copper_top,
            paste: paste_top,
            mirror: false,
            enabled: true,
            pads: Vec::new(),
        });
    }
    if copper_bottom.is_some() || paste_bottom.is_some() {
        sides.push(Side {
            project_name: name.clone(),
            board_bbox: bbox,
            name: SIDE_BOTTOM.to_string(),
            copper: copper_bottom,
            paste: paste_bottom,
            mirror: mirror_bottom,
            enabled: true,
            pads: Vec::new(),
        });
    }
    Some(Project {
        name,
        source: source.to_string(),
        outline,
        bbox,
        sides,
    })
}

/// Make project names unique by appending ' (2)', ' (3)', ...
fn deduplicate(projects: &mut [Project]) {
    let mut seen: HashMap<String, usize> = HashMap::new();
    for project in projects.iter_mut() {
        let count = seen.get(&project.name).copied().unwrap_or(0) + 1;
        seen.insert(project.name.clone(), count);
        if count > 1 {
            project.name = format!("{} ({count})", project.name);
            for side in project.sides.iter_mut() {
                side.project_name = project.name.clone();
            }
        }
    }
}

/// Options for `discover_projects`.
#[derive(Clone, Debug, Default)]
pub struct DiscoverOptions {
    pub mirror_bottom: bool,
    /// absolute paths never scanned
    pub exclude_dirs: Vec<String>,
}

/// Find projects in explicit inputs, or (when `inputs` is empty) every `*.zip`
/// and gerber subdirectory of `cwd`. Warnings go to `warn`.
pub fn discover_projects(
    inputs: &[String],
    cwd: &str,
    opts: &DiscoverOptions,
    warn: &mut dyn FnMut(&str),
) -> anyhow::Result<Vec<Project>> {
    let excluded: Vec<String> = opts
        .exclude_dirs
        .iter()
        .map(|p| abspath(&expanduser(p)))
        .collect();
    let mut cache: HashMap<String, SourceRoles> = HashMap::new();

    let sources: Vec<String> = if !inputs.is_empty() {
        inputs.iter().map(|p| expanduser(p)).collect()
    } else {
        default_sources(cwd, &excluded, warn, &mut cache)
    };

    let mut projects: Vec<Project> = Vec::new();
    for source in sources {
        if excluded.contains(&abspath(&source)) {
            continue;
        }
        if !Path::new(&source).exists() {
            warn(&format!("{source}: no such file or directory, skipped"));
            continue;
        }
        let roles = match cache.remove(&source) {
            Some(roles) => roles,
            None => classify_source(&source, warn),
        };
        if roles.is_empty() {
            warn(&format!("{source}: no gerber layers recognised, skipped"));
            continue;
        }
        if let Some(project) = build_project(&source, &roles, opts.mirror_bottom, warn) {
            projects.push(project);
        }
    }
    deduplicate(&mut projects);
    Ok(projects)
}

/// True for gerbers produced by stencicrity itself (never an input).
pub fn is_own_output(gf: &GerberFile) -> bool {
    gf.file_attrs
        .get("GenerationSoftware")
        .map(|v| {
            v.to_lowercase().contains("pcbstencil") || v.to_lowercase().contains("stencicrity")
        })
        .unwrap_or(false)
}
