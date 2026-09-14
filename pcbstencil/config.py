"""The ``.stencil`` file: the whole project configuration in one text file.

It stores the stencil sheet (size and orientation), the layout parameters, the
rules that give fresh pads their default state, the on/off switch of every
board side and the open/ignore decision of every copper pad without a paste
opening (plus the pads *with* paste whose opening the user closed).  The file
is written next to the gerbers as soon as the projects are known, updated when
the TUI exits and is meant to be readable and editable by hand::

    [stencil]
    size = 380x280
    orientation = landscape

    [layout]
    spacing = 30.0
    holes = on
    hole_grid = 8.0
    ...

    [rules]
    ignore_prefixes = NT TP

    [sides]
    RBARF/top = on

    [pads]
    RBARF/bottom/TP1.1@148.082,-99.568 = open

Sections, keys and values are case insensitive; ``#`` starts a comment.  Keys
that no longer match anything in the current gerbers are kept at the end of
their section so hand made decisions are never lost.  A flat file in the old
format (only ``<pad key> = <state>`` lines, no sections) still loads: lines
before the first section header are read as ``[pads]``.
"""
from __future__ import annotations

import os
import re
import sys
import tempfile
from datetime import datetime
from typing import Callable, Optional

from .model import (
    ORIENTATIONS,
    SORT_ORDERS,
    STATE_IGNORE,
    STATE_OPEN,
    STATE_UNDEFINED,
    STATES,
    STENCIL_SIZES,
    Config,
    Project,
    Side,
    parse_size,
    size_label,
)
from .pads import default_state, natural_key

__all__ = ["load_config", "apply_config", "collect_config", "save_config",
           "format_config", "parse_bool", "parse_prefixes"]

SECTION_STENCIL = "stencil"
SECTION_LAYOUT = "layout"
SECTION_RULES = "rules"
SECTION_SIDES = "sides"
SECTION_PADS = "pads"
SECTIONS = (SECTION_STENCIL, SECTION_LAYOUT, SECTION_RULES, SECTION_SIDES,
            SECTION_PADS)

#: States a pad that already has a paste opening may be in.
PASTED_STATES = (STATE_OPEN, STATE_IGNORE)

_TRUE_WORDS = frozenset({"on", "true", "yes", "1"})
_FALSE_WORDS = frozenset({"off", "false", "no", "0"})

#: ``[layout]`` key -> (LayoutParams attribute, predicate, requirement text).
_LAYOUT_FLOATS: dict[str, tuple[str, Optional[Callable[[float], bool]], str]] = {
    "spacing": ("gap", lambda v: v >= 0.0, "must not be negative"),
    "hole_dia": ("hole_dia", lambda v: v > 0.0, "must be positive"),
    "hole_inset": ("hole_inset", None, ""),
    "dot_dia": ("dot_dia", lambda v: v > 0.0, "must be positive"),
    "dot_pitch": ("dot_pitch", lambda v: v > 0.0, "must be positive"),
    "dot_line_gap": ("dot_line_gap", lambda v: v >= 0.0, "must not be negative"),
    "hole_grid": ("hole_grid", lambda v: v >= 0.0, "must not be negative"),
}
#: ``[layout]`` key -> LayoutParams attribute (booleans).
_LAYOUT_BOOLS = {"holes": "holes", "outer_border": "outer_border"}
#: Accepted spellings of a ``[layout]`` key.
_LAYOUT_ALIASES = {"gap": "spacing", "border": "outer_border"}

_COMMENT_RE = re.compile(r"\s#")
_COMMENT_COLUMN = 26
_STALE_HEADER = "# --- not found in the current gerbers ---"
_CLOSED_HEADER = "# --- pads with paste whose opening is closed (state ignore) ---"
_PREFIX_SPLIT_RE = re.compile(r"[,\s]+")

Warn = Callable[[str], None]


def _default_warn(message: str) -> None:
    print(f"warning: {message}", file=sys.stderr)


def parse_bool(value: str) -> Optional[bool]:
    """``on/true/yes/1`` -> True, ``off/false/no/0`` -> False, else None."""
    text = str(value).strip().lower()
    if text in _TRUE_WORDS:
        return True
    if text in _FALSE_WORDS:
        return False
    return None


def parse_prefixes(value: str) -> tuple[str, ...]:
    """``"nt, TP"`` -> ``("NT", "TP")``; empty text gives an empty tuple."""
    prefixes: list[str] = []
    for token in _PREFIX_SPLIT_RE.split(str(value).strip()):
        upper = token.strip().upper()
        if upper and upper not in prefixes:
            prefixes.append(upper)
    return tuple(prefixes)


# --------------------------------------------------------------------------- #
# Reading
# --------------------------------------------------------------------------- #
def load_config(path: str, *, warn: Optional[Warn] = None) -> Config:
    """Read *path* and return a :class:`~pcbstencil.model.Config`.

    A missing file yields the built-in defaults.  Nothing in here ever raises:
    every unreadable file, unknown key or bad value is reported through *warn*
    (stderr by default) and the default is kept.
    """
    emit = warn if warn is not None else _default_warn
    config = Config()
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as handle:
            lines = handle.readlines()
    except FileNotFoundError:
        return config
    except OSError as exc:
        emit(f"{path}: cannot read the configuration ({exc})")
        return config

    # Lines before the first header belong to [pads] (old, flat format).
    section = SECTION_PADS
    for number, raw in enumerate(lines, 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        line = _COMMENT_RE.split(line, maxsplit=1)[0].strip()
        if not line:
            continue
        where = f"{path}:{number}"
        if line.startswith("[") and line.endswith("]"):
            section = line[1:-1].strip().lower()
            if section not in SECTIONS:
                emit(f"{where}: unknown section [{section}], its lines are ignored")
            continue
        if "=" not in line:
            emit(f"{where}: ignoring line without '=': {line!r}")
            continue
        key, _, value = line.rpartition("=")
        key, value = key.strip(), value.strip()
        if not key:
            emit(f"{where}: ignoring line without a key")
            continue
        if section == SECTION_STENCIL:
            _read_stencil(config, key.lower(), value, where, emit)
        elif section == SECTION_LAYOUT:
            _read_layout(config, key.lower(), value, where, emit)
        elif section == SECTION_RULES:
            _read_rule(config, key.lower(), value, where, emit)
        elif section == SECTION_SIDES:
            _read_side(config, key, value, where, emit)
        elif section == SECTION_PADS:
            _read_pad(config, key, value, where, emit)
    return config


def _read_stencil(config: Config, key: str, value: str, where: str,
                  emit: Warn) -> None:
    if key == "size":
        try:
            size = parse_size(value)
        except ValueError:
            size = None
        if size is None or size not in STENCIL_SIZES:
            known = ", ".join(size_label(s) for s in STENCIL_SIZES)
            emit(f"{where}: unknown stencil size {value!r} (known: {known}), "
                 f"using {config.size_label}")
            return
        config.size = size
    elif key == "orientation":
        text = value.lower()
        if text not in ORIENTATIONS:
            emit(f"{where}: unknown orientation {value!r} "
                 f"(use {' | '.join(ORIENTATIONS)}), using {config.orientation}")
            return
        config.orientation = text
    else:
        emit(f"{where}: unknown [stencil] key {key!r}, ignored")


def _read_layout(config: Config, key: str, value: str, where: str,
                 emit: Warn) -> None:
    key = _LAYOUT_ALIASES.get(key, key)
    params = config.layout
    if key in _LAYOUT_FLOATS:
        attr, ok, requirement = _LAYOUT_FLOATS[key]
        try:
            number = float(value)
        except ValueError:
            emit(f"{where}: {key} = {value!r} is not a number, "
                 f"using {getattr(params, attr)}")
            return
        if number != number or number in (float("inf"), float("-inf")):
            emit(f"{where}: {key} = {value!r} is not a finite number, "
                 f"using {getattr(params, attr)}")
            return
        if ok is not None and not ok(number):
            emit(f"{where}: {key} {requirement}, using {getattr(params, attr)}")
            return
        setattr(params, attr, number)
    elif key in _LAYOUT_BOOLS:
        attr = _LAYOUT_BOOLS[key]
        flag = parse_bool(value)
        if flag is None:
            emit(f"{where}: {key} = {value!r} is not on/off, "
                 f"using {_bool_text(getattr(params, attr))}")
            return
        setattr(params, attr, flag)
    elif key == "sort":
        text = value.lower()
        if text not in SORT_ORDERS:
            emit(f"{where}: unknown sort order {value!r} "
                 f"(use {' | '.join(SORT_ORDERS)}), using {params.sort}")
            return
        params.sort = text
    else:
        emit(f"{where}: unknown [layout] key {key!r}, ignored")


def _read_rule(config: Config, key: str, value: str, where: str,
               emit: Warn) -> None:
    if key == "ignore_prefixes":
        config.ignore_prefixes = parse_prefixes(value)
    else:
        emit(f"{where}: unknown [{SECTION_RULES}] key {key!r}, ignored")


def _read_side(config: Config, key: str, value: str, where: str,
               emit: Warn) -> None:
    flag = parse_bool(value)
    if flag is None:
        emit(f"{where}: {key} = {value!r} is not on/off, using on")
        flag = True
    config.sides[key] = flag


def _read_pad(config: Config, key: str, value: str, where: str,
              emit: Warn) -> None:
    state = value.lower()
    if state not in STATES:
        emit(f"{where}: unknown state {value!r} for {key}, using {STATE_UNDEFINED}")
        state = STATE_UNDEFINED
    config.pads[key] = state


# --------------------------------------------------------------------------- #
# Config <-> projects
# --------------------------------------------------------------------------- #
def apply_config(config: Config, projects: list[Project]) -> None:
    """Push the stored switches and decisions into the discovered projects.

    A pad the file says nothing about (or says something unusable about) falls
    back to :func:`~pcbstencil.pads.default_state`: ``open`` when it already
    has paste, ``ignore`` when its reference matches one of the
    ``[rules] ignore_prefixes``, ``undefined`` otherwise.  A pad with paste can
    only be ``open`` or ``ignore`` (closed).
    """
    for project in projects:
        for side in project.sides():
            side.enabled = bool(config.sides.get(side.key, True))
            for pad in side.pads:
                state = config.pads.get(pad.key)
                if pad.is_candidate:
                    pad.state = (state if state in STATES
                                 else default_state(pad, config.ignore_prefixes))
                else:
                    pad.state = state if state in PASTED_STATES else STATE_OPEN


def collect_config(config: Config, projects: list[Project]) -> None:
    """Pull the current switches and decisions back into *config*.

    Every candidate is stored with its state; a pad with paste only shows up
    when the user closed it (``= ignore``) - an open one is the default and its
    entry is dropped.  Keys that are already in the config but do not match
    anything in *projects* are left untouched (they stay in the file as stale
    entries).
    """
    for project in projects:
        for side in project.sides():
            if side.is_relevant():
                config.sides[side.key] = bool(side.enabled)
            for pad in side.pads:
                if pad.is_candidate:
                    config.pads[pad.key] = pad.state
                elif pad.is_closed:
                    config.pads[pad.key] = STATE_IGNORE
                else:
                    config.pads.pop(pad.key, None)


# --------------------------------------------------------------------------- #
# Writing
# --------------------------------------------------------------------------- #
def _bool_text(flag: bool) -> str:
    return "on" if flag else "off"


def _num_text(value: float) -> str:
    """Compact but exact enough decimal text: ``30.0``, ``0.5``, ``-1.25``."""
    text = f"{float(value):.4f}".rstrip("0")
    return text + "0" if text.endswith(".") else text


def _entry(key: str, value: str, comment: str = "") -> str:
    line = f"{key} = {value}"
    if not comment:
        return line
    padding = max(_COMMENT_COLUMN - len(line), 3)
    return f"{line}{' ' * padding}# {comment}"


def _plural(count: int, word: str) -> str:
    return f"{count} {word}" if count == 1 else f"{count} {word}s"


def _side_comment(side: Side) -> str:
    project = side.project
    return (f"{project.width:.1f} x {project.height:.1f} mm, "
            f"{_plural(len(side.paste_objects), 'paste opening')}, "
            f"{_plural(len(side.candidates), 'pad')} to decide")


def _pad_comment(pad) -> str:
    return " ".join(part for part in (pad.function, pad.shape) if part)


def _ordered_projects(projects: list[Project]) -> list[Project]:
    return sorted(projects, key=lambda p: natural_key(p.name))


def format_config(config: Config, projects: list[Project]) -> str:
    """Render the whole ``.stencil`` file text."""
    stamp = datetime.now().isoformat(timespec="seconds")
    params = config.layout
    lines: list[str] = [
        f"# pcbstencil configuration and pad decisions (generated {stamp}).",
        "# Edit by hand or through the TUI (python3 stencicrity.py).",
        "",
        f"[{SECTION_STENCIL}]",
        _entry("size", config.size_label,
               " | ".join(size_label(s) for s in STENCIL_SIZES)),
        _entry("orientation", config.orientation,
               "landscape (long side horizontal) | portrait"),
        "",
        f"[{SECTION_LAYOUT}]",
        _entry("spacing", _num_text(params.gap),
               "mm between neighbouring boards; the dotted border runs in the middle"),
        _entry("holes", _bool_text(params.holes),
               "cut dowel pin holes near every cell corner"),
        _entry("hole_dia", _num_text(params.hole_dia), "mm"),
        _entry("hole_inset", _num_text(params.hole_inset),
               "mm from the dotted line to the hole edge (negative = onto the line)"),
        _entry("dot_dia", _num_text(params.dot_dia), "mm"),
        _entry("dot_pitch", _num_text(params.dot_pitch), "mm"),
        _entry("dot_line_gap", _num_text(params.dot_line_gap),
               "mm between the two dotted border lines (0 = single line)"),
        _entry("hole_grid", _num_text(params.hole_grid),
               "dowel hole centres snap to this grid, cells grow to fit (0 = off)"),
        _entry("outer_border", _bool_text(params.outer_border),
               "also dot the cell edges on the outer boundary of the block"),
        _entry("sort", params.sort, "height (tallest boards first) | name"),
        "",
        f"[{SECTION_RULES}]",
        _entry("ignore_prefixes", " ".join(config.ignore_prefixes),
               "references whose pads without paste default to ignore "
               "(prefix + digit)"),
        "",
        f"[{SECTION_SIDES}]",
        "# <project>/<side> = on | off",
    ]

    ordered = _ordered_projects(projects)
    known_sides: set[str] = set()
    for project in ordered:
        for side in project.sides():
            if not side.is_relevant():
                continue
            known_sides.add(side.key)
            flag = config.sides.get(side.key, side.enabled)
            lines.append(_entry(side.key, _bool_text(bool(flag)),
                                _side_comment(side)))
    stale_sides = [key for key in config.sides if key not in known_sides]
    if stale_sides:
        lines.append(_STALE_HEADER)
        for key in stale_sides:
            lines.append(_entry(key, _bool_text(bool(config.sides[key]))))

    lines.append("")
    lines.append(f"[{SECTION_PADS}]")
    lines.append(f"# <project>/<side>/<REF>.<pin>@<x>,<y> = "
                 f"{' | '.join(STATES)}")
    known_pads: set[str] = set()
    closed: list[str] = []
    for project in ordered:
        for side in project.sides():
            # Pads with paste are open by default: only the closed ones are
            # written, but none of them may end up in the stale block.
            for pad in side.pasted_pads:
                known_pads.add(pad.key)
                if config.pads.get(pad.key, pad.state) == STATE_IGNORE:
                    closed.append(_entry(pad.key, STATE_IGNORE, _pad_comment(pad)))
            candidates = sorted(side.candidates,
                                key=lambda p: (natural_key(p.ref),
                                               natural_key(p.pin), p.x, p.y))
            if not candidates:
                continue
            lines.append(f"# --- {project.name} / {side.name} ---")
            for pad in candidates:
                known_pads.add(pad.key)
                state = config.pads.get(pad.key, pad.state)
                lines.append(_entry(pad.key, state, _pad_comment(pad)))
    if closed:
        lines.append(_CLOSED_HEADER)
        lines.extend(closed)
    stale_pads = [key for key in config.pads if key not in known_pads]
    if stale_pads:
        lines.append(_STALE_HEADER)
        for key in stale_pads:
            lines.append(_entry(key, config.pads[key]))

    return "\n".join(lines) + "\n"


def save_config(path: str, config: Config, projects: list[Project]) -> str:
    """Collect the current state into *config* and write it to *path*."""
    collect_config(config, projects)
    _atomic_write(path, format_config(config, projects))
    return path


def _current_umask() -> int:
    mask = os.umask(0)
    os.umask(mask)
    return mask


def _atomic_write(path: str, text: str) -> None:
    """Write *text* to *path* through a temporary file in the same directory."""
    directory = os.path.dirname(os.path.abspath(path)) or "."
    os.makedirs(directory, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=directory, prefix=".stencil-", suffix=".tmp")
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as handle:
            handle.write(text)
        # mkstemp creates 0600 files; keep the existing mode or a normal umask mode.
        try:
            mode = os.stat(path).st_mode & 0o777
        except OSError:
            mode = 0o666 & ~_current_umask()
        os.chmod(tmp, mode)
        os.replace(tmp, path)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise
