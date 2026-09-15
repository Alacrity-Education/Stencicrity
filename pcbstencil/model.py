"""Shared data model: projects, sides, pads, configuration and layout.

Coordinate conventions
----------------------
* "board coordinates": the coordinates found in a project's gerber files (mm).
  Top and bottom layers of one project share the same board coordinates.
* "sheet coordinates": the output stencil sheet, origin (0, 0) at the bottom
  left corner of the *stencil* (its ordered size, e.g. 380 x 280 mm), X to the
  right, Y up, mm.
* A bottom side is placed on the stencil mirrored about the Y axis
  (x -> -x) so that the stencil matches the board when it is flipped over.

Layout model
------------
Every enabled side gets a *cell*: the board bounding box padded by at least
``gap/2`` on all four sides (so neighbouring boards are at least ``gap``
apart). When ``hole_grid`` is set, cells are enlarged (board kept centred) and
positioned so that every dowel hole centre lies on one common grid of that
pitch across the whole sheet. Cells are placed on the stencil sheet with a
MaxRects bin packer; every cell owns one sparse dotted line along each of its
edges, ``dot_line_gap/2`` inside the edge, so two touching cells show two
lines ``dot_line_gap`` apart and the scissors cut between them; the lines on
the outer boundary of the whole block are left out unless ``outer_border``.

Alignment features (``datum``): with ``slots`` the cells are sized to
multiples of ``slot_pitch`` (the raster of a modular pin jig) and every cell
carries an obround slot at every raster position along its bottom edge and
its left edge, ``slot_offset`` inside the edge. A slot lies completely inside
its own cell (it may clip the cell's dotted line, never a neighbour) and
keeps ``slot_web`` of foil to the board. The jig pins pass through the slots;
the pin centre (``pin_offset`` inside the edge) lies on the same raster, so
two slots on one of the two edges plus one on the other give an exact
three-contact location when the piece is pushed toward the bottom-left
corner; cells are sized so that this is always possible. An X marker
(``marker``) is cut at the raster point ``pin_offset`` inside the datum corner,
where the left pin column meets the bottom pin row, so the orientation of a
cut-out piece can be read at a glance. Cell corners sit on
the raster with phase ``-pin_offset`` so touching cells share the raster and
their dotted lines are exactly ``dot_line_gap`` apart. With
``holes`` every cell gets four round holes near its corners (legacy);
``hole_grid`` snaps those. The block of all cells is centred on the stencil
(keeping the raster alignment).
"""
from __future__ import annotations

from dataclasses import dataclass, field, replace
from typing import Optional

from shapely.geometry.base import BaseGeometry

from .gerber import Flash, GerberFile, GraphicObject

SIDE_TOP = "top"
SIDE_BOTTOM = "bottom"

STATE_UNDEFINED = "undefined"
STATE_OPEN = "open"
STATE_IGNORE = "ignore"
STATES = (STATE_UNDEFINED, STATE_OPEN, STATE_IGNORE)

# Orderable stencil sheet sizes (mm), long side first, smallest first.
STENCIL_SIZES: tuple[tuple[int, int], ...] = (
    (270, 270), (380, 280), (420, 320), (450, 350), (460, 460), (520, 420), (600, 600), (700, 600),
)
DEFAULT_STENCIL_SIZE: tuple[int, int] = (380, 280)
ORIENTATION_LANDSCAPE = "landscape"   # long side horizontal
ORIENTATION_PORTRAIT = "portrait"     # long side vertical
ORIENTATIONS = (ORIENTATION_LANDSCAPE, ORIENTATION_PORTRAIT)
SORT_HEIGHT = "height"
SORT_NAME = "name"
SORT_ORDERS = (SORT_HEIGHT, SORT_NAME)

# Reference prefixes whose pads without paste default to "ignore" (net ties, test points).
DEFAULT_IGNORE_PREFIXES: tuple[str, ...] = ("NT", "TP")

# Alignment datum cut into every cell for the pin jig.
DATUM_SLOTS = "slots"    # obround slots at every slot_pitch raster position along the bottom and left cell edges; pins pass through them, piece pushed to the datum corner
DATUM_HOLES = "holes"    # four round holes near the cell corners (legacy, over-constrained but simple)
DATUM_NONE = "none"      # no alignment features
DATUM_MODES = (DATUM_SLOTS, DATUM_HOLES, DATUM_NONE)


def size_label(size: tuple[int, int]) -> str:
    return f"{size[0]}x{size[1]}"


def parse_size(text: str) -> tuple[int, int]:
    """'380x280' -> (380, 280); raises ValueError for anything else."""
    parts = text.lower().replace("×", "x").split("x")
    if len(parts) != 2:
        raise ValueError(f"bad stencil size {text!r}")
    w, h = (int(round(float(p))) for p in parts)
    return (max(w, h), min(w, h))


def side_key(project_name: str, side_name: str) -> str:
    """Key of one side in the config file: '<project>/<side>'."""
    return f"{project_name}/{side_name}"


@dataclass
class Transform:
    """Board -> sheet mapping:  x' = (-x if mirror else x) + dx ;  y' = y + dy."""
    mirror: bool = False
    dx: float = 0.0
    dy: float = 0.0

    def apply(self, x: float, y: float) -> tuple[float, float]:
        return ((-x if self.mirror else x) + self.dx, y + self.dy)

    def affine(self) -> list[float]:
        """Matrix for shapely.affinity.affine_transform: [a, b, d, e, xoff, yoff]."""
        return [-1.0 if self.mirror else 1.0, 0.0, 0.0, 1.0, self.dx, self.dy]


@dataclass
class Pad:
    """A pad flash found on a copper layer."""
    key: str                 # "<project>/<side>/<ref>.<pin>@<x>,<y>" (board coords, 3 decimals)
    project: str
    side: str                # SIDE_TOP | SIDE_BOTTOM
    ref: str                 # component reference, e.g. "U2"
    pin: str                 # pin number/name as in %TO.P, e.g. "1"
    x: float                 # flash position, board coordinates (unmirrored)
    y: float
    function: str            # first token of AperFunction ("SMDPad", "BGAPad", ...), may be ""
    shape: str               # human readable aperture description, e.g. "R 0.28x0.52"
    flash: Flash             # the copper flash object
    geom: BaseGeometry       # pad copper shape, board coordinates
    has_paste: bool          # True when the paste layer already has an opening on this pad
    state: str = STATE_UNDEFINED   # candidates: undefined/open/ignore; pasted pads: open (default) or ignore
    paste_indices: list[int] = field(default_factory=list)   # indices into side.paste.objects covering this pad

    @property
    def is_candidate(self) -> bool:
        """A pad without a paste opening: the user decides whether to open it."""
        return not self.has_paste

    @property
    def is_closed(self) -> bool:
        """A pad WITH paste whose opening the user removed from the stencil."""
        return self.has_paste and self.state == STATE_IGNORE

    @property
    def label(self) -> str:
        return f"{self.ref}.{self.pin}"

    @property
    def side_key(self) -> str:
        return side_key(self.project, self.side)


@dataclass
class Side:
    """One side (top or bottom) of one project = one stencil cell when enabled."""
    project: "Project"
    name: str                          # SIDE_TOP | SIDE_BOTTOM
    copper: Optional[GerberFile]
    paste: Optional[GerberFile]
    mirror: bool = False               # place mirrored on the sheet (set for bottom sides)
    enabled: bool = True               # user switch from the config file / TUI sides page
    pads: list[Pad] = field(default_factory=list)   # every pad flash on copper, incl. those with paste

    @property
    def key(self) -> str:
        return side_key(self.project.name, self.name)

    @property
    def label(self) -> str:
        return f"{self.project.name} {self.name}"

    @property
    def paste_objects(self) -> list[GraphicObject]:
        return list(self.paste.objects) if self.paste is not None else []

    @property
    def copper_objects(self) -> list[GraphicObject]:
        return list(self.copper.objects) if self.copper is not None else []

    @property
    def candidates(self) -> list[Pad]:
        """Pads without a paste opening (shown by default in the TUI)."""
        return [p for p in self.pads if p.is_candidate]

    @property
    def pasted_pads(self) -> list[Pad]:
        """Pads that already have a paste opening (normally hidden in the TUI)."""
        return [p for p in self.pads if p.has_paste]

    @property
    def open_pads(self) -> list[Pad]:
        """Candidates the user opened: they get an opening cut from the copper pad."""
        return [p for p in self.candidates if p.state == STATE_OPEN]

    @property
    def closed_pads(self) -> list[Pad]:
        """Pasted pads the user closed: their paste openings are dropped."""
        return [p for p in self.pads if p.is_closed]

    def closed_paste_indices(self) -> set[int]:
        closed: set[int] = set()
        for pad in self.closed_pads:
            closed.update(pad.paste_indices)
        return closed

    @property
    def active_paste_objects(self) -> list[GraphicObject]:
        """Paste objects that end up on the stencil (closed pads removed)."""
        closed = self.closed_paste_indices()
        if not closed:
            return self.paste_objects
        return [o for i, o in enumerate(self.paste_objects) if i not in closed]

    @property
    def closed_paste_objects(self) -> list[GraphicObject]:
        closed = self.closed_paste_indices()
        return [o for i, o in enumerate(self.paste_objects) if i in closed]

    def has_openings(self) -> bool:
        return bool(self.active_paste_objects) or bool(self.open_pads)

    def is_relevant(self) -> bool:
        """Worth a stencil cell: has paste openings or pads to decide on."""
        return bool(self.paste_objects) or bool(self.candidates)


@dataclass
class Project:
    name: str                                   # from %TF.ProjectId, else the zip/dir name
    source: str                                 # zip path or directory
    outline: Optional[GerberFile]
    top: Optional[Side]
    bottom: Optional[Side]
    bbox: tuple[float, float, float, float]     # board bounding box, board coords (minx, miny, maxx, maxy)

    def sides(self) -> list[Side]:
        return [s for s in (self.top, self.bottom) if s is not None]

    @property
    def width(self) -> float:
        return self.bbox[2] - self.bbox[0]

    @property
    def height(self) -> float:
        return self.bbox[3] - self.bbox[1]


@dataclass
class LayoutParams:
    """Everything on the TUI "Layout" page; persisted in the [layout] section."""
    gap: float = 30.0          # spacing between neighbouring boards (mm); dotted border at gap/2
    datum: str = DATUM_SLOTS   # alignment features per cell: DATUM_SLOTS | DATUM_HOLES | DATUM_NONE
    # -- holes datum (legacy) --
    hole_dia: float = 5.0      # dowel pin hole diameter (mm)
    hole_inset: float = 2.0    # dotted line (cell edge) to hole edge (mm); may be negative
    # -- slots datum --
    slot_width: float = 4.5    # slot size across the cell edge (mm)
    slot_length: float = 8.0   # slot size along the cell edge (mm)
    slot_offset: float = 0.5   # cell edge to the slot's outer wall (mm); the slot may clip the dotted line but never the neighbour
    slot_pitch: float = 20.0   # raster of the modular jig (mm): slot centres along the bottom and left edges and pin centres across them lie on it; cells grow to multiples of it
    slot_web: float = 3.0      # minimum foil between a slot's inner wall and the board (mm); cells grow to keep it
    pin_dia: float = 3.0       # jig pin diameter for the slots datum (mm); the holes datum uses hole_dia pins
    marker: bool = True        # slots datum: cut an X at the raster point pin_offset inside the datum corner (where the left pin column meets the bottom pin row) so the piece's orientation can be read
    marker_size: float = 4.0   # X marker: length of each of its two strokes (mm); stroke width = dot_dia
    # -- dotted border --
    dot_dia: float = 0.5       # divider dot diameter (mm)
    dot_pitch: float = 3.0     # centre-to-centre distance of divider dots (mm)
    dot_line_gap: float = 2.5  # every cell's own dotted line runs dot_line_gap/2 inside its edge; touching cells thus show two lines this far apart and the cut goes between them (0 = on the edge)
    # -- jig --
    hole_grid: float = 8.0     # holes datum: hole centres snap to a grid of this pitch (mm); 0 = off (the slots datum uses slot_pitch)
    outer_border: bool = False # also dot the cell edges on the outer boundary of the block
    sort: str = SORT_HEIGHT    # cell order: SORT_HEIGHT (tallest boards first) or SORT_NAME

    @property
    def pad(self) -> float:
        """Padding between a board and its cell edge."""
        return self.gap / 2.0

    @property
    def holes(self) -> bool:
        return self.datum == DATUM_HOLES

    @property
    def slots(self) -> bool:
        return self.datum == DATUM_SLOTS

    @property
    def hole_offset(self) -> float:
        """Cell edge to hole centre (holes datum)."""
        return self.hole_inset + self.hole_dia / 2.0

    @property
    def slot_inner(self) -> float:
        """Cell edge to the slot's inner wall, the wall the pin touches (slots datum)."""
        return self.slot_offset + self.slot_width

    @property
    def pin_offset(self) -> float:
        """Cell edge to the pin centre for the active datum (used for grid snapping)."""
        if self.datum == DATUM_SLOTS:
            return self.slot_inner - self.pin_dia / 2.0
        return self.hole_offset

    @property
    def min_pad_for_slots(self) -> float:
        """Smallest cell padding that hosts a slot and the web to the board."""
        return self.slot_inner + self.slot_web


@dataclass
class Config:
    """The whole .stencicrity file: stencil size, layout, enabled sides, pad states."""
    size: tuple[int, int] = DEFAULT_STENCIL_SIZE     # long side first, one of STENCIL_SIZES
    orientation: str = ORIENTATION_LANDSCAPE
    layout: LayoutParams = field(default_factory=LayoutParams)
    ignore_prefixes: tuple[str, ...] = DEFAULT_IGNORE_PREFIXES   # [rules] ignore_prefixes
    sides: dict[str, bool] = field(default_factory=dict)   # side_key -> enabled
    pads: dict[str, str] = field(default_factory=dict)     # pad key -> state (candidates: any; pasted pads: open|ignore)

    @property
    def size_label(self) -> str:
        return size_label(self.size)

    def sheet_size(self) -> tuple[float, float]:
        """(width, height) of the stencil as laid out, honouring the orientation."""
        long_side, short_side = max(self.size), min(self.size)
        if self.orientation == ORIENTATION_PORTRAIT:
            return float(short_side), float(long_side)
        return float(long_side), float(short_side)

    def copy(self) -> "Config":
        return Config(self.size, self.orientation, replace(self.layout),
                      tuple(self.ignore_prefixes), dict(self.sides), dict(self.pads))


@dataclass
class Area:
    """Placement of one side: its cell on the sheet."""
    side: Side
    x: float                   # cell bottom-left corner, sheet coords
    y: float
    w: float                   # cell size: board + gap in both directions
    h: float
    board_rect: tuple[float, float, float, float]   # where the board bbox lands (minx, miny, maxx, maxy)
    transform: Transform       # board -> sheet for this side's objects
    row: int = 0               # informational (placement order); cells are packed with MaxRects, not in rows
    overflow: bool = False     # did not fit on the sheet; parked to the right of it
    holes: list[tuple[float, float]] = field(default_factory=list)   # round hole centres, sheet coords (holes datum)
    slots: list[tuple[float, float, float, float]] = field(default_factory=list)  # obround slots (cx, cy, w, h), sheet coords (slots datum); w along x, h along y
    pins: list[tuple[float, float]] = field(default_factory=list)    # jig pin centres, sheet coords (one per slot for the slots datum; == holes for the holes datum)
    datum_corner: tuple[float, float] = (0.0, 0.0)   # sheet coords of the corner the piece is pushed toward (slots datum: bottom-left of the cell)
    marker: Optional[tuple[float, float]] = None      # centre of the X orientation marker, sheet coords (slots datum with marker on): (x0 + pin_offset, y0 + pin_offset)

    @property
    def rect(self) -> tuple[float, float, float, float]:
        return (self.x, self.y, self.x + self.w, self.y + self.h)


@dataclass
class Layout:
    params: LayoutParams
    areas: list[Area]
    width: float                                          # stencil sheet size (mm)
    height: float
    block: tuple[float, float, float, float]              # bbox of all cells on the sheet
    fits: bool                                            # block fits inside the sheet
    dots: list[tuple[float, float]] = field(default_factory=list)          # divider dot centres
    dividers: list[tuple[float, float, float, float]] = field(default_factory=list)  # (x0,y0,x1,y1)
    heuristic: str = ""                                   # MaxRects heuristic that won
    overflow: int = 0                                     # number of cells that did not fit

    @property
    def block_width(self) -> float:
        return self.block[2] - self.block[0]

    @property
    def block_height(self) -> float:
        return self.block[3] - self.block[1]
