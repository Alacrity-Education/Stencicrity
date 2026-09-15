"""MaxRects packing of PCB sides onto one stencil sheet.

Every *enabled* side (top/bottom of every project) becomes one rectangular
*cell* on the sheet: the board bounding box padded by at least ``gap/2`` on all
four sides, so two neighbouring boards are at least ``gap`` apart and the shared
edge of two touching cells sits in the middle of that gap.  A cell is exactly
``board + gap`` large unless the datum below asks for more (the slot padding,
the hole grid) - cells are never stretched to a common size and there are no
rows.

Every cell also carries the *datum* the jig pins locate it by
(``LayoutParams.datum``):

``slots``
    Obround slots at every position of the ``slot_pitch`` raster - the raster
    of the modular pin jig - along the cell's bottom edge and along its left
    edge (never the top or the right one), so a jig carrying pins on that
    raster can locate the piece with any of them.  Cell sizes are whole
    multiples of the pitch and cell corners sit at ``k * pitch - pin_offset``,
    so one raster runs across the whole sheet and touching cells share it; an
    edge of ``n`` pitches then carries ``n - 1`` slots and a cell is grown
    until its longer edge has two of them and its shorter edge one - what an
    exact three-contact location needs.  A slot's outer wall is
    ``slot_offset`` inside the cell edge: it clips the cell's own dotted line
    but never reaches into the neighbouring cell, which keeps the foil next to
    the board free for the squeegee.  The inner wall - ``slot_inner`` inside
    the edge - stays ``slot_web`` away from the board (the cell padding has a
    floor of ``min_pad_for_slots``).  The piece is lowered over the pins and
    pushed toward the bottom-left *datum corner*, so the wall nearest the
    board touches its pin: two contacts on one edge, one on the other, exact
    constraint.  ``marker`` cuts a small X into the foil at the raster point
    ``pin_offset`` inside that datum corner - where the left pin column meets
    the bottom pin row, the one raster point of the two edges that is too
    close to the corner to carry a slot - so which corner of a cut-out piece
    is the datum corner (and therefore which way round the piece goes) can be
    read at a glance.
``holes``
    Four round dowel holes near the cell corners, ``hole_inset`` away from the
    cell edges (the legacy, over-constrained scheme).
``none``
    No alignment features at all.

Both datums put *every* jig pin centre of the whole sheet on one common
raster, so the stencil can be pinned onto a fixture plate whose pins sit on
it.  A pin is ``pin_offset`` inside the cell corner, so the cell corner must
sit at ``k * pitch - pin_offset``: the packer only places cells there and the
block is centred by a whole number of pitches, so the raster is measured from
the sheet origin.  The ``slots`` datum uses ``slot_pitch`` and cells that are
whole multiples of it, so neighbouring cells still touch exactly.  The
``holes`` datum uses ``hole_grid``: there the hole-to-hole distance inside a
cell, ``cell - 2*hole_offset``, has to be a multiple of the pitch as well, so
the cell is grown until it is (the board stays centred and the padding only
ever gets larger than ``gap/2``); cells then no longer have to touch and the
leftover slivers are simply free space.  ``hole_grid = 0`` switches that off
and has no effect on the ``slots`` datum.

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

Every cell owns one *divider* along each of its four edges: a sparse row of
small round openings (the *dots*) running ``dot_line_gap / 2`` *inside* that
edge, so the user can see and cut the cell out.  The four lines of a cell meet
at their corners - together they are the cell rectangle inset by
``dot_line_gap / 2`` - and no line is ever drawn outside a cell.  Where two
cells touch, their two lines are ``dot_line_gap`` apart and the scissors cut
between them; where a cell edge faces free space (a packing gap, or a sliver
left by the hole grid) the cell still has its one line inside the edge and the
cut goes anywhere outside it.  ``dot_line_gap = 0`` puts every line on the edge
itself, so two touching cells then share one line.  Collinear pieces on one
line - two cells stacked along it with the same inset - are merged, so a long
straight border is one divider with evenly spaced dots.  The only lines that
may be left out are the ones whose *edge* lies on the outer boundary of the
block of cells (nothing has to be cut apart there): ``outer_border`` dots those
too.  :attr:`Layout.dividers` holds the dotted lines themselves, not the cell
edges.  A dot whose centre would fall inside a datum opening (a slot or a dowel
hole) is dropped - there is no foil left there to guide anything.

Sheet coordinates have their origin at the bottom left of the stencil, X to the
right and Y up, in millimetres (see :mod:`pcbstencil.model`).
"""
from __future__ import annotations

import math

from .model import (
    DATUM_NONE,
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

__all__ = ["pack", "layout_report", "ordered_sides", "marker_strokes",
           "marker_half"]

_EPS = 1e-9
_FIT_EPS = 1e-6
#: Two coordinates closer than this are the same coordinate (divider merging).
_MERGE_EPS = 1e-6
#: Touching cell edges are snapped together within this distance (see _aligned).
_ALIGN_EPS = 1e-9
#: Two holes closer than this are the same hole (neighbouring cells share them
#: when ``hole_inset == -hole_dia / 2``, i.e. the holes sit on the dotted line).
_HOLE_EPS = 1e-6
#: Slack of the pin raster arithmetic, in steps (see _snap_up / _grid_size).
_GRID_EPS = 1e-9
#: Safety net of _edge_for_slots: no cell edge is ever grown past this many
#: pitches looking for a slot position.
_MAX_PITCH_STEPS = 1000

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
# The jig pin grid
# --------------------------------------------------------------------------- #
def _grid_of(params: LayoutParams) -> float:
    """The pin raster that actually applies to the layout's datum.

    ``slots`` rides on ``slot_pitch`` (the raster of the modular jig, which
    also sizes the cells), ``holes`` on ``hole_grid``, ``none`` on nothing.
    """
    if params.slots:
        return max(0.0, params.slot_pitch)
    if params.holes:
        return max(0.0, params.hole_grid)
    return 0.0


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
    """The smallest cell corner ``>= value`` whose pins land on the grid.

    A cell corner at ``x`` carries a pin at ``x + ho`` (and, for the holes
    datum, another one at ``x + w - ho``), so ``x + ho`` has to be a multiple
    of ``grid``.  A ``value`` that is already admissible (up to floating point
    noise) is returned as the exact grid position; ``grid <= 0`` returns it
    unchanged.
    """
    if grid <= 0.0:
        return value
    steps = (value + ho) / grid
    nearest = round(steps)
    steps = nearest if abs(steps - nearest) * grid <= _ALIGN_EPS else math.ceil(steps)
    return steps * grid - ho


def _snap_down(value: float, grid: float) -> float:
    """``value`` (>= 0) floored onto a multiple of ``grid``; keeps pins aligned."""
    if grid <= 0.0:
        return value
    steps = value / grid
    nearest = round(steps)
    steps = nearest if abs(steps - nearest) * grid <= _ALIGN_EPS else math.floor(steps)
    return max(0.0, steps * grid)


def _ceil_pitch(value: float, pitch: float) -> float:
    """``value`` rounded up to a whole (at least one) multiple of ``pitch``."""
    steps = max(1, math.ceil(value / pitch - _GRID_EPS))
    return steps * pitch


# --------------------------------------------------------------------------- #
# Cells and their datum features
# --------------------------------------------------------------------------- #
def _cell_of(board_w: float, board_h: float, params: LayoutParams,
             grid: float) -> tuple[float, float]:
    """Cell size around one board: ``board + gap``, grown for the datum.

    ``slots`` needs ``min_pad_for_slots`` of padding on every side (the slot
    plus the web to the board), a size that is a whole number of ``slot_pitch``
    steps - so touching cells keep one raster - and enough of them for two
    slots on the longer edge and one on the shorter (see :func:`_slot_offsets`).
    ``holes`` grows the cell until the hole spacing is a multiple of
    ``hole_grid``.  ``none`` is exactly ``board + gap``.
    """
    cw, ch = board_w + params.gap, board_h + params.gap
    if params.slots:
        pad = max(params.pad, params.min_pad_for_slots)
        cw, ch = board_w + 2.0 * pad, board_h + 2.0 * pad
        pitch = max(0.0, params.slot_pitch)
        if pitch <= 0.0:
            return (cw, ch)                      # no raster: padding only
        cw, ch = _ceil_pitch(cw, pitch), _ceil_pitch(ch, pitch)
        two, one = _edge_for_slots(2, params), _edge_for_slots(1, params)
        if cw >= ch:                             # two slots along the longer edge
            return (max(cw, two), max(ch, one))
        return (max(cw, one), max(ch, two))
    if params.holes:
        return _cell_size(cw, ch, params.hole_offset, grid)
    return (cw, ch)


def _slot_span(start: float, size: float, params: LayoutParams) -> tuple[float, float]:
    """Range a slot centre may take along one cell edge of ``size``.

    The slot has to stay ``slot_offset`` clear of the two cell edges it runs
    into, so its centre lives in ``[slot_offset + length/2, size - ...]``.
    """
    half = params.slot_length / 2.0
    return (start + params.slot_offset + half, start + size - params.slot_offset - half)


def _slot_offsets(size: float, params: LayoutParams) -> list[float]:
    """Slot centres along one cell edge of ``size``, measured from its corner.

    The cell corner sits at ``k * slot_pitch - pin_offset``, so the raster
    positions seen from the corner are ``pin_offset + k * slot_pitch``; every
    one of them that leaves the slot ``slot_offset`` clear of both ends of the
    edge carries a slot (with the default numbers an edge of ``n`` pitches
    gets ``n - 1``).  All of them are opened, so a modular jig may use any.
    """
    pitch = max(0.0, params.slot_pitch)
    if pitch <= 0.0:
        return []
    lo, hi = _slot_span(0.0, size, params)
    if hi < lo:
        return []                                # the cell is shorter than a slot
    offset = params.pin_offset
    first = math.ceil((lo - offset) / pitch - _GRID_EPS)
    last = math.floor((hi - offset) / pitch + _GRID_EPS)
    return [offset + k * pitch for k in range(first, last + 1)]


def _edge_for_slots(count: int, params: LayoutParams) -> float:
    """Shortest whole number of pitches whose edge carries ``count`` slots."""
    pitch = max(0.0, params.slot_pitch)
    if pitch <= 0.0:
        return 0.0
    steps = 1
    while (len(_slot_offsets(steps * pitch, params)) < count
           and steps < _MAX_PITCH_STEPS):
        steps += 1
    return steps * pitch


def marker_strokes(center: tuple[float, float], size: float
                   ) -> list[tuple[float, float, float, float]]:
    """The two crossed strokes of an X marker, as ``(x0, y0, x1, y1)``.

    Two segments of length ``size`` at +45 and -45 degrees through ``center``:
    together they are an X whose bounding square is ``size * sqrt(2) / 2``
    wide (a 4 mm X reaches 1.41 mm from its centre).  Both the gerber writer
    and the preview stroke them ``dot_dia`` wide, so the foil actually goes
    ``dot_dia / 2`` further out - see :func:`marker_half`.
    """
    cx, cy = center
    half = max(0.0, size) * math.sqrt(2.0) / 4.0        # half the X's square
    return [(cx - half, cy - half, cx + half, cy + half),
            (cx - half, cy + half, cx + half, cy - half)]


def marker_half(params: LayoutParams) -> float:
    """Half the side of the square an X marker cuts out of the foil.

    The strokes themselves reach ``marker_size * sqrt(2) / 4`` from the
    centre; the round ``dot_dia`` stroke adds half its width on top.
    """
    return (max(0.0, params.marker_size) * math.sqrt(2.0) / 4.0
            + max(0.0, params.dot_dia) / 2.0)


def _marker(x: float, y: float, params: LayoutParams) -> tuple[float, float] | None:
    """Centre of the X orientation marker of the cell at ``(x, y)``, or ``None``.

    Only the ``slots`` datum has one, and only when ``params.marker`` is set:
    the raster point ``pin_offset`` inside the datum corner, where the left
    pin column meets the bottom pin row.  That point is the one raster
    position of the two edges that never carries a slot - a slot needs
    ``slot_offset + slot_length / 2`` of clearance from the corner, which with
    any sane numbers is more than ``pin_offset`` - so the X is cut into free
    foil.
    """
    if not (params.slots and params.marker):
        return None
    return (x + params.pin_offset, y + params.pin_offset)


def _datum(x: float, y: float, w: float, h: float, params: LayoutParams,
           grid: float, seen: "_Dedupe"
           ) -> tuple[list[tuple[float, float]],
                      list[tuple[float, float, float, float]],
                      list[tuple[float, float]]]:
    """The alignment features of the cell at ``(x, y, w, h)``.

    Returns ``(holes, slots, pins)`` in sheet coordinates.  For ``slots``
    every raster position along the bottom edge and along the left edge gets
    one (bottom edge first, left to right, then the left edge bottom to top);
    the pins are tangent to the slot walls nearest the board (the piece is
    pushed toward the bottom-left corner, so a bottom pin touches its slot
    from below and a left pin from the left).  For ``holes`` the pins are the
    holes.
    """
    if params.slots:
        width, length = params.slot_width, params.slot_length
        across = params.slot_offset + width / 2.0     # cell edge to the slot centre
        pin = params.pin_offset                       # == slot_inner - pin_dia / 2
        slots: list[tuple[float, float, float, float]] = []
        pins: list[tuple[float, float]] = []
        for offset in _slot_offsets(w, params):       # the bottom edge
            slots.append((x + offset, y + across, length, width))
            pins.append((x + offset, y + pin))
        for offset in _slot_offsets(h, params):       # the left edge
            slots.append((x + across, y + offset, width, length))
            pins.append((x + pin, y + offset))
        return [], slots, pins
    if params.holes:
        holes = _holes(x, y, w, h, params, seen)
        return holes, [], list(holes)
    return [], [], []


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
    fit into any free rectangle.  With a pin ``grid`` the bottom left corner of
    a free rectangle is first snapped up to the next position whose pins land
    on the raster; the cell has to fit in what is left of the free rectangle
    and is scored there.  The sliver below and left of the cell stays free
    space (with the ``slots`` datum only along the sheet edges: cell sizes are
    whole multiples of the pitch, so cells placed next to each other touch).
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
    and their cells (``board + gap``, grown for the datum) are packed with
    MaxRects; the block of placed cells is then centred on the stencil (by a
    whole number of raster pitches, so the jig pins stay on the raster).  Cells
    that fit nowhere are lined up to the right of the sheet and make
    ``Layout.fits`` ``False``.
    """
    params = config.layout
    sheet_w, sheet_h = config.sheet_size()
    grid = _grid_of(params)         # slot_pitch (slots) or hole_grid (holes)
    ho = params.pin_offset          # cell edge to pin centre, for both datums

    order = ordered_sides(sides, params)
    cells: list[tuple[float, float]] = []
    for side in order:
        minx, miny, maxx, maxy = side.project.bbox
        # gap/2 per side, then grown for the datum (slot raster / hole grid).
        cells.append(_cell_of(maxx - minx, maxy - miny, params, grid))

    # 1. Pack, then line the cells that fit nowhere up right of the sheet.
    spots, heuristic = _best_packing(cells, sheet_w, sheet_h, ho, grid)
    overflow = [i for i, spot in enumerate(spots) if spot is None]
    overflow_set = set(overflow)
    x, y = _snap_up(sheet_w, ho, grid), _snap_up(0.0, ho, grid)
    for i in overflow:
        spots[i] = (x, y)
        x = _snap_up(x + cells[i][0], ho, grid)

    # 2. Centre the block of *placed* cells on the stencil (overflow moves too).
    #    The offset is a whole number of raster pitches, so every pin stays on
    #    the raster measured from the sheet origin.
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
    seen = _Dedupe(_HOLE_EPS)
    for index, (side, (cw, ch)) in enumerate(zip(order, cells)):
        cx, cy = xs[index], ys[index]
        minx, miny, maxx, maxy = side.project.bbox
        board_w, board_h = maxx - minx, maxy - miny
        # The board is centred inside its cell: gap/2 on every side, more when
        # the cell was grown for the datum.
        bx = cx + (cw - board_w) / 2.0
        by = cy + (ch - board_h) / 2.0
        # Board -> sheet.  A mirrored board occupies [-maxx, -minx] in x.
        dx = bx - (-maxx if side.mirror else minx)
        dy = by - miny
        cell_holes, slots, pins = _datum(cx, cy, cw, ch, params, grid, seen)
        area = Area(
            side=side,
            x=cx, y=cy, w=cw, h=ch,
            board_rect=(bx, by, bx + board_w, by + board_h),
            transform=Transform(mirror=side.mirror, dx=dx, dy=dy),
            row=index,                       # informational: the packing order
            holes=cell_holes,
            slots=slots,
            pins=pins,
            # The corner the piece is pushed into (a mirrored side has it at the
            # board's physical bottom-right).
            datum_corner=(cx, cy),
            # The X that says which corner that is (slots datum, marker on).
            marker=_marker(cx, cy, params),
        )
        # Cells that fit nowhere are drawn outside the stencil (see the report).
        area.overflow = index in overflow_set
        areas.append(area)

    block = _block_of(areas)          # the dividers use the same boundary
    fits = (not overflow
            and block[0] >= -_FIT_EPS and block[1] >= -_FIT_EPS
            and block[2] <= sheet_w + _FIT_EPS and block[3] <= sheet_h + _FIT_EPS)

    dividers = _dividers(areas, params.outer_border, params.dot_line_gap)
    dots = _dots(dividers, params, _Cover(areas, params))
    layout = Layout(params=params, areas=areas, width=sheet_w, height=sheet_h,
                    block=block, fits=fits, dots=dots, dividers=dividers,
                    heuristic=heuristic, overflow=len(overflow))
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

    Every cell owns one dotted line along each of its four edges, running
    ``line_gap / 2`` *inside* that edge (a left edge at ``x`` gives a line at
    ``x + line_gap/2``, a right edge one at ``x - line_gap/2``, and so on).
    The extent of a line is the extent of its edge shortened by the same
    ``line_gap / 2`` at both ends, so the four lines of a cell meet at their
    corners instead of sticking out: together they are exactly the cell
    rectangle inset by ``line_gap / 2``.  No line is ever drawn outside a cell.

    Where two cells touch, their two lines are ``line_gap`` apart and the
    scissors cut between them; where a cell edge faces free space the cell
    still has its one line inside the edge and the cut goes anywhere outside
    it.  ``line_gap <= 0`` puts every line on the edge itself, so two touching
    cells then share one line.

    A cell thinner than ``line_gap`` cannot hold the inset; it is then clamped
    to half the cell in that direction, which collapses the two opposite lines
    onto the cell's centre line and drops the two perpendicular ones (they
    would have no length left).

    The lines are collected per line (same orientation, same coordinate within
    ``_MERGE_EPS``) and the pieces that overlap or touch are merged into a
    single segment, so two cells stacked along one line with the same inset
    give one straight divider with evenly spaced dots.

    ``outer`` (``LayoutParams.outer_border``) only decides what happens on the
    outer boundary of the block, i.e. on the four lines of :attr:`Layout.block`:
    the vertical edges at the left- and rightmost cell edge and the horizontal
    ones at the bottom and top of the block.  There is nothing to cut apart
    there, so their lines are dropped unless ``outer`` is set.  The test is made
    on the cell *edge*, not on the position of the line it carries.
    """
    minx, miny, maxx, maxy = _block_of(areas)
    boundary = {_VERTICAL: (minx, maxx), _HORIZONTAL: (miny, maxy)}

    half = max(line_gap, 0.0) / 2.0
    snap = {_VERTICAL: _Snap(_MERGE_EPS), _HORIZONTAL: _Snap(_MERGE_EPS)}
    lines: dict[tuple[int, float], list[tuple[float, float]]] = {}
    for area in areas:
        x0, y0, x1, y1 = area.rect
        # This cell's own rectangle, inset by line_gap/2 (clamped so it never
        # turns inside out): the four lines run along its sides.
        ix, iy = min(half, (x1 - x0) / 2.0), min(half, (y1 - y0) / 2.0)
        lox, hix, loy, hiy = x0 + ix, x1 - ix, y0 + iy, y1 - iy
        # (orientation, the cell edge, its line, the extent of that line)
        raw = ((_VERTICAL, x0, lox, loy, hiy), (_VERTICAL, x1, hix, loy, hiy),
               (_HORIZONTAL, y0, loy, lox, hix), (_HORIZONTAL, y1, hiy, lox, hix))
        for orientation, edge, line, lo, hi in raw:
            if hi - lo <= _MERGE_EPS:
                continue                    # no room left for this line
            if not outer and any(abs(edge - bound) <= _MERGE_EPS
                                 for bound in boundary[orientation]):
                continue                    # edge on the outer boundary of the block
            lines.setdefault((orientation, snap[orientation](line)), []).append((lo, hi))

    out: list[tuple[float, float, float, float]] = []
    for (orientation, line), parts in sorted(lines.items()):
        for lo, hi in _merge(parts):
            out.append((line, lo, line, hi) if orientation == _VERTICAL
                       else (lo, line, hi, line))
    return out


def _dots(dividers: list[tuple[float, float, float, float]],
          params: LayoutParams, cover: "_Cover | None" = None
          ) -> list[tuple[float, float]]:
    """Dot centres along the dividers, centred on each segment and deduplicated.

    A dot whose centre falls inside a datum opening (``cover``: a slot or a
    dowel hole) is dropped - there is no foil there to mark.
    """
    pitch = params.dot_pitch
    inside = cover.covers if cover is not None else (lambda x, y: False)
    dots: list[tuple[float, float]] = []
    for x0, y0, x1, y1 in dividers:
        length = math.hypot(x1 - x0, y1 - y0)
        if length <= 0.0:
            if not inside(x0, y0):
                dots.append((x0, y0))
            continue
        ux, uy = (x1 - x0) / length, (y1 - y0) / length
        n = int(math.floor(length / pitch)) if pitch > 0 else 0
        start = (length - n * pitch) / 2.0      # n == 0 -> a single dot in the middle
        for i in range(n + 1):
            d = start + i * pitch
            px, py = x0 + ux * d, y0 + uy * d
            if not inside(px, py):
                dots.append((px, py))
    keep = _Dedupe(params.dot_dia)
    return [p for p in dots if keep.add(*p)]


def _in_obround(px: float, py: float, ax: float, ay: float,
                bx: float, by: float, r: float) -> bool:
    """Is ``(px, py)`` inside the segment ``a-b`` grown by ``r`` (a stadium)?"""
    dx, dy = bx - ax, by - ay
    span = dx * dx + dy * dy
    t = 0.0 if span <= 0.0 else min(1.0, max(0.0, ((px - ax) * dx + (py - ay) * dy) / span))
    qx, qy = px - (ax + t * dx), py - (ay + t * dy)
    return qx * qx + qy * qy <= r * r


class _Cover:
    """The datum openings of a layout, hashed so a point test is O(1).

    Every opening - an obround slot or a round dowel hole - is stored as a
    segment plus a radius and bucketed by a raster at least as coarse as the
    largest of them, so a point only has to be tested against the openings in
    its own bucket.  An X marker is covered by its bounding square (the two
    strokes plus their width, :func:`marker_half`): a dot anywhere in there
    sits in or right beside the X, so it is dropped as well.
    """

    def __init__(self, areas: list[Area], params: LayoutParams) -> None:
        shapes: list[tuple[float, float, float, float, float]] = []
        squares: list[tuple[float, float, float, float]] = []
        biggest = 0.0
        half = marker_half(params)
        for area in areas:
            for cx, cy, w, h in area.slots:
                r = min(w, h) / 2.0
                ex, ey = max(0.0, w / 2.0 - r), max(0.0, h / 2.0 - r)
                shapes.append((cx - ex, cy - ey, cx + ex, cy + ey, r))
                biggest = max(biggest, w, h)
            if params.holes:
                r = params.hole_dia / 2.0
                for hx, hy in area.holes:
                    shapes.append((hx, hy, hx, hy, r))
                    biggest = max(biggest, params.hole_dia)
            if area.marker is not None and half > 0.0:
                mx, my = area.marker
                squares.append((mx - half, my - half, mx + half, my + half))
                biggest = max(biggest, 2.0 * half)
        self.cell = max(biggest, 1.0)
        self.buckets: dict[tuple[int, int], list[tuple[float, float, float, float, float]]] = {}
        self.squares: dict[tuple[int, int], list[tuple[float, float, float, float]]] = {}
        for shape in shapes:
            ax, ay, bx, by, r = shape
            self._bucket(self.buckets, min(ax, bx) - r, min(ay, by) - r,
                         max(ax, bx) + r, max(ay, by) + r, shape)
        for square in squares:
            self._bucket(self.squares, *square, square)

    def _bucket(self, target: dict, x0: float, y0: float, x1: float, y1: float,
                item) -> None:
        """File ``item`` under every bucket its bounding box touches."""
        for i in range(int(math.floor(x0 / self.cell)),
                       int(math.floor(x1 / self.cell)) + 1):
            for j in range(int(math.floor(y0 / self.cell)),
                           int(math.floor(y1 / self.cell)) + 1):
                target.setdefault((i, j), []).append(item)

    def covers(self, x: float, y: float) -> bool:
        """Is ``(x, y)`` inside a datum opening or an X marker's square?"""
        if not self.buckets and not self.squares:
            return False
        key = (int(math.floor(x / self.cell)), int(math.floor(y / self.cell)))
        if any(x0 <= x <= x1 and y0 <= y <= y1
               for x0, y0, x1, y1 in self.squares.get(key, ())):
            return True
        return any(_in_obround(x, y, *shape) for shape in self.buckets.get(key, ()))


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


def _crowded(params: LayoutParams) -> str:
    """Why the slots of one cell would run into each other, or ``""``.

    Only reachable with hand-set numbers: a slot longer than the raster pitch
    makes two neighbours on the same edge overlap, and a first raster position
    closer to the corner than ``slot_inner + slot_length/2`` makes the first
    bottom slot and the first left slot meet in the corner of the cell.
    """
    if params.slot_pitch <= 0.0:
        return "slot_pitch must be positive: no slots are cut at all"
    if params.slot_length > params.slot_pitch + _FIT_EPS:
        return (f"slot_length {params.slot_length:g} mm is longer than the "
                f"{params.slot_pitch:g} mm raster: two slots on one edge overlap")
    first = _slot_offsets(1000.0 * max(params.slot_pitch, 1.0), params)
    if first and first[0] < params.slot_inner + params.slot_length / 2.0 - _FIT_EPS:
        return ("the first raster position is so close to the corner that the "
                "first bottom slot and the first left slot overlap there")
    return ""


def _marker_trouble(layout: Layout) -> str:
    """Why the X marker does not fit where it is cut, or ``""``.

    Only reachable with hand-set numbers: an X that reaches further than
    ``pin_offset`` crosses the cell edge, and one long enough to reach the
    first raster position of an edge runs into that slot.
    """
    params = layout.params
    half = marker_half(params)
    if half > params.pin_offset + _FIT_EPS:
        return (f"the {params.marker_size:g} mm X reaches {half:.2f} mm from its "
                f"centre but is cut only {params.pin_offset:.2f} mm inside the "
                f"cell edge: it crosses the edge")
    for area in layout.areas:
        if area.marker is None:
            continue
        mx, my = area.marker
        for sx, sy, sw, sh in area.slots:
            if (abs(sx - mx) < (sw + 2.0 * half) / 2.0 - _FIT_EPS
                    and abs(sy - my) < (sh + 2.0 * half) / 2.0 - _FIT_EPS):
                return (f"the {params.marker_size:g} mm X at "
                        f"{_fmt_point((mx, my))} reaches the slot at "
                        f"{_fmt_point((sx, sy))}")
    return ""


def _edge_counts(area: Area, params: LayoutParams) -> tuple[int, int]:
    """How many slots ``area`` carries on its bottom edge and on its left edge."""
    return (len(_slot_offsets(area.w, params)), len(_slot_offsets(area.h, params)))


def _on_grid(value: float, grid: float) -> bool:
    """Is ``value`` a whole number of ``grid``, measured from the sheet origin?"""
    if grid <= 0.0:
        return True
    steps = value / grid
    return abs(steps - round(steps)) * grid <= _FIT_EPS


def _pins_on_grid(layout: Layout) -> bool:
    """Is every jig pin centre of the sheet on the datum's raster?"""
    grid = _grid_of(layout.params)
    if grid <= 0.0:
        return True
    return all(_on_grid(value, grid)
               for area in layout.areas for point in area.pins for value in point)


def _grid_waste(layout: Layout) -> float:
    """Extra cell area (mm2) the datum costs, against plain ``board + gap``."""
    gap = layout.params.gap
    waste = 0.0
    for area in layout.areas:
        project = area.side.project
        waste += area.w * area.h - (project.width + gap) * (project.height + gap)
    return waste


def layout_report(layout: Layout, config: Config) -> str:
    """Human readable description of a layout (stencil, cells, datum, counts)."""
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
    # Cells grow for the slot raster and for the hole grid, so gap/2 is then
    # only the *minimum* padding.
    floor_pad = max(params.pad, params.min_pad_for_slots) if params.slots else params.pad
    exact_pad = _grid_of(params) <= 0.0 and not params.slots
    padding = (f"padding {floor_pad:.1f} mm" if exact_pad
               else f"padding ≥ {floor_pad:.2f} mm")
    lines.append(f"Cells:   {len(layout.areas) - spilled} placed with MaxRects "
                 f"({heuristic}), {spilled} overflow, "
                 f"gap {params.gap:.1f} mm ({padding}), "
                 f"sort by {params.sort}")
    if params.slots:
        counts = sorted({_edge_counts(area, params) for area in layout.areas})
        spread = ", ".join(f"{b}+{l}" for b, l in counts) or "none"
        lines.append(f"Datum:   slots at a {params.slot_pitch:g} mm raster along the bottom "
                     f"and left edges, outer wall {params.slot_offset:g} mm inside the "
                     f"cell edge, inner wall {params.slot_inner:.1f} mm, pins "
                     f"⌀{params.pin_dia:g} mm tangent to the inner walls on the same "
                     f"raster, push toward the bottom-left corner")
        lines.append(f"         {params.slot_width:g} x {params.slot_length:g} mm obround; "
                     f"every raster position that fits is opened, so a modular jig may "
                     f"use any of them (bottom+left per cell: {spread})")
        lines.append(f"         cells are whole multiples of the raster, at least two "
                     f"slots on the longer edge and one on the shorter; every slot lies "
                     f"inside its own cell, ≥ {params.slot_web:g} mm of foil to the board")
        crowded = _crowded(params)
        if crowded:
            lines.append(f"         WARNING: {crowded}")
        band = max(params.dot_line_gap / 2.0 + params.dot_dia / 2.0, params.dot_dia)
        if params.slot_offset < band:
            lines.append(f"         the slots clip this cell's own dotted line "
                         f"({params.dot_line_gap / 2.0:.2f} mm inside the edge) and stop "
                         f"{params.slot_offset:g} mm short of the neighbouring cell: "
                         f"intended, it frees the foil beside the board for the squeegee")
        if params.marker:
            lines.append(f"Marker:  X {params.marker_size:g} mm at the raster point "
                         f"{params.pin_offset:g} mm inside the datum corner, where the "
                         f"left pin column meets the bottom pin row (the one raster "
                         f"point that never carries a slot)")
            lines.append(f"         two crossed strokes at ±45°, {params.dot_dia:g} mm "
                         f"wide, reaching {marker_half(params):.2f} mm from the centre: "
                         f"the orientation of a cut-out piece can be read at a glance")
            trouble = _marker_trouble(layout)
            if trouble:
                lines.append(f"         WARNING: {trouble}")
        else:
            lines.append("Marker:  off (no orientation X is cut)")
    elif params.holes:
        lines.append(f"Datum:   holes: ⌀{params.hole_dia:.1f} mm, inset {params.hole_inset:.1f} mm "
                     f"(centres {params.hole_offset:.2f} mm inside the cell edge)")
    else:
        lines.append("Datum:   none")
    raster = _grid_of(params)
    if raster > 0.0:
        count = sum(len(area.pins) for area in layout.areas)
        verdict = "yes" if _pins_on_grid(layout) else "NO"
        source = "slot_pitch" if params.slots else "hole_grid"
        lines.append(f"Grid:    pin centres on the {raster:.1f} mm {source} raster: "
                     f"{verdict} ({count} pin(s), measured from the sheet origin)")
        lines.append(f"         cells grown for the raster: {_grid_waste(layout):,.1f} mm2 "
                     f"more than board + gap (the least this raster allows)")
        if params.slots and params.hole_grid > 0.0:
            lines.append(f"         hole_grid ({params.hole_grid:g} mm) is set but applies "
                         f"to the holes datum only")
    elif params.datum == DATUM_NONE:
        lines.append("Grid:    off (datum none: no pins to align)")
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
        if params.slots:
            dcx, dcy = area.datum_corner
            note = ("  (mirrored: the board's physical bottom-right)" if side.mirror
                    else "")
            lines.append(f"   datum corner {_fmt_point((dcx, dcy))}"
                         f"   rel {_fmt_point((dcx - bx0, dcy - by0))}{note}")
            if area.marker is not None:
                mx, my = area.marker
                lines.append(f"   marker X ({params.marker_size:g} mm strokes) "
                             f"{_fmt_point((mx, my))}"
                             f"   rel {_fmt_point((mx - bx0, my - by0))}")
            bottom, left = _edge_counts(area, params)
            lines.append(f"   slots ({params.slot_width:g} x {params.slot_length:g} mm "
                         f"obround), {bottom} on the bottom edge + {left} on the left "
                         f"edge, sheet / relative to board corner "
                         f"{_fmt_point((bx0, by0))}:")
            for sx, sy, sw, sh in area.slots:
                lines.append(f"     {_fmt_point((sx, sy))} {sw:.2f} x {sh:.2f} mm"
                             f"   rel {_fmt_point((sx - bx0, sy - by0))}")
        elif area.holes:
            lines.append(f"   dowel holes (⌀{params.hole_dia:.1f} mm), "
                         f"sheet / relative to board corner {_fmt_point((bx0, by0))}:")
            for hx, hy in area.holes:
                lines.append(f"     {_fmt_point((hx, hy))}"
                             f"   rel {_fmt_point((hx - bx0, hy - by0))}")
        elif params.holes:
            lines.append("   dowel holes: none (shared with a neighbouring cell)")
        else:
            lines.append("   datum features: none")
        if area.pins:
            pin_dia = params.pin_dia if params.slots else params.hole_dia
            lines.append(f"   jig pins (⌀{pin_dia:g} mm), sheet / relative to board "
                         f"corner {_fmt_point((bx0, by0))}:")
            grid = _grid_of(params)
            for px, py in area.pins:
                off = "" if grid <= 0.0 or _on_grid(px, grid) and _on_grid(py, grid) \
                    else "   OFF THE RASTER (the cell is too small for it)"
                lines.append(f"     {_fmt_point((px, py))}"
                             f"   rel {_fmt_point((px - bx0, py - by0))}{off}")
        if params.slots:
            board = ("+x and -y in board coordinates, the board's physical bottom-right"
                     if side.mirror else "-x and -y in board coordinates")
            lines.append(f"   nesting: push toward the datum corner, sheet -x and -y "
                         f"({board}); first -y onto the bottom pins, then -x onto "
                         f"the left one(s)")

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
    inset = (f"{params.dot_line_gap / 2.0:.2f} mm inside the edge"
             if params.dot_line_gap > 0.0 else "on the edge itself")
    lines.append(f"Divider dots: {len(layout.dots)} "
                 f"(⌀{params.dot_dia:.2f} mm, pitch {params.dot_pitch:.2f} mm, "
                 f"{len(layout.dividers)} dotted line(s), one per cell edge, "
                 f"{inset}"
                 f"{', block boundary included' if params.outer_border else ''})")
    return "\n".join(lines)
