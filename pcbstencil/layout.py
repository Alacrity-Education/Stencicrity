"""MaxRects packing of PCB sides onto one stencil sheet.

Every *enabled* side (top/bottom of every project) becomes one rectangular
*cell* on the sheet: the board bounding box padded by at least ``gap/2`` on all
four sides, so two neighbouring boards are at least ``gap`` apart and the shared
edge of two touching cells sits in the middle of that gap.  Without a hole grid
a cell is exactly ``board + gap`` large - cells are never stretched and there
are no rows.

``hole_grid`` puts *every* dowel hole of the whole sheet on one common grid of
that pitch, so the stencil can be pinned onto a fixture plate whose dowels sit
on that raster.  A hole is ``hole_offset`` inside the cell corner, so two things
have to be true: the hole-to-hole distance inside a cell, ``cell - 2*offset``,
must be a multiple of the pitch (the cell is grown until it is, the board stays
centred, and the padding only ever gets larger than ``gap/2``), and the cell
corner must sit at ``k * pitch - offset`` (the packer only places cells there,
and the block is centred by a whole number of pitches so the grid is measured
from the sheet origin).  Cells therefore no longer have to touch - the leftover
slivers are simply free space.  ``hole_grid = 0`` switches all of that off.

The cells are packed into the sheet with a MaxRects bin packer (Jylanki's
MaxRectsBinPack, no rotation): the free space is kept as a list of *maximal*
free rectangles, each cell is placed in the bottom left corner of the free
rectangle that scores best, every free rectangle overlapping the placement is
split into its (up to four) remainders and rectangles contained in another one
are dropped.  Three placement heuristics are run - bottom left, best short side
fit and best area fit - and the best result is kept.  Cells that fit nowhere
are *overflow*: they are lined up to the right of the sheet so the user can see
them, and ``Layout.fits`` is ``False``.  The block of placed cells is centred
on the stencil.

Every cell edge is a *divider*: it is marked on the stencil by two parallel
rows of small round openings (the *dots*), ``dot_line_gap`` apart and centred
on the edge, so the user can see and cut along the border between two boards
(``dot_line_gap = 0`` gives a single row on the edge itself).  Every cell is
outlined completely - all four of its edges, whether they are shared with a
neighbouring cell or face empty space.  Collinear pieces that overlap or touch
are merged, so a shared edge gives exactly one pair of dotted lines and a long
straight border is one pair.  The only edges that may be left out are the ones
on the outer boundary of the block of cells (nothing has to be cut apart
there): ``outer_border`` dots those too.  :attr:`Layout.dividers` holds the
dotted lines themselves (both of every pair), not the cell edges.  Each cell
also gets four dowel-pin holes near its corners, ``hole_inset`` away from the
cell edges.

Sheet coordinates have their origin at the bottom left of the stencil, X to the
right and Y up, in millimetres (see :mod:`pcbstencil.model`).
"""
from __future__ import annotations

import math

from .model import (
    ORIENTATION_PORTRAIT,
    SIDE_TOP,
    SORT_NAME,
    STATE_IGNORE,
    STATE_OPEN,
    STATE_UNDEFINED,
    Area,
    Config,
    Layout,
    LayoutParams,
    Side,
    Transform,
)
from .pads import natural_key

__all__ = ["pack", "layout_report", "ordered_sides"]

_EPS = 1e-9
_FIT_EPS = 1e-6
#: Two coordinates closer than this are the same coordinate (divider merging).
_MERGE_EPS = 1e-6
#: Touching cell edges are snapped together within this distance (see _aligned).
_ALIGN_EPS = 1e-9
#: Two holes closer than this are the same hole (neighbouring cells share them
#: when ``hole_inset == -hole_dia / 2``, i.e. the holes sit on the dotted line).
_HOLE_EPS = 1e-6
#: Slack of the hole grid arithmetic, in grid steps (see _snap_up / _grid_size).
_GRID_EPS = 1e-9

#: Placement heuristics, in preference order (the first one wins a tie).
HEURISTIC_BL = "bottom-left"
HEURISTIC_BSSF = "best short side"
HEURISTIC_BAF = "best area"
HEURISTICS = (HEURISTIC_BL, HEURISTIC_BSSF, HEURISTIC_BAF)

_VERTICAL = 0
_HORIZONTAL = 1


# --------------------------------------------------------------------------- #
# Ordering
# --------------------------------------------------------------------------- #
def _side_rank(side: Side) -> int:
    return 0 if side.name == SIDE_TOP else 1


def ordered_sides(sides: list[Side], params: LayoutParams) -> list[Side]:
    """The enabled sides in the order they are handed to the packer."""
    enabled = [s for s in sides if s.enabled]
    if params.sort == SORT_NAME:
        return sorted(enabled, key=lambda s: (natural_key(s.project.name), _side_rank(s)))
    return sorted(enabled, key=lambda s: (-round(s.project.height, 6),
                                          -round(s.project.width, 6),
                                          natural_key(s.project.name), _side_rank(s)))


# --------------------------------------------------------------------------- #
# The dowel hole grid
# --------------------------------------------------------------------------- #
def _grid_size(size: float, ho: float, grid: float) -> float:
    """``size`` grown until ``size - 2 * ho`` is a whole number of ``grid``."""
    steps = max(0, math.ceil((size - 2.0 * ho) / grid - _GRID_EPS))
    return steps * grid + 2.0 * ho


def _cell_size(width: float, height: float, ho: float, grid: float
               ) -> tuple[float, float]:
    """Cell size of a ``board + gap`` sized cell, grown to fit the hole grid.

    The two holes on one cell edge are ``cell - 2 * ho`` apart, so that distance
    has to be a multiple of the grid pitch; the cell is only ever *grown* (the
    board stays centred, the padding grows past ``gap/2``).
    """
    if grid <= 0.0:
        return (width, height)
    return (max(width, _grid_size(width, ho, grid)),
            max(height, _grid_size(height, ho, grid)))


def _snap_up(value: float, ho: float, grid: float) -> float:
    """The smallest cell corner ``>= value`` whose holes land on the grid.

    A cell corner at ``x`` carries holes at ``x + ho`` and ``x + w - ho``, so
    ``x + ho`` has to be a multiple of ``grid``.  A ``value`` that is already
    admissible (up to floating point noise) is returned as the exact grid
    position; ``grid <= 0`` returns it unchanged.
    """
    if grid <= 0.0:
        return value
    steps = (value + ho) / grid
    nearest = round(steps)
    steps = nearest if abs(steps - nearest) * grid <= _ALIGN_EPS else math.ceil(steps)
    return steps * grid - ho


def _snap_down(value: float, grid: float) -> float:
    """``value`` (>= 0) floored onto a multiple of ``grid``; keeps holes aligned."""
    if grid <= 0.0:
        return value
    steps = value / grid
    nearest = round(steps)
    steps = nearest if abs(steps - nearest) * grid <= _ALIGN_EPS else math.floor(steps)
    return max(0.0, steps * grid)


# --------------------------------------------------------------------------- #
# MaxRects bin packing
# --------------------------------------------------------------------------- #
def _score_bottom_left(fx: float, fy: float, fw: float, fh: float,
                       cw: float, ch: float) -> tuple[float, ...]:
    """Bottom-Left: lowest top edge, then leftmost."""
    return (fy + ch, fx)


def _score_short_side(fx: float, fy: float, fw: float, fh: float,
                      cw: float, ch: float) -> tuple[float, ...]:
    """Best Short Side Fit: smallest leftover on the tighter axis."""
    dw, dh = fw - cw, fh - ch
    return (min(dw, dh), max(dw, dh))


def _score_area(fx: float, fy: float, fw: float, fh: float,
                cw: float, ch: float) -> tuple[float, ...]:
    """Best Area Fit: smallest leftover area, then the tighter axis."""
    dw, dh = fw - cw, fh - ch
    return (fw * fh - cw * ch, min(dw, dh))


_SCORE = {
    HEURISTIC_BL: _score_bottom_left,
    HEURISTIC_BSSF: _score_short_side,
    HEURISTIC_BAF: _score_area,
}


def _split(free: tuple[float, float, float, float],
           placed: tuple[float, float, float, float]
           ) -> list[tuple[float, float, float, float]]:
    """Split one free rectangle by a placed cell into its maximal remainders."""
    fx, fy, fw, fh = free
    px, py, pw, ph = placed
    if (px >= fx + fw - _EPS or px + pw <= fx + _EPS
            or py >= fy + fh - _EPS or py + ph <= fy + _EPS):
        return [free]                                   # no overlap
    out: list[tuple[float, float, float, float]] = []
    if px > fx + _EPS:                                  # left
        out.append((fx, fy, px - fx, fh))
    if px + pw < fx + fw - _EPS:                        # right
        out.append((px + pw, fy, fx + fw - px - pw, fh))
    if py > fy + _EPS:                                  # bottom
        out.append((fx, fy, fw, py - fy))
    if py + ph < fy + fh - _EPS:                        # top
        out.append((fx, py + ph, fw, fy + fh - py - ph))
    return out


def _contains(outer: tuple[float, float, float, float],
              inner: tuple[float, float, float, float]) -> bool:
    ox, oy, ow, oh = outer
    ix, iy, iw, ih = inner
    return (ox <= ix + _FIT_EPS and oy <= iy + _FIT_EPS
            and ix + iw <= ox + ow + _FIT_EPS and iy + ih <= oy + oh + _FIT_EPS)


def _prune(rects: list[tuple[float, float, float, float]]
           ) -> list[tuple[float, float, float, float]]:
    """Drop every free rectangle that another one already covers."""
    dead = [False] * len(rects)
    for i, inner in enumerate(rects):
        if dead[i]:
            continue
        for j, outer in enumerate(rects):
            if i == j or dead[j]:
                continue
            if _contains(outer, inner):
                dead[i] = True
                break
    return [r for i, r in enumerate(rects) if not dead[i]]


def _maxrects(cells: list[tuple[float, float]], sheet_w: float, sheet_h: float,
              heuristic: str, ho: float = 0.0, grid: float = 0.0
              ) -> list[tuple[float, float] | None]:
    """Place ``cells`` (in order) into ``(0, 0, sheet_w, sheet_h)``.

    Returns one bottom-left corner per cell, ``None`` for a cell that did not
    fit into any free rectangle.  With a hole ``grid`` the bottom left corner of
    a free rectangle is first snapped up to the next position whose holes land
    on the grid; the cell has to fit in what is left of the free rectangle and
    is scored there.  The sliver below and left of the cell stays free space.
    """
    score = _SCORE[heuristic]
    free: list[tuple[float, float, float, float]] = []
    if sheet_w > 0.0 and sheet_h > 0.0:
        free.append((0.0, 0.0, sheet_w, sheet_h))
    out: list[tuple[float, float] | None] = []
    for cw, ch in cells:
        best: tuple[tuple[float, ...], float, float] | None = None
        for fx, fy, fw, fh in free:
            x, y = _snap_up(fx, ho, grid), _snap_up(fy, ho, grid)
            aw, ah = fw - (x - fx), fh - (y - fy)
            if aw + _FIT_EPS < cw or ah + _FIT_EPS < ch:
                continue
            # Ties are broken by the placement itself (x, then y).
            key = (score(x, y, aw, ah, cw, ch), x, y)
            if best is None or key < best:
                best = key
        if best is None:
            out.append(None)
            continue
        _, x, y = best
        out.append((x, y))
        placed = (x, y, cw, ch)
        split: list[tuple[float, float, float, float]] = []
        for rect in free:
            split.extend(_split(rect, placed))
        free = _prune(split)
    return out


def _best_packing(cells: list[tuple[float, float]], sheet_w: float, sheet_h: float,
                  ho: float = 0.0, grid: float = 0.0
                  ) -> tuple[list[tuple[float, float] | None], str]:
    """Run every heuristic and keep the best result.

    Best = fewest unplaced cells, then smallest bounding box of the placed
    cells, then the earliest heuristic (bottom-left first).
    """
    best: tuple[int, float, int] | None = None
    best_out: list[tuple[float, float] | None] = []
    best_name = HEURISTICS[0]
    for rank, name in enumerate(HEURISTICS):
        out = _maxrects(cells, sheet_w, sheet_h, name, ho, grid)
        unplaced = sum(1 for p in out if p is None)
        bw = max((p[0] + cells[i][0] for i, p in enumerate(out) if p is not None),
                 default=0.0)
        bh = max((p[1] + cells[i][1] for i, p in enumerate(out) if p is not None),
                 default=0.0)
        key = (unplaced, bw * bh, rank)
        if best is None or key < best:
            best, best_out, best_name = key, out, name
    return best_out, best_name


# --------------------------------------------------------------------------- #
# Packing
# --------------------------------------------------------------------------- #
def pack(sides: list[Side], config: Config) -> Layout:
    """Place every enabled side on the stencil and return the :class:`Layout`.

    The packing is deterministic: the sides are ordered by ``config.layout.sort``
    and their cells (``board + gap``, grown to fit the hole grid) are packed with
    MaxRects; the block of placed cells is then centred on the stencil (by a
    whole number of grid pitches, so the dowel holes stay on the grid).  Cells
    that fit nowhere are lined up to the right of the sheet and make
    ``Layout.fits`` ``False``.
    """
    params = config.layout
    sheet_w, sheet_h = config.sheet_size()
    gap = params.gap
    ho, grid = params.hole_offset, max(0.0, params.hole_grid)

    order = ordered_sides(sides, params)
    cells: list[tuple[float, float]] = []
    for side in order:
        minx, miny, maxx, maxy = side.project.bbox
        # gap/2 per side, then grown until the holes fit the grid.
        cells.append(_cell_size((maxx - minx) + gap, (maxy - miny) + gap, ho, grid))

    # 1. Pack, then line the cells that fit nowhere up right of the sheet.
    spots, heuristic = _best_packing(cells, sheet_w, sheet_h, ho, grid)
    overflow = [i for i, spot in enumerate(spots) if spot is None]
    overflow_set = set(overflow)
    x, y = _snap_up(sheet_w, ho, grid), _snap_up(0.0, ho, grid)
    for i in overflow:
        spots[i] = (x, y)
        x = _snap_up(x + cells[i][0], ho, grid)

    # 2. Centre the block of *placed* cells on the stencil (overflow moves too).
    #    The offset is a whole number of grid pitches, so every hole stays on
    #    the grid measured from the sheet origin.
    placed = [i for i in range(len(cells)) if i not in overflow_set]
    if placed:
        block_w = max(spots[i][0] + cells[i][0] for i in placed)
        block_h = max(spots[i][1] + cells[i][1] for i in placed)
        ox = _snap_down(max(0.0, (sheet_w - block_w) / 2.0), grid)
        oy = _snap_down(max(0.0, (sheet_h - block_h) / 2.0), grid)
    else:
        ox = oy = 0.0

    # 3. Translate every cell onto the sheet, keeping touching edges identical.
    xs = _aligned([s[0] for s in spots], [c[0] for c in cells], ox)
    ys = _aligned([s[1] for s in spots], [c[1] for c in cells], oy)

    # 4. The cells themselves.
    areas: list[Area] = []
    holes = _Dedupe(_HOLE_EPS)
    for index, (side, (cw, ch)) in enumerate(zip(order, cells)):
        cx, cy = xs[index], ys[index]
        minx, miny, maxx, maxy = side.project.bbox
        board_w, board_h = maxx - minx, maxy - miny
        # The board is centred inside its cell: gap/2 on every side, more when
        # the cell was grown for the hole grid.
        bx = cx + (cw - board_w) / 2.0
        by = cy + (ch - board_h) / 2.0
        # Board -> sheet.  A mirrored board occupies [-maxx, -minx] in x.
        dx = bx - (-maxx if side.mirror else minx)
        dy = by - miny
        area = Area(
            side=side,
            x=cx, y=cy, w=cw, h=ch,
            board_rect=(bx, by, bx + board_w, by + board_h),
            transform=Transform(mirror=side.mirror, dx=dx, dy=dy),
            row=index,                       # informational: the packing order
            holes=_holes(cx, cy, cw, ch, params, holes),
        )
        # Cells that fit nowhere are drawn outside the stencil (see the report).
        area.overflow = index in overflow_set
        areas.append(area)

    block = _block_of(areas)          # the dividers use the same boundary
    fits = (not overflow
            and block[0] >= -_FIT_EPS and block[1] >= -_FIT_EPS
            and block[2] <= sheet_w + _FIT_EPS and block[3] <= sheet_h + _FIT_EPS)

    dividers = _dividers(areas, params.outer_border, params.dot_line_gap)
    dots = _dots(dividers, params)
    layout = Layout(params=params, areas=areas, width=sheet_w, height=sheet_h,
                    block=block, fits=fits, dots=dots, dividers=dividers)
    # Extra, informational attributes used by layout_report() (Layout has no
    # field for them; every consumer reads them with getattr).
    layout.heuristic = heuristic
    layout.overflow = len(overflow)
    return layout


def _aligned(starts: list[float], sizes: list[float], offset: float) -> list[float]:
    """Translate the packed coordinates by ``offset`` along one axis.

    Floating point addition is not associative, so ``(start + offset) + size``
    can miss ``(start + size) + offset`` by one ULP and two cells that touch in
    the bin would overlap by ~1e-13 mm on the sheet.  Every translated
    coordinate is therefore snapped onto the far edge of an already translated
    cell when the two are within ``_ALIGN_EPS``.
    """
    out = [0.0] * len(starts)
    edges: list[float] = []
    for i in sorted(range(len(starts)), key=lambda k: (starts[k], sizes[k], k)):
        value = starts[i] + offset
        for edge in edges:
            if abs(edge - value) <= _ALIGN_EPS:
                value = edge
                break
        out[i] = value
        far = value + sizes[i]
        if all(abs(edge - far) > _ALIGN_EPS for edge in edges):
            edges.append(far)
    return out


def _holes(x: float, y: float, w: float, h: float, params: LayoutParams,
           seen: "_Dedupe") -> list[tuple[float, float]]:
    """Centres of the four dowel holes of the cell at ``(x, y, w, h)``.

    The hole *edge* sits ``params.hole_inset`` away from both cell edges it is
    near, so the centre is offset by ``hole_inset + hole_dia / 2``.  A hole that
    another cell already carries (neighbouring cells share their corners when
    the offset is zero) is dropped.
    """
    if not params.holes:
        return []
    off = params.hole_offset
    corners = [
        (x + off, y + off),
        (x + w - off, y + off),
        (x + w - off, y + h - off),
        (x + off, y + h - off),
    ]
    return [p for p in corners if seen.add(*p)]


# --------------------------------------------------------------------------- #
# Dividers and dots
# --------------------------------------------------------------------------- #
class _Snap:
    """Maps coordinates that are within ``tol`` of each other onto one value."""

    def __init__(self, tol: float) -> None:
        self.tol = tol
        self.values: list[float] = []

    def __call__(self, value: float) -> float:
        for known in self.values:
            if abs(known - value) <= self.tol:
                return known
        self.values.append(value)
        return value


def _merge(intervals: list[tuple[float, float]]) -> list[tuple[float, float]]:
    """Merge overlapping or touching 1-D intervals (sorted, non-degenerate)."""
    out: list[tuple[float, float]] = []
    for lo, hi in sorted(intervals):
        if hi - lo <= _MERGE_EPS:
            continue
        if out and lo <= out[-1][1] + _MERGE_EPS:
            if hi > out[-1][1]:
                out[-1] = (out[-1][0], hi)
        else:
            out.append((lo, hi))
    return out


def _block_of(areas: list[Area]) -> tuple[float, float, float, float]:
    """Bounding box of all cells - the same value as :attr:`Layout.block`."""
    if not areas:
        return (0.0, 0.0, 0.0, 0.0)
    return (min(a.x for a in areas), min(a.y for a in areas),
            max(a.x + a.w for a in areas), max(a.y + a.h for a in areas))


def _dividers(areas: list[Area], outer: bool, line_gap: float = 0.0
              ) -> list[tuple[float, float, float, float]]:
    """The dotted lines, as ``(x0, y0, x1, y1)``.

    Every cell is outlined by dividers: all four of its edges, whether they are
    shared with a neighbouring cell or face the empty space between two
    stencils - the user cuts every cell out along them, so the whole perimeter
    has to be marked.  The edge pieces of every cell are collected per line
    (same orientation, same coordinate within ``_MERGE_EPS``) and the ones that
    overlap or touch are merged into a single segment, so a shared edge becomes
    one divider (dotted once) and several cells abutting one long border give
    one straight divider with evenly spaced dots.

    A cell edge is then *doubled*: instead of one row of dots on the edge
    itself, two parallel lines ``line_gap`` apart are returned, centred on the
    edge (a vertical edge at ``x`` gives ``x - line_gap/2`` and
    ``x + line_gap/2``), both with the extent of the edge.  The cut runs between
    them.  ``line_gap <= 0`` keeps the single line on the edge.  Lines of
    different edges that end up on the same coordinate are merged again, so
    nothing is ever dotted twice.

    ``outer`` (``LayoutParams.outer_border``) only decides what happens on the
    outer boundary of the block, i.e. on the four lines of :attr:`Layout.block`:
    the vertical edges at the left- and rightmost cell edge and the horizontal
    ones at the bottom and top of the block.  There is nothing to cut apart
    there, so they are dropped unless ``outer`` is set.  Everything inside the
    block is always dotted.
    """
    # Collect the four edges of every cell per line: (orientation, coordinate).
    snap = {_VERTICAL: _Snap(_MERGE_EPS), _HORIZONTAL: _Snap(_MERGE_EPS)}
    lines: dict[tuple[int, float], list[tuple[float, float]]] = {}
    for area in areas:
        x0, y0, x1, y1 = area.rect
        raw = ((_VERTICAL, x0, y0, y1), (_VERTICAL, x1, y0, y1),
               (_HORIZONTAL, y0, x0, x1), (_HORIZONTAL, y1, x0, x1))
        for orientation, line, lo, hi in raw:
            if hi - lo <= _MERGE_EPS:
                continue                    # a degenerate cell has no edge here
            lines.setdefault((orientation, snap[orientation](line)), []).append((lo, hi))

    minx, miny, maxx, maxy = _block_of(areas)
    boundary = {_VERTICAL: (minx, maxx), _HORIZONTAL: (miny, maxy)}

    # Every surviving edge line becomes one dotted line (line_gap 0) or a pair.
    offsets = (0.0,) if line_gap <= 0.0 else (-line_gap / 2.0, line_gap / 2.0)
    shift = {_VERTICAL: _Snap(_MERGE_EPS), _HORIZONTAL: _Snap(_MERGE_EPS)}
    dotted: dict[tuple[int, float], list[tuple[float, float]]] = {}
    for (orientation, line), parts in sorted(lines.items()):
        if not outer and any(abs(line - edge) <= _MERGE_EPS
                             for edge in boundary[orientation]):
            continue                        # on the outer boundary of the block
        for offset in offsets:
            key = (orientation, shift[orientation](line + offset))
            dotted.setdefault(key, []).extend(parts)

    out: list[tuple[float, float, float, float]] = []
    for (orientation, line), parts in sorted(dotted.items()):
        for lo, hi in _merge(parts):
            out.append((line, lo, line, hi) if orientation == _VERTICAL
                       else (lo, line, hi, line))
    return out


def _dots(dividers: list[tuple[float, float, float, float]],
          params: LayoutParams) -> list[tuple[float, float]]:
    """Dot centres along the dividers, centred on each segment and deduplicated."""
    pitch = params.dot_pitch
    dots: list[tuple[float, float]] = []
    for x0, y0, x1, y1 in dividers:
        length = math.hypot(x1 - x0, y1 - y0)
        if length <= 0.0:
            dots.append((x0, y0))
            continue
        ux, uy = (x1 - x0) / length, (y1 - y0) / length
        n = int(math.floor(length / pitch)) if pitch > 0 else 0
        start = (length - n * pitch) / 2.0      # n == 0 -> a single dot in the middle
        for i in range(n + 1):
            d = start + i * pitch
            dots.append((x0 + ux * d, y0 + uy * d))
    keep = _Dedupe(params.dot_dia)
    return [p for p in dots if keep.add(*p)]


class _Dedupe:
    """Spatial hash that rejects points closer than ``min_dist`` to a kept one."""

    def __init__(self, min_dist: float) -> None:
        self.min_dist = min_dist
        self.limit = min_dist * min_dist
        self.cells: dict[tuple[int, int], list[tuple[float, float]]] = {}

    def add(self, x: float, y: float) -> bool:
        """Remember ``(x, y)`` and return ``True`` when it is a new point."""
        if self.min_dist <= 0.0:
            return True
        cx, cy = int(math.floor(x / self.min_dist)), int(math.floor(y / self.min_dist))
        for i in (cx - 1, cx, cx + 1):
            for j in (cy - 1, cy, cy + 1):
                for px, py in self.cells.get((i, j), ()):
                    if (px - x) ** 2 + (py - y) ** 2 < self.limit:
                        return False
        self.cells.setdefault((cx, cy), []).append((x, y))
        return True


# --------------------------------------------------------------------------- #
# Report
# --------------------------------------------------------------------------- #
def _fmt_point(p: tuple[float, float]) -> str:
    return f"({p[0]:.2f}, {p[1]:.2f})"


def _holes_on_grid(layout: Layout) -> bool:
    """Is every dowel hole centre of the sheet on the ``hole_grid`` raster?"""
    grid = layout.params.hole_grid
    if grid <= 0.0:
        return True
    for area in layout.areas:
        for point in area.holes:
            for value in point:
                steps = value / grid
                if abs(steps - round(steps)) * grid > _FIT_EPS:
                    return False
    return True


def _grid_waste(layout: Layout) -> float:
    """Extra cell area (mm2) the hole grid costs, against plain ``board + gap``."""
    gap = layout.params.gap
    waste = 0.0
    for area in layout.areas:
        project = area.side.project
        waste += area.w * area.h - (project.width + gap) * (project.height + gap)
    return waste


def layout_report(layout: Layout, config: Config) -> str:
    """Human readable description of a layout (stencil, cells, holes, counts)."""
    params = layout.params
    lines: list[str] = []
    orientation = ("portrait" if config.orientation == ORIENTATION_PORTRAIT
                   else "landscape")
    lines.append(f"Stencil: {layout.width:.1f} x {layout.height:.1f} mm "
                 f"({config.size_label} {orientation})")
    bx0, by0, bx1, by1 = layout.block
    lines.append(f"Block:   {layout.block_width:.2f} x {layout.block_height:.2f} mm "
                 f"at {_fmt_point((bx0, by0))}..{_fmt_point((bx1, by1))}   "
                 f"{'FITS' if layout.fits else 'DOES NOT FIT'}")
    spilled = layout.overflow
    heuristic = layout.heuristic or "unknown"
    # With a hole grid the cells are grown, so gap/2 is only the *minimum*.
    padding = (f"padding ≥ {params.pad:.1f} mm" if params.hole_grid > 0.0
               else f"padding {params.pad:.1f} mm")
    lines.append(f"Cells:   {len(layout.areas) - spilled} placed with MaxRects "
                 f"({heuristic}), {spilled} overflow, "
                 f"gap {params.gap:.1f} mm ({padding}), "
                 f"sort by {params.sort}")
    if params.holes:
        lines.append(f"Holes:   ⌀{params.hole_dia:.1f} mm, inset {params.hole_inset:.1f} mm "
                     f"(centres {params.hole_offset:.2f} mm inside the cell edge)")
    else:
        lines.append("Holes:   off")
    if params.hole_grid > 0.0:
        count = sum(len(area.holes) for area in layout.areas)
        verdict = "yes" if _holes_on_grid(layout) else "NO"
        lines.append(f"Grid:    dowel holes on a {params.hole_grid:.1f} mm grid: "
                     f"{verdict} ({count} hole(s), measured from the sheet origin)")
        lines.append(f"         cells grown for the grid: {_grid_waste(layout):,.1f} mm2 "
                     f"more than board + gap (the least this grid allows)")
    else:
        lines.append("Grid:    off (cells are exactly board + gap)")

    for index, area in enumerate(layout.areas, start=1):
        side = area.side
        label = side.label + (" (mirrored)" if side.mirror else "")
        bx0, by0, bx1, by1 = area.board_rect
        spill = " OVERFLOW" if area.overflow else ""
        lines.append("")
        lines.append(f"{index}. {label}   [cell {area.row}{spill}]")
        lines.append(f"   cell   x={area.x:.2f} y={area.y:.2f} "
                     f"w={area.w:.2f} h={area.h:.2f} mm")
        lines.append(f"   board  x {bx0:.2f}..{bx1:.2f}  y {by0:.2f}..{by1:.2f} mm "
                     f"({bx1 - bx0:.2f} x {by1 - by0:.2f} mm)")
        if area.holes:
            lines.append(f"   dowel holes (⌀{params.hole_dia:.1f} mm), "
                         f"sheet / relative to board corner {_fmt_point((bx0, by0))}:")
            for hx, hy in area.holes:
                lines.append(f"     {_fmt_point((hx, hy))}"
                             f"   rel {_fmt_point((hx - bx0, hy - by0))}")
        else:
            lines.append("   dowel holes: none (shared with a neighbouring cell or off)")

        candidates = side.candidates
        states = {state: 0 for state in (STATE_OPEN, STATE_IGNORE, STATE_UNDEFINED)}
        for pad in candidates:
            if pad.state in states:
                states[pad.state] += 1
        lines.append(f"   paste openings: {len(side.paste_objects)}")
        lines.append(f"   candidate pads: {len(candidates)} "
                     f"(open {states[STATE_OPEN]}, ignore {states[STATE_IGNORE]}, "
                     f"undefined {states[STATE_UNDEFINED]})")
        lines.append(f"   closed openings: {len(side.closed_pads)}")

    lines.append("")
    doubled = (f"two lines {params.dot_line_gap:.2f} mm apart"
               if params.dot_line_gap > 0.0 else "one line on the cell edge")
    lines.append(f"Divider dots: {len(layout.dots)} "
                 f"(⌀{params.dot_dia:.2f} mm, pitch {params.dot_pitch:.2f} mm, "
                 f"{len(layout.dividers)} dotted line(s) around every cell, "
                 f"{doubled}"
                 f"{', block boundary included' if params.outer_border else ''})")
    return "\n".join(lines)
