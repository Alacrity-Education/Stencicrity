"""Discovery of PCB projects: find gerber sets, classify layers, build Projects.

A "source" is one zip file or one directory holding the gerber files of a
single KiCad project.  Every source yields at most one :class:`~.model.Project`
with up to two :class:`~.model.Side` objects (top / bottom).
"""
from __future__ import annotations

import os
import re
import sys
import zipfile
from typing import Callable, Optional

from .gerber import GerberError, GerberFile, parse_gerber
from .model import SIDE_BOTTOM, SIDE_TOP, Project, Side

__all__ = ["classify_layer", "load_source", "discover_projects",
           "ROLES", "SKIP_EXTENSIONS"]

#: Layer roles we care about, in the order they are parsed / searched.
ROLES = ("copper_top", "copper_bottom", "paste_top", "paste_bottom", "outline")

#: File extensions that are never gerber files.
SKIP_EXTENSIONS = frozenset({
    ".drl", ".xln", ".pdf", ".png", ".jpg", ".zip", ".csv", ".txt", ".md",
    ".json", ".gbrjob", ".step", ".stp",
})

#: Prefixes stripped from a source basename when it is used as a project name.
_NAME_PREFIXES = ("GERBER-", "gerber-", "gerbers-")

_HEADER_CHARS = 4000
_FILE_FUNCTION_RE = re.compile(r"%TF\.FileFunction,([^*%]*)\*?%")
_GENERATION_SOFTWARE_RE = re.compile(r"%TF\.GenerationSoftware,([^*%]*)\*?%")

Warn = Callable[[str], None]


def _default_warn(message: str) -> None:
    """Print a warning to stderr (the default for the ``warn`` parameters)."""
    print(f"warning: {message}", file=sys.stderr)


def _warner(warn: Optional[Warn]) -> Warn:
    return warn if warn is not None else _default_warn


# --------------------------------------------------------------------------- #
# Layer classification
# --------------------------------------------------------------------------- #
def _role_from_file_function(value: str) -> Optional[str]:
    """Map an X2 ``.FileFunction`` value to a role (or None when irrelevant)."""
    fields = [f.strip() for f in value.split(",")]
    kind = fields[0].lower() if fields else ""
    if kind == "copper":
        # Copper,L<n>,Top|Bot|Inr
        side = fields[2].lower() if len(fields) > 2 else ""
        if side.startswith("top"):
            return "copper_top"
        if side.startswith("bot"):
            return "copper_bottom"
        return None                      # inner layer
    if kind in ("paste", "solderpaste"):
        side = fields[1].lower() if len(fields) > 1 else ""
        if side.startswith("top"):
            return "paste_top"
        if side.startswith("bot"):
            return "paste_bottom"
        return None
    if kind == "profile":
        return "outline"
    return None


def _role_from_filename(filename: str) -> Optional[str]:
    """Guess a role from the file name for gerbers without an X2 header."""
    base = os.path.basename(filename).lower()
    stem, ext = os.path.splitext(base)
    if "-f_paste" in base or "-pastetop" in base or ext == ".gtp":
        return "paste_top"
    if "-b_paste" in base or "-pastebottom" in base or ext == ".gbp":
        return "paste_bottom"
    if "-f_cu" in base or "-cutop" in base or ext == ".gtl":
        return "copper_top"
    if "-b_cu" in base or "-cubottom" in base or ext == ".gbl":
        return "copper_bottom"
    if "-edge_cuts" in base or "-edgecuts" in base or ext in (".gm1", ".gko"):
        return "outline"
    if ext == ".gbr" and ("outline" in stem or "profile" in stem):
        return "outline"
    return None


def classify_layer(filename: str, text: str) -> str | None:
    """Return the role of a gerber file, or None when it is not needed.

    The X2 ``%TF.FileFunction`` header wins when present (it is authoritative);
    otherwise the file name is matched against the usual KiCad / CAM patterns.
    """
    match = _FILE_FUNCTION_RE.search(text[:_HEADER_CHARS])
    if match:
        return _role_from_file_function(match.group(1))
    return _role_from_filename(filename)


def _generation_software(text: str) -> str:
    match = _GENERATION_SOFTWARE_RE.search(text[:_HEADER_CHARS])
    return match.group(1) if match else ""


def _is_own_output(text: str) -> bool:
    """True for gerbers produced by pcbstencil itself (never an input)."""
    return "pcbstencil" in _generation_software(text).lower()


# --------------------------------------------------------------------------- #
# Loading sources
# --------------------------------------------------------------------------- #
def _skip_member(name: str) -> bool:
    """True for archive/directory members that cannot be gerber files."""
    parts = [p for p in name.replace("\\", "/").split("/") if p]
    if not parts:
        return True
    if any(p.startswith(".") for p in parts[:-1]) or parts[0] == "__MACOSX":
        return True
    base = parts[-1]
    if base.startswith("."):
        return True
    return os.path.splitext(base)[1].lower() in SKIP_EXTENSIONS


def load_source(path: str) -> dict[str, str]:
    """Read every candidate gerber file of a zip archive or a directory.

    Returns ``{relative file name: text}``; files with an extension that is
    never a gerber (see :data:`SKIP_EXTENSIONS`) are skipped.  Text is decoded
    as utf-8 with ``errors="replace"``.
    """
    files: dict[str, str] = {}
    if zipfile.is_zipfile(path) and not os.path.isdir(path):
        with zipfile.ZipFile(path) as zf:
            for info in zf.infolist():
                if info.is_dir() or _skip_member(info.filename):
                    continue
                with zf.open(info) as fh:
                    files[info.filename] = fh.read().decode("utf-8", errors="replace")
        return files
    if os.path.isdir(path):
        for root, dirnames, filenames in os.walk(path):
            dirnames[:] = sorted(d for d in dirnames if not d.startswith("."))
            for filename in sorted(filenames):
                full = os.path.join(root, filename)
                rel = os.path.relpath(full, path).replace(os.sep, "/")
                if _skip_member(rel):
                    continue
                with open(full, "rb") as fh:
                    files[rel] = fh.read().decode("utf-8", errors="replace")
        return files
    raise FileNotFoundError(f"{path!r} is neither a zip archive nor a directory")


# --------------------------------------------------------------------------- #
# Discovery
# --------------------------------------------------------------------------- #
def _classify_source(path: str, warn: Warn) -> dict[str, tuple[str, str]]:
    """Map role -> (file name, text) for one source; first file of a role wins."""
    roles: dict[str, tuple[str, str]] = {}
    try:
        files = load_source(path)
    except (OSError, zipfile.BadZipFile) as exc:
        warn(f"{path}: cannot read source ({exc})")
        return roles
    for name in sorted(files):
        text = files[name]
        if _is_own_output(text):
            continue                     # our own stencil output
        role = classify_layer(name, text)
        if role is None:
            continue
        if role in roles:
            warn(f"{path}: duplicate {role} layer {name!r}, "
                 f"keeping {roles[role][0]!r}")
            continue
        roles[role] = (name, text)
    return roles


def _abspaths(paths: Optional[list[str]]) -> set[str]:
    return {os.path.abspath(os.path.expanduser(p)) for p in (paths or [])}


def _default_sources(cwd: str, excluded: set[str], warn: Warn,
                     cache: dict[str, dict[str, tuple[str, str]]]) -> list[str]:
    """Zips directly in *cwd* plus immediate subdirectories holding gerbers."""
    try:
        entries = sorted(os.listdir(cwd), key=lambda s: (s.lower(), s))
    except OSError as exc:
        warn(f"{cwd}: cannot list directory ({exc})")
        return []
    zips: list[str] = []
    dirs: list[str] = []
    for entry in entries:
        if entry.startswith("."):
            continue
        full = os.path.join(cwd, entry)
        if os.path.abspath(full) in excluded:
            continue
        if os.path.isfile(full) and entry.lower().endswith(".zip"):
            zips.append(full)
        elif os.path.isdir(full):
            roles = _classify_source(full, warn)
            if roles:
                cache[full] = roles
                dirs.append(full)
    return zips + dirs


def _source_name(source: str) -> str:
    """Fallback project name: the source basename without extension/prefix."""
    base = os.path.basename(os.path.normpath(source))
    if not os.path.isdir(source):
        base = os.path.splitext(base)[0]
    for prefix in _NAME_PREFIXES:
        if base.startswith(prefix):
            base = base[len(prefix):]
            break
    return base or source


def _project_id_name(layers: dict[str, GerberFile]) -> Optional[str]:
    """First field of %TF.ProjectId of any parsed layer."""
    for role in ROLES:
        gf = layers.get(role)
        if gf is None:
            continue
        project_id = gf.file_attrs.get("ProjectId", "")
        name = project_id.split(",")[0].strip()
        if name:
            return name
    return None


def _union_bounds(boxes: list[tuple[float, float, float, float]]
                  ) -> Optional[tuple[float, float, float, float]]:
    if not boxes:
        return None
    return (min(b[0] for b in boxes), min(b[1] for b in boxes),
            max(b[2] for b in boxes), max(b[3] for b in boxes))


def _build_project(source: str, roles: dict[str, tuple[str, str]],
                   mirror_bottom: bool, warn: Warn) -> Optional[Project]:
    """Parse the relevant layers of one source and assemble a Project."""
    layers: dict[str, GerberFile] = {}
    for role in ROLES:
        entry = roles.get(role)
        if entry is None:
            continue
        name, text = entry
        try:
            layers[role] = parse_gerber(text, name)
        except (GerberError, ValueError, IndexError) as exc:
            warn(f"{source}: cannot parse {name!r} ({exc})")
    if not any(role in layers for role in
               ("copper_top", "copper_bottom", "paste_top", "paste_bottom")):
        warn(f"{source}: no copper or paste layer found, skipped")
        return None

    outline = layers.get("outline")
    bbox = outline.bounds() if outline is not None else None
    if bbox is None:
        boxes = []
        for role in ("copper_top", "copper_bottom", "paste_top", "paste_bottom"):
            gf = layers.get(role)
            if gf is None:
                continue
            gb = gf.bounds()
            if gb is not None:
                boxes.append(gb)
        bbox = _union_bounds(boxes)
    if bbox is None:
        warn(f"{source}: no drawable objects on any layer, skipped")
        return None

    name = _project_id_name(layers) or _source_name(source)
    project = Project(name=name, source=source, outline=outline,
                      top=None, bottom=None, bbox=bbox)
    if "copper_top" in layers or "paste_top" in layers:
        project.top = Side(project, SIDE_TOP,
                           layers.get("copper_top"), layers.get("paste_top"))
    if "copper_bottom" in layers or "paste_bottom" in layers:
        project.bottom = Side(project, SIDE_BOTTOM,
                              layers.get("copper_bottom"), layers.get("paste_bottom"),
                              mirror=mirror_bottom)
    return project


def _deduplicate(projects: list[Project]) -> None:
    """Make project names unique by appending ' (2)', ' (3)', ..."""
    seen: dict[str, int] = {}
    for project in projects:
        count = seen.get(project.name, 0) + 1
        seen[project.name] = count
        if count > 1:
            project.name = f"{project.name} ({count})"


def discover_projects(inputs: list[str] | None, cwd: str, *,
                      mirror_bottom: bool = True,
                      exclude_dirs: list[str] | None = None,
                      warn: Optional[Warn] = None) -> list[Project]:
    """Find gerber sets and turn each of them into a :class:`Project`.

    ``inputs`` are explicit zip files or directories.  When it is empty every
    ``*.zip`` directly in *cwd* plus every immediate subdirectory containing at
    least one classifiable gerber file is used.  ``exclude_dirs`` (compared by
    absolute path) and gerbers generated by pcbstencil itself are skipped.
    """
    emit = _warner(warn)
    excluded = _abspaths(exclude_dirs)
    cache: dict[str, dict[str, tuple[str, str]]] = {}

    if inputs:
        sources = [os.path.expanduser(p) for p in inputs]
    else:
        sources = _default_sources(cwd, excluded, emit, cache)

    projects: list[Project] = []
    for source in sources:
        abs_source = os.path.abspath(source)
        if abs_source in excluded:
            continue
        if not os.path.exists(source):
            emit(f"{source}: no such file or directory, skipped")
            continue
        roles = cache.get(source)
        if roles is None:
            roles = _classify_source(source, emit)
        if not roles:
            emit(f"{source}: no gerber layers recognised, skipped")
            continue
        project = _build_project(source, roles, mirror_bottom, emit)
        if project is not None:
            projects.append(project)
    _deduplicate(projects)
    return projects
