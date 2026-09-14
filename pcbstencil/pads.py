"""Pad detection and the user's open/ignore decisions.

A pad is a flash on a copper layer that belongs to a component footprint.
Pads that the paste layer already covers are copied to the stencil as they are
(state ``open``); the user may still *close* one, which drops its paste
openings from the stencil (state ``ignore``).  Pads without paste
("candidates") are offered to the user, who marks them ``open`` (cut an
opening) or ``ignore``.  Candidates whose reference starts with one of the
configured ignore prefixes plus a digit (``TP3``, ``NT12``) default to
``ignore``.  The decisions themselves are stored in the project's ``.stencicrity``
file (see :mod:`pcbstencil.config`).
"""
from __future__ import annotations

import re
from functools import lru_cache
from typing import Iterable, Optional, Sequence

from shapely import STRtree
from shapely.geometry.base import BaseGeometry

from .gerber import Flash, object_geometry
from .model import (
    DEFAULT_IGNORE_PREFIXES,
    SIDE_TOP,
    STATE_IGNORE,
    STATE_OPEN,
    STATE_UNDEFINED,
    STATES,
    Pad,
    Project,
    Side,
)

__all__ = ["SMD_FUNCTIONS", "THT_FUNCTIONS", "NON_PAD_FUNCTIONS", "pad_key",
           "detect_pads", "matches_prefix", "default_state", "natural_key",
           "sorted_candidates", "sorted_pads", "state_counts", "closed_count"]

#: AperFunction values that always denote a surface mount pad.
SMD_FUNCTIONS = frozenset({"SMDPad", "BGAPad", "HeatsinkPad", "FiducialPad",
                           "TestPad", "ConnectorPad"})
#: Through hole pads: only pads when the user asks for them.
THT_FUNCTIONS = frozenset({"ComponentPad", "CastellatedPad"})
#: AperFunction values that are never pads.
NON_PAD_FUNCTIONS = frozenset({"ViaPad", "Conductor", "NonConductor",
                               "EtchedComponent", "Profile", "WasherPad",
                               "AntiPad", "Other", "Drawing"})


_CHUNK_RE = re.compile(r"(\d+)")


# --------------------------------------------------------------------------- #
# Default state of a freshly detected pad
# --------------------------------------------------------------------------- #
@lru_cache(maxsize=None)
def _prefix_re(prefixes: tuple[str, ...]) -> Optional[re.Pattern]:
    """``("NT", "TP")`` -> a regex matching ``NT12`` / ``tp3`` but not ``TPS1``."""
    parts = [re.escape(text.strip()) for text in prefixes if text and text.strip()]
    if not parts:
        return None
    return re.compile(r"^(?:" + "|".join(parts) + r")\d", re.IGNORECASE)


def matches_prefix(ref: str, prefixes: Sequence[str]) -> bool:
    """True when *ref* is one of *prefixes* followed by a digit (case insensitive).

    ``TP1``, ``NT12`` and ``tp3`` match ``("NT", "TP")``; ``TPS1`` (letter after
    the prefix), ``T1`` and ``R1`` do not.  An empty prefix list matches nothing.
    """
    pattern = _prefix_re(tuple(prefixes or ()))
    return bool(pattern is not None and pattern.match(str(ref)))


def default_state(pad: Pad,
                  ignore_prefixes: Sequence[str] = DEFAULT_IGNORE_PREFIXES) -> str:
    """State of *pad* when the configuration file says nothing about it.

    A pad that already has paste is ``open`` (its openings are on the stencil);
    a candidate whose reference matches an ignore prefix is ``ignore``;
    every other candidate is ``undefined`` and waits for a decision.
    """
    if pad.has_paste:
        return STATE_OPEN
    if matches_prefix(pad.ref, ignore_prefixes):
        return STATE_IGNORE
    return STATE_UNDEFINED


# --------------------------------------------------------------------------- #
# Detection
# --------------------------------------------------------------------------- #
def pad_key(project: str, side: str, ref: str, pin: str, x: float, y: float) -> str:
    """Stable identifier of a pad, used in the .stencicrity file (board coords)."""
    return f"{project}/{side}/{ref}.{pin}@{x:.3f},{y:.3f}"


def _is_pad(flash: Flash, include_tht: bool) -> bool:
    """Decide whether a copper flash is a component pad."""
    function = flash.aperture.function
    if function in NON_PAD_FUNCTIONS:
        return False
    if function in SMD_FUNCTIONS:
        return True
    if function in THT_FUNCTIONS:
        return include_tht
    # Unknown or missing AperFunction: trust the object attribute instead.
    return bool(flash.attrs.get("P"))


def _ref_pin(flash: Flash, fallback_index: int) -> tuple[str, str]:
    """Component reference and pin from the %TO.P object attribute."""
    raw = flash.attrs.get("P", "")
    if raw:
        parts = [p.strip() for p in raw.split(",")]
        ref = parts[0] or "?"
        pin = parts[1] if len(parts) > 1 and parts[1] else str(fallback_index)
        return ref, pin
    return "?", str(fallback_index)


def _paste_geometries(side: Side) -> tuple[list[BaseGeometry], list[int]]:
    """Usable paste geometries and, for each, its index in ``side.paste_objects``."""
    geoms: list[BaseGeometry] = []
    indices: list[int] = []
    for index, obj in enumerate(side.paste_objects):
        geom = object_geometry(obj)
        if geom is not None and not geom.is_empty:
            geoms.append(geom)
            indices.append(index)
    return geoms, indices


def detect_pads(side: Side, *, include_tht: bool = False,
                min_overlap: float = 0.10,
                ignore_prefixes: Sequence[str] = DEFAULT_IGNORE_PREFIXES) -> list[Pad]:
    """Find every pad of *side* and mark which ones the paste layer covers.

    A pad counts as pasted when the paste openings cover at least
    ``min_overlap`` of its copper area, or when an opening contains its
    centroid.  Every pasted pad also records the indices of the paste objects
    covering it in ``pad.paste_indices`` (so closing the pad can drop exactly
    those openings), and every pad gets its :func:`default_state`.  The result
    is stored in ``side.pads`` and returned, sorted by reference and pin.
    """
    paste_geoms, paste_indices = _paste_geometries(side)
    tree = STRtree(paste_geoms) if paste_geoms else None

    pads: list[Pad] = []
    unnamed = 0
    for obj in side.copper_objects:
        if not isinstance(obj, Flash) or not obj.dark:
            continue
        if not _is_pad(obj, include_tht):
            continue
        if not obj.attrs.get("P"):
            unnamed += 1
        ref, pin = _ref_pin(obj, unnamed)
        geom = object_geometry(obj)
        has_paste, covering = _paste_hits(geom, paste_geoms, paste_indices,
                                          tree, min_overlap)
        pad = Pad(
            key=pad_key(side.project.name, side.name, ref, pin, obj.x, obj.y),
            project=side.project.name,
            side=side.name,
            ref=ref,
            pin=pin,
            x=obj.x,
            y=obj.y,
            function=obj.aperture.function,
            shape=obj.aperture.describe(),
            flash=obj,
            geom=geom,
            has_paste=has_paste,
            state=STATE_UNDEFINED,
            paste_indices=covering if has_paste else [],
        )
        pad.state = default_state(pad, ignore_prefixes)
        pads.append(pad)

    pads.sort(key=lambda p: (natural_key(p.ref), natural_key(p.pin), p.x, p.y))
    side.pads = pads
    return pads


def _paste_hits(geom: BaseGeometry, paste_geoms: list[BaseGeometry],
                paste_indices: list[int], tree: Optional[STRtree],
                min_overlap: float) -> tuple[bool, list[int]]:
    """``(the paste layer opens this pad, indices of the openings on it)``.

    An opening counts as being *on* the pad when it contains the pad centroid
    or overlaps it at all; the pad counts as pasted when an opening holds its
    centroid or the openings together cover ``min_overlap`` of its area.
    """
    if tree is None or geom is None or geom.is_empty:
        return False, []
    area = geom.area
    centroid = geom.centroid
    covered = 0.0
    centred = False
    hits: list[int] = []
    for index in tree.query(geom):
        index = int(index)
        paste = paste_geoms[index]
        overlap = geom.intersection(paste).area if area > 0 else 0.0
        inside = paste.contains(centroid)
        if inside:
            centred = True
        if inside or overlap > 0:
            hits.append(paste_indices[index])
        covered += overlap
    has_paste = centred or (area > 0 and covered >= min_overlap * area)
    return has_paste, sorted(hits)


# --------------------------------------------------------------------------- #
# Ordering and counting
# --------------------------------------------------------------------------- #
def natural_key(text: str) -> tuple:
    """Sort key where embedded numbers compare numerically ('R2' < 'R10')."""
    key = []
    for chunk in _CHUNK_RE.split(str(text)):
        if not chunk:
            continue
        if chunk.isdigit():
            key.append((0, int(chunk), ""))
        else:
            key.append((1, 0, chunk.lower()))
    return tuple(key)


def _side_order(name: str) -> int:
    return 0 if name == SIDE_TOP else 1


def _pad_order(pad: Pad) -> tuple:
    return (natural_key(pad.project), _side_order(pad.side),
            natural_key(pad.ref), natural_key(pad.pin), pad.x, pad.y)


def sorted_candidates(projects: list[Project],
                      sides: list[Side] | None = None) -> list[Pad]:
    """Every pad without paste, ordered project / top-first / ref / pin."""
    return sorted_pads(projects, sides)


def sorted_pads(projects: list[Project], sides: list[Side] | None = None, *,
                all_pads: bool = False) -> list[Pad]:
    """Pads ordered project / top-first / ref / pin.

    With ``all_pads`` every pad is returned (candidates *and* pads that already
    have paste), otherwise only the candidates.
    """
    if sides is None:
        sides = [side for project in projects for side in project.sides()]
    pads = [pad for side in sides for pad in side.pads
            if all_pads or pad.is_candidate]
    pads.sort(key=_pad_order)
    return pads


def state_counts(pads: Iterable[Pad]) -> dict[str, int]:
    """Count pads per state; every state of :data:`model.STATES` is present."""
    counts = {state: 0 for state in STATES}
    for pad in pads:
        if pad.state in counts:
            counts[pad.state] += 1
        else:
            counts[STATE_UNDEFINED] += 1
    return counts


def closed_count(pads: Iterable[Pad]) -> int:
    """How many of *pads* are pasted pads the user closed (state ignore)."""
    return sum(1 for pad in pads if pad.is_closed)
