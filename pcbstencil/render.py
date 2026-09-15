"""High resolution PNG preview of a stencil layout.

The preview shows the whole stencil sheet the way it will be cut: everything
that becomes an opening is red, copper is dim grey, pads that still need a
decision are yellow, and pads the user excluded - together with the paste
openings the user closed - are blue.  It is meant to be looked at, not to be
exact - it is rasterised with Pillow straight from the shapely geometry of the
gerber objects.

Cells can sit anywhere on the sheet (they are packed with MaxRects, see
:mod:`pcbstencil.layout`); the only cell markings drawn are a faint dashed guide
under every dotted line (one per cell edge, ``dot_line_gap/2`` inside the cell) and,
when the datum rides on a raster (``slot_pitch`` for the slots, ``hole_grid``
for the holes), a very faint grid of that pitch over the whole sheet, so the
jig pins can be seen sitting on its intersections.  Cells that did not
fit are drawn outside the stencil boundary - the image simply grows to cover
them.

The alignment datum is drawn on top of the openings: the slots are openings
themselves (red obrounds), every jig pin is a dashed blue ghost circle where
the fixture pin comes through the foil, and the ``slots`` datum also marks the
corner the piece is pushed into with an orange bracket and a small green arrow
pointing at it.

The image carries a millimetre ruler along its left and bottom edge (origin at
the bottom left corner of the stencil) and a legend band underneath.  Sheet
millimetres are mapped to pixels with ``px_per_mm`` (reduced so the whole
image, bands included, stays below ``max_px``) and the Y axis is flipped,
because image rows grow downwards while sheet coordinates grow upwards.
"""
from __future__ import annotations

import math
import os
import subprocess
import sys
from functools import lru_cache
from typing import Callable, Iterable, Optional

from PIL import Image, ImageChops, ImageDraw, ImageFont
from shapely.affinity import affine_transform
from shapely.geometry import LineString, Point
from shapely.geometry.base import BaseGeometry

from .gerber import Region, Stroke, contour_points, object_geometry, polygons_of
from .model import STATE_IGNORE, STATE_OPEN, STATE_UNDEFINED, Area, Layout, Pad

__all__ = ["render_preview", "open_file"]

BACKGROUND = "#141414"
COLOR_SHEET = "#8a8a8a"          # the stencil boundary
COLOR_GUIDE = "#333333"          # faint dashed line under the divider dots
COLOR_GRID = "#202020"           # even fainter: the jig pin raster
COLOR_COPPER = "#2e2e2e"
COLOR_PAD = "#5a5a5a"
COLOR_OUTLINE = "#c8c8c8"
COLOR_IGNORE = "#3a5fcd"
COLOR_OPEN = "#ff2a2a"
COLOR_UNDEFINED = "#ffd000"
COLOR_SELECTED = "#00e5ff"
COLOR_PIN = "#4aa3ff"            # ghost outline of a jig pin (dashed)
COLOR_DATUM = "#ff9a1f"          # bracket at the corner the piece is pushed into
COLOR_NEST = "#39d98a"           # arrow showing the nesting direction
COLOR_LABEL = "#dddddd"
COLOR_RULER_BG = "#1c1c1c"
COLOR_RULER = "#b0b0b0"

_BAND_PX = 48                # smallest legend band height
_LEGEND_MM = 5.0             # nominal legend band height in millimetres
_REFERENCE_PPMM = 20.0       # scale the pixel-sized constants were chosen for
_RULER_MM = 6.0              # nominal ruler band thickness in millimetres
_RULER_MIN_PX = 28           # ... but never thinner than this
_TICK_MAJOR = 0.60           # tick lengths as a fraction of the nominal band
_TICK_MEDIUM = 0.40
_TICK_MINOR = 0.20
_LABEL_FRACTION = 0.55       # ruler label height as a fraction of the band
_MINOR_PPMM = 8.0            # 1 mm ticks only from this scale up
_LABEL_STEPS = (10, 20, 50, 100, 200, 500, 1000)

_FONT_PATHS = (
    "DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
)


# --------------------------------------------------------------------------- #
# Fonts
# --------------------------------------------------------------------------- #
@lru_cache(maxsize=64)
def _font(size: int) -> ImageFont.FreeTypeFont | ImageFont.ImageFont:
    """A DejaVu Sans face of the given pixel size, or Pillow's default."""
    size = max(1, int(size))
    for path in _FONT_PATHS:
        try:
            return ImageFont.truetype(path, size)
        except OSError:
            continue
    try:
        return ImageFont.load_default(size=size)
    except TypeError:       # very old Pillow
        return ImageFont.load_default()


def _text_width(font, text: str) -> float:
    try:
        return float(font.getlength(text))
    except AttributeError:      # pragma: no cover - ancient Pillow
        return 0.62 * getattr(font, "size", 10) * len(text)


def _fitted_font(text: str, size: int, available: float, minimum: int
                 ) -> ImageFont.FreeTypeFont | ImageFont.ImageFont:
    """The largest font at or below ``size`` that keeps ``text`` under ``available`` px."""
    font = _font(size)
    if available <= 0 or not text:
        return font
    length = _text_width(font, text)
    if length <= available or length <= 0:
        return font
    return _font(max(minimum, int(size * available / length)))


def _fit_label(text: str, size: int, available: float, minimum: int = 11):
    """Font and (possibly shortened) text that stay inside ``available`` px."""
    font = _fitted_font(text, size, available, minimum)
    if available <= 0:
        return font, ""
    if _text_width(font, text) <= available:
        return font, text
    short = text
    while short and _text_width(font, short + "…") > available:
        short = short[:-1]
    return font, (short + "…") if short else ""


# --------------------------------------------------------------------------- #
# Raster helpers
# --------------------------------------------------------------------------- #
class _ClassMask:
    """An 'L' mask collecting every shape of one colour class.

    Shapes are accumulated at full image size but the dirty region is tracked,
    so compositing only touches the pixels that were actually drawn.
    """

    def __init__(self, size: tuple[int, int]) -> None:
        self.size = size
        self.img = Image.new("L", size, 0)
        self.draw = ImageDraw.Draw(self.img)
        self.bbox: Optional[list[int]] = None

    def touch(self, x0: float, y0: float, x1: float, y1: float) -> None:
        """Extend the dirty region by a pixel-space box."""
        box = [int(math.floor(x0)) - 1, int(math.floor(y0)) - 1,
               int(math.ceil(x1)) + 1, int(math.ceil(y1)) + 1]
        if self.bbox is None:
            self.bbox = box
        else:
            self.bbox = [min(self.bbox[0], box[0]), min(self.bbox[1], box[1]),
                         max(self.bbox[2], box[2]), max(self.bbox[3], box[3])]

    def polygon(self, points: list[tuple[float, float]], value: int = 255) -> None:
        self.draw.polygon(points, fill=value)

    def ellipse(self, cx: float, cy: float, r: float, value: int = 255) -> None:
        self.draw.ellipse([cx - r, cy - r, cx + r, cy + r], fill=value)
        self.touch(cx - r, cy - r, cx + r, cy + r)

    def merge_crop(self, box: tuple[int, int, int, int], tmp: Image.Image,
                   erase: bool = False) -> None:
        """Merge a small mask into the class mask over ``box``."""
        region = self.img.crop(box)
        merged = ImageChops.subtract(region, tmp) if erase else ImageChops.lighter(region, tmp)
        self.img.paste(merged, box)

    def composite(self, img: Image.Image, color: str) -> None:
        """Paste ``color`` onto ``img`` wherever this mask is set."""
        if self.bbox is None:
            return
        x0 = max(0, min(self.size[0], self.bbox[0]))
        y0 = max(0, min(self.size[1], self.bbox[1]))
        x1 = max(x0, min(self.size[0], self.bbox[2]))
        y1 = max(y0, min(self.size[1], self.bbox[3]))
        if x1 <= x0 or y1 <= y0:
            return
        box = (x0, y0, x1, y1)
        img.paste(color, box, self.img.crop(box))


def _fill_polygon(mask: _ClassMask, poly, to_px: Callable[[float, float], tuple[float, float]],
                  value: int = 255) -> None:
    """Rasterise one shapely polygon (holes included) onto a class mask."""
    exterior = [to_px(x, y) for x, y in poly.exterior.coords]
    if len(exterior) < 3:
        return
    xs = [p[0] for p in exterior]
    ys = [p[1] for p in exterior]
    minx, maxx = min(xs), max(xs)
    miny, maxy = min(ys), max(ys)
    mask.touch(minx, miny, maxx, maxy)

    if maxx - minx < 1.0 and maxy - miny < 1.0:
        # Sub-pixel shape: keep at least one pixel so it does not disappear.
        px, py = int(minx), int(miny)
        if 0 <= px < mask.size[0] and 0 <= py < mask.size[1]:
            mask.draw.rectangle([px, py, px, py], fill=value)
        return

    if not poly.interiors:
        mask.polygon(exterior, value)
        return

    x0 = max(0, int(math.floor(minx)))
    y0 = max(0, int(math.floor(miny)))
    x1 = min(mask.size[0], int(math.ceil(maxx)) + 1)
    y1 = min(mask.size[1], int(math.ceil(maxy)) + 1)
    if x1 <= x0 or y1 <= y0:
        return
    tmp = Image.new("L", (x1 - x0, y1 - y0), 0)
    tdraw = ImageDraw.Draw(tmp)
    tdraw.polygon([(px - x0, py - y0) for px, py in exterior], fill=255)
    for ring in poly.interiors:
        hole = [to_px(x, y) for x, y in ring.coords]
        if len(hole) >= 3:
            tdraw.polygon([(px - x0, py - y0) for px, py in hole], fill=0)
    mask.merge_crop((x0, y0, x1, y1), tmp, erase=(value == 0))


def _fill_geometry(mask: _ClassMask, geom: BaseGeometry, matrix: list[float],
                   to_px: Callable[[float, float], tuple[float, float]],
                   value: int = 255) -> None:
    """Transform a board-coordinate geometry to the sheet and rasterise it."""
    if geom is None or geom.is_empty:
        return
    for poly in polygons_of(affine_transform(geom, matrix)):
        _fill_polygon(mask, poly, to_px, value)


def _fill_objects(mask: _ClassMask, objects: Iterable, matrix: list[float],
                  to_px: Callable[[float, float], tuple[float, float]]) -> None:
    """Rasterise gerber graphic objects, honouring clear (negative) polarity."""
    for obj in objects:
        try:
            geom = object_geometry(obj)
        except Exception:       # pragma: no cover - never fail a preview
            continue
        _fill_geometry(mask, geom, matrix, to_px, 255 if getattr(obj, "dark", True) else 0)


def _dashed_line(draw: ImageDraw.ImageDraw, p0: tuple[float, float], p1: tuple[float, float],
                 color: str, width: int, dash: float, space: float) -> None:
    """Draw a dashed segment between two pixel points."""
    length = math.hypot(p1[0] - p0[0], p1[1] - p0[1])
    if length <= 0:
        return
    ux, uy = (p1[0] - p0[0]) / length, (p1[1] - p0[1]) / length
    pos = 0.0
    while pos < length:
        end = min(pos + dash, length)
        draw.line([(p0[0] + ux * pos, p0[1] + uy * pos),
                   (p0[0] + ux * end, p0[1] + uy * end)], fill=color, width=width)
        pos = end + space


def _dashed_circle(draw: ImageDraw.ImageDraw, cx: float, cy: float, r: float,
                   color: str, width: int, dash_deg: float = 22.0,
                   gap_deg: float = 14.0) -> None:
    """A dashed circle outline (a ghost jig pin) around a pixel centre."""
    if r <= 0.0:
        return
    box = [cx - r, cy - r, cx + r, cy + r]
    if r < 3.0:                     # too small for dashes: a plain ring
        draw.ellipse(box, outline=color, width=width)
        return
    angle = 0.0
    while angle < 360.0:
        draw.arc(box, angle, min(360.0, angle + dash_deg), fill=color, width=width)
        angle += dash_deg + gap_deg


def _arrow(draw: ImageDraw.ImageDraw, tail: tuple[float, float],
           tip: tuple[float, float], color: str, width: int, head: float) -> None:
    """A thin arrow from ``tail`` to ``tip`` (pixel points) with a small head."""
    dx, dy = tip[0] - tail[0], tip[1] - tail[1]
    length = math.hypot(dx, dy)
    if length <= 0.0:
        return
    ux, uy = dx / length, dy / length
    draw.line([tail, tip], fill=color, width=width)
    for sign in (-1.0, 1.0):
        # rotate the reversed direction by ±30 degrees for the two barbs
        ca, sa = math.cos(math.radians(30.0)) , math.sin(math.radians(30.0)) * sign
        bx, by = -ux * ca - (-uy) * sa, -uy * ca + (-ux) * sa
        draw.line([tip, (tip[0] + bx * head, tip[1] + by * head)], fill=color, width=width)


# --------------------------------------------------------------------------- #
# Image geometry: rulers, legend band and scale
# --------------------------------------------------------------------------- #
class _Frame:
    """Pixel geometry of one preview image."""

    def __init__(self, ppmm: float, content_w: float, content_h: float,
                 legend: int) -> None:
        self.ppmm = ppmm
        self.content_w = content_w
        self.content_h = content_h
        self.legend = legend
        self.base = max(_RULER_MIN_PX, int(round(_RULER_MM * ppmm)))
        self.major = self.base * _TICK_MAJOR
        self.medium = self.base * _TICK_MEDIUM
        self.minor = self.base * _TICK_MINOR
        self.font_px = max(9, int(round(_LABEL_FRACTION * self.base)))
        self.font = _font(self.font_px)
        # Widest label that can occur on either ruler.
        widest = str(int(math.ceil(max(content_w, content_h, 1.0))))
        label_w = _text_width(self.font, widest)
        # The bands are grown when the labels would not fit next to the ticks.
        self.left = max(self.base, int(math.ceil(self.major + label_w + 0.30 * self.base)))
        self.bottom = max(self.base, int(math.ceil(self.major + 1.25 * self.font_px + 4)))
        self.label_w = label_w
        self.width_px = self.left + max(1, int(math.ceil(content_w * ppmm)))
        self.height_px = max(1, int(math.ceil(content_h * ppmm))) + self.bottom + legend
        #: image row of the stencil origin (y = 0)
        self.origin_y = self.height_px - self.bottom - legend
        self.longest = max(self.width_px, self.height_px)

    def to_px(self, x: float, y: float) -> tuple[float, float]:
        """Sheet millimetres -> image pixels (Y flipped)."""
        return (self.left + x * self.ppmm, self.origin_y - y * self.ppmm)

    def label_step(self, span: float, vertical: bool) -> int:
        """Millimetres between two labelled ticks so the labels never touch."""
        need = (1.8 * self.font_px if vertical
                else self.label_w + 0.8 * self.font_px)
        for step in _LABEL_STEPS:
            if step * self.ppmm >= need:
                return step
        return _LABEL_STEPS[-1]


def _legend_band(ppmm: float, max_px: int) -> int:
    """Height of the legend band in pixels."""
    band = max(_BAND_PX, int(round(_LEGEND_MM * ppmm)))
    return max(1, min(band, max(1, max_px // 3)))


def _frame_for(content_w: float, content_h: float, px_per_mm: float,
               max_px: int) -> _Frame:
    """The largest frame at or below ``px_per_mm`` that stays within ``max_px``.

    The ruler and legend bands grow with the scale and the scale is limited by
    the bands, so the fixed point is found by iteration instead of one shrink.
    """
    max_px = max(64, int(max_px))
    ppmm = max(px_per_mm, 1e-9)
    frame = _Frame(ppmm, content_w, content_h, _legend_band(ppmm, max_px))
    for _ in range(12):
        bands_w = frame.left
        bands_h = frame.bottom + frame.legend
        limit_w = (max_px - bands_w) / content_w if content_w > 0 else float("inf")
        limit_h = (max_px - bands_h) / content_h if content_h > 0 else float("inf")
        new = max(min(px_per_mm, limit_w, limit_h), 1e-3)
        if abs(new - ppmm) < 1e-9:
            break
        ppmm = new
        frame = _Frame(ppmm, content_w, content_h, _legend_band(ppmm, max_px))
    # Integer rounding (ceil) can still push the image one pixel over the cap.
    for _ in range(24):
        if frame.longest <= max_px:
            return frame
        ppmm *= 0.999
        frame = _Frame(ppmm, content_w, content_h, _legend_band(ppmm, max_px))
    return frame


def _draw_rulers(img: Image.Image, draw: ImageDraw.ImageDraw, frame: _Frame) -> None:
    """Millimetre rulers along the left and the bottom of the stencil area."""
    width_px, height_px = img.size
    ruler_bottom = frame.origin_y + frame.bottom
    # Bands (drawn over the content so nothing spills into them); the row and
    # the column of the stencil boundary itself are left untouched.
    draw.rectangle([0, frame.origin_y + 1, width_px, ruler_bottom - 1], fill=COLOR_RULER_BG)
    draw.rectangle([0, 0, frame.left - 1, ruler_bottom - 1], fill=COLOR_RULER_BG)

    thin = max(1, int(round(frame.ppmm / _REFERENCE_PPMM)))
    minor = frame.ppmm >= _MINOR_PPMM
    step_x = frame.label_step(frame.content_w, vertical=False)
    step_y = frame.label_step(frame.content_h, vertical=True)

    def level(mm: int) -> Optional[float]:
        if mm % 10 == 0:
            return frame.major
        if mm % 5 == 0:
            return frame.medium
        return frame.minor if minor else None

    gutter = 0.35 * frame.font_px      # smallest gap between two labels

    # Bottom ruler: X.
    used = -1e9
    for mm in range(0, int(math.floor(frame.content_w)) + 1):
        length = level(mm)
        if length is None:
            continue
        px = frame.left + mm * frame.ppmm
        draw.line([(px, frame.origin_y), (px, frame.origin_y + length)],
                  fill=COLOR_RULER, width=thin)
        if mm % step_x:
            continue
        text = str(mm)
        half = _text_width(frame.font, text) / 2.0
        tx = min(max(px, half + 2), width_px - half - 2)
        if tx - half < used + gutter:       # would touch the previous label
            continue
        used = tx + half
        draw.text((tx, frame.origin_y + frame.major + 2), text,
                  font=frame.font, fill=COLOR_RULER, anchor="ma")

    # Left ruler: Y (numbers grow upwards).
    used = 1e9
    for mm in range(0, int(math.floor(frame.content_h)) + 1):
        length = level(mm)
        if length is None:
            continue
        py = frame.origin_y - mm * frame.ppmm
        draw.line([(frame.left - length, py), (frame.left, py)],
                  fill=COLOR_RULER, width=thin)
        if mm % step_y:
            continue
        half = frame.font_px * 0.6
        ty = min(max(py, half + 2), frame.origin_y - half - 2)
        if ty + half > used - gutter:       # would touch the previous label
            continue
        used = ty - half
        draw.text((frame.left - frame.major - 3, ty), str(mm),
                  font=frame.font, fill=COLOR_RULER, anchor="rm")


# --------------------------------------------------------------------------- #
# Content helpers
# --------------------------------------------------------------------------- #
def _pad_groups(area: Area) -> dict[str, list[Pad]]:
    """Undefined candidate pads of one area, grouped by component reference."""
    groups: dict[str, list[Pad]] = {}
    for pad in area.side.pads:
        if pad.is_candidate and pad.state == STATE_UNDEFINED:
            groups.setdefault(pad.ref, []).append(pad)
    return groups


def _outline_paths(area: Area) -> list[list[tuple[float, float]]]:
    """Board outline polylines in sheet coordinates (empty when there is none)."""
    outline = area.side.project.outline
    if outline is None:
        return []
    apply = area.transform.apply
    paths: list[list[tuple[float, float]]] = []
    for obj in outline.objects:        # flashes on an outline layer carry no shape info
        if isinstance(obj, Stroke):
            pts = obj.seg.points()
            if len(pts) >= 2:
                paths.append([apply(x, y) for x, y in pts])
        elif isinstance(obj, Region):
            for contour in obj.contours:
                pts = contour_points(contour)
                if len(pts) >= 3:
                    ring = [apply(x, y) for x, y in pts]
                    if ring[0] != ring[-1]:
                        ring.append(ring[0])
                    paths.append(ring)
    return paths


#: Sheet coordinates are already sheet coordinates: no transform needed.
_IDENTITY = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]
_DATUM_LEG_MM = 4.0          # length of the two legs of the datum corner bracket
_NEST_TAIL_MM = 16.0         # the nesting arrow runs from here ...
_NEST_TIP_MM = 10.0          # ... to here, on the cell diagonal from the corner


def _obround(cx: float, cy: float, w: float, h: float) -> BaseGeometry:
    """An axis-aligned obround (stadium) polygon, sheet coordinates.

    ``w`` is the size along x, ``h`` along y; the short sides are half circles
    (a circle when the two are equal), exactly like the ``O`` gerber aperture
    the writer flashes for a slot.
    """
    r = min(w, h) / 2.0
    ex, ey = max(0.0, w / 2.0 - r), max(0.0, h / 2.0 - r)
    if ex <= 0.0 and ey <= 0.0:
        return Point(cx, cy).buffer(r, quad_segs=24)
    return LineString([(cx - ex, cy - ey), (cx + ex, cy + ey)]).buffer(r, quad_segs=24)


def _draw_datum(draw: ImageDraw.ImageDraw, layout: Layout,
                to_px: Callable[[float, float], tuple[float, float]],
                ppmm: float, scale: float) -> None:
    """Jig pins (both datums) and the datum corner marks (slots datum).

    Every pin is a dashed blue ghost circle where the fixture pin comes up
    through the foil - through a slot, or through a dowel hole.  With the
    slots datum the corner the piece is pushed into also gets an orange
    bracket and a small green arrow pointing at it from inside the cell.
    """
    params = layout.params
    pin_dia = params.pin_dia if params.slots else params.hole_dia
    pin_w = max(1, min(2, int(round(scale))))
    for area in layout.areas:
        for px, py in area.pins:
            cx, cy = to_px(px, py)
            _dashed_circle(draw, cx, cy, pin_dia / 2.0 * ppmm, COLOR_PIN, pin_w)
    if not params.slots:
        return
    leg = max(1, int(round(2 * scale)))
    for area in layout.areas:
        dx, dy = area.datum_corner
        span = min(_DATUM_LEG_MM, area.w / 4.0, area.h / 4.0)
        draw.line([to_px(dx + span, dy), to_px(dx, dy), to_px(dx, dy + span)],
                  fill=COLOR_DATUM, width=leg, joint="curve")
        # The arrow runs along the diagonal inside the padding: it stops short
        # of the board and its tip stays clear of the two slots.
        pad = min(area.board_rect[0] - dx, area.board_rect[1] - dy)
        tail = min(_NEST_TAIL_MM, pad - 0.75)
        tip = max(params.slot_inner + 1.5, tail - _NEST_TAIL_MM + _NEST_TIP_MM)
        if tail - tip >= 3.0:
            _arrow(draw, to_px(dx + tail, dy + tail), to_px(dx + tip, dy + tip),
                   COLOR_NEST, max(1, int(round(scale))), max(4.0, 2.0 * ppmm))


def _content_size(layout: Layout) -> tuple[float, float]:
    """Sheet area the image has to cover: the stencil plus any overflowing block."""
    return (max(layout.width, layout.block[2], 1.0),
            max(layout.height, layout.block[3], 1.0))


# --------------------------------------------------------------------------- #
# Public API
# --------------------------------------------------------------------------- #
def render_preview(layout: Layout, path: str, *, px_per_mm: float = 20.0, max_px: int = 12000,
                   selected: Pad | None = None, title: str = "") -> str:
    """Render ``layout`` to a PNG at ``path`` and return ``path``.

    The image shows the whole stencil sheet with a millimetre ruler on its left
    and bottom edge and a legend underneath.  ``px_per_mm`` is reduced
    automatically so the longer image side stays at or below ``max_px``,
    ``selected`` highlights a single pad in cyan and ``title`` is prepended to
    the legend line.  A layout that does not fit is drawn anyway: the image
    grows to cover the overflowing cells and the legend says so.
    """
    content_w, content_h = _content_size(layout)
    frame = _frame_for(content_w, content_h, px_per_mm, max_px)
    ppmm = frame.ppmm
    to_px = frame.to_px
    size = (frame.width_px, frame.height_px)
    params = layout.params

    scale = ppmm / _REFERENCE_PPMM
    thin = max(1, int(round(scale)))                 # 1 px at the reference scale
    outline_w = max(2, int(round(2 * scale)))
    select_w = max(3, int(round(3 * scale)))
    dash = max(4.0, 6.0 * scale)
    space = max(4.0, 5.0 * scale)

    img = Image.new("RGB", size, BACKGROUND)
    draw = ImageDraw.Draw(img)

    # 0. The dowel hole grid, under everything else: every hole centre sits on
    #    one of its intersections.
    _draw_pin_raster(draw, layout, to_px, ppmm)

    # 1. A very faint dashed guide under every dotted line (the dots are the
    #    real marking; there are two lines per cell edge); no rectangles.
    for dx0, dy0, dx1, dy1 in layout.dividers:
        _dashed_line(draw, to_px(dx0, dy0), to_px(dx1, dy1), COLOR_GUIDE, thin, dash, space)

    # Build one mask per colour class.
    copper = _ClassMask(size)
    pads = _ClassMask(size)
    ignored = _ClassMask(size)
    openings = _ClassMask(size)
    undefined = _ClassMask(size)

    for area in layout.areas:
        matrix = area.transform.affine()
        side = area.side
        _fill_objects(copper, side.copper_objects, matrix, to_px)
        for pad in side.pads:
            _fill_geometry(pads, pad.geom, matrix, to_px)
        # Only the paste openings that survive: the ones the user closed are
        # drawn in the "ignored" blue instead, so the change stays visible.
        _fill_objects(openings, side.active_paste_objects, matrix, to_px)
        _fill_objects(ignored, side.closed_paste_objects, matrix, to_px)
        for pad in side.candidates:
            if pad.state == STATE_OPEN:
                _fill_geometry(openings, pad.geom, matrix, to_px)
            elif pad.state == STATE_IGNORE:
                _fill_geometry(ignored, pad.geom, matrix, to_px)
            elif pad.state == STATE_UNDEFINED:
                _fill_geometry(undefined, pad.geom, matrix, to_px)

    # Dots and the datum openings are already in sheet coordinates.
    for cx, cy in layout.dots:
        px, py = to_px(cx, cy)
        openings.ellipse(px, py, max(1.0, params.dot_dia / 2.0 * ppmm))
    for area in layout.areas:
        for hx, hy in area.holes:
            px, py = to_px(hx, hy)
            openings.ellipse(px, py, max(1.0, params.hole_dia / 2.0 * ppmm))
        for sx, sy, sw, sh in area.slots:
            _fill_geometry(openings, _obround(sx, sy, sw, sh), _IDENTITY, to_px)

    # 2. Copper, then every pad.
    copper.composite(img, COLOR_COPPER)
    pads.composite(img, COLOR_PAD)

    # 3. The stencil boundary, then the board outlines.
    sx0, sy0 = to_px(0.0, layout.height)
    sx1, sy1 = to_px(layout.width, 0.0)
    draw.rectangle([sx0, sy0, sx1, sy1], outline=COLOR_SHEET, width=outline_w)

    for area in layout.areas:
        paths = _outline_paths(area)
        if paths:
            for pts in paths:
                draw.line([to_px(x, y) for x, y in pts], fill=COLOR_OUTLINE,
                          width=outline_w, joint="curve")
        else:
            bx0, by0, bx1, by1 = area.board_rect
            p0, p1 = to_px(bx0, by1), to_px(bx1, by0)
            draw.rectangle([p0[0], p0[1], p1[0], p1[1]], outline=COLOR_OUTLINE, width=thin)

    # 4.-6. Ignored pads, openings, undefined pads.
    ignored.composite(img, COLOR_IGNORE)
    openings.composite(img, COLOR_OPEN)
    undefined.composite(img, COLOR_UNDEFINED)

    # 6a. The jig: a ghost circle per pin, the datum corner of every cell.
    _draw_datum(draw, layout, to_px, ppmm, scale)

    # 6b. One labelled box per component that still has undefined pads.
    ref_font = _font(max(11, int(round(1.1 * ppmm))))
    grow = 0.4
    for area in layout.areas:
        matrix = area.transform.affine()
        for ref, group in _pad_groups(area).items():
            bounds = _group_bounds(group, matrix)
            if bounds is None:
                continue
            minx, miny, maxx, maxy = bounds
            p0 = to_px(minx - grow, maxy + grow)
            p1 = to_px(maxx + grow, miny - grow)
            draw.rectangle([p0[0], p0[1], p1[0], p1[1]], outline=COLOR_UNDEFINED, width=thin)
            draw.text((p0[0], p0[1] - 2 * thin), ref or "?", font=ref_font,
                      fill=COLOR_UNDEFINED, anchor="ld")

    # 7. Selected pad.
    if selected is not None:
        area = _area_of(layout, selected)
        if area is not None:
            bounds = _group_bounds([selected], area.transform.affine())
            if bounds is not None:
                minx, miny, maxx, maxy = bounds
                p0 = to_px(minx - 0.6, maxy + 0.6)
                p1 = to_px(maxx + 0.6, miny - 0.6)
                draw.rectangle([p0[0], p0[1], p1[0], p1[1]], outline=COLOR_SELECTED,
                               width=select_w)
                draw.text((p1[0] + 4 * thin, (p0[1] + p1[1]) / 2.0), selected.label,
                          font=ref_font, fill=COLOR_SELECTED, anchor="lm")

    # 8. Cell labels, inside the cell, right of the top left dowel hole.
    label_size = max(14, int(round(2.5 * ppmm)))
    if params.holes:
        left_span = right_span = max(params.hole_offset + params.hole_dia / 2.0, 0.0)
    elif params.slots:
        left_span, right_span = params.slot_inner, 0.0    # slots are low or at mid height
    else:
        left_span = right_span = 0.0
    for area in layout.areas:
        side = area.side
        text = f"{side.project.name} · {side.name}" + (" (mirrored)" if side.mirror else "")
        lx = area.x + left_span + 1.0
        px, py = to_px(lx, area.y + area.h - 1.0)
        available = max(0.0, (area.x + area.w - right_span - 1.0) - lx) * ppmm
        font, text = _fit_label(text, label_size, available)
        if text:
            draw.text((px, py), text, font=font, fill=COLOR_LABEL, anchor="la")

    # 9. Rulers over the bands, then the legend.
    _draw_rulers(img, draw, frame)

    prefix = f"{title}:  " if title else ""
    legend = (f"{prefix}stencil {layout.width:.0f} x {layout.height:.0f} mm"
              f"   block {layout.block_width:.1f} x {layout.block_height:.1f} mm"
              f"   {len(layout.areas)} cell(s)"
              "   red = stencil opening   yellow = undefined pad"
              "   blue = ignored pad / closed opening   grey = copper"
              "   blue outline = jig pin")
    warning = "" if layout.fits else "   —  DOES NOT FIT"
    left = max(6, int(round(6 * scale)))
    legend_font = _fitted_font(legend + warning, max(12, int(round(frame.legend * 0.42))),
                               frame.width_px - 2 * left, 10)
    base_y = frame.height_px - frame.legend / 2.0
    draw.text((left, base_y), legend, font=legend_font, fill=COLOR_LABEL, anchor="lm")
    if warning:
        draw.text((left + _text_width(legend_font, legend), base_y), warning,
                  font=legend_font, fill=COLOR_OPEN, anchor="lm")

    img.save(path, "PNG", compress_level=1)
    return path


def _draw_pin_raster(draw: ImageDraw.ImageDraw, layout: Layout,
                     to_px: Callable[[float, float], tuple[float, float]],
                     ppmm: float) -> None:
    """One hairline per raster pitch over the sheet (nothing when there is none).

    The raster is ``slot_pitch`` for the slots datum and ``hole_grid`` for the
    holes datum; it is measured from the sheet origin, exactly like the jig
    pins, so every pin has to sit on a crossing.  It is skipped when the lines
    would be closer than two pixels - at that scale it is noise, not
    information.
    """
    params = layout.params
    grid = (params.slot_pitch if params.slots
            else params.hole_grid if params.holes else 0.0)
    if grid <= 0.0 or grid * ppmm < 2.0:
        return
    for i in range(int(math.floor(layout.width / grid)) + 1):
        x = i * grid
        draw.line([to_px(x, 0.0), to_px(x, layout.height)], fill=COLOR_GRID, width=1)
    for i in range(int(math.floor(layout.height / grid)) + 1):
        y = i * grid
        draw.line([to_px(0.0, y), to_px(layout.width, y)], fill=COLOR_GRID, width=1)


def _group_bounds(pads: list[Pad], matrix: list[float]
                  ) -> Optional[tuple[float, float, float, float]]:
    """Sheet-coordinate bounding box of a group of pads."""
    boxes = []
    for pad in pads:
        geom = pad.geom
        if geom is None or geom.is_empty:
            continue
        boxes.append(affine_transform(geom, matrix).bounds)
    if not boxes:
        return None
    return (min(b[0] for b in boxes), min(b[1] for b in boxes),
            max(b[2] for b in boxes), max(b[3] for b in boxes))


def _area_of(layout: Layout, pad: Pad) -> Optional[Area]:
    """The area a pad belongs to, matched by project name and side name."""
    for area in layout.areas:
        if area.side.project.name == pad.project and area.side.name == pad.side:
            return area
    return None


def open_file(path: str) -> bool:
    """Open ``path`` with the desktop's default viewer, detached; never blocks.

    Returns ``False`` when no viewer could be launched.
    """
    try:
        if sys.platform.startswith("win"):
            os.startfile(path)      # type: ignore[attr-defined]
            return True
        cmd = ["open", path] if sys.platform == "darwin" else ["xdg-open", path]
        subprocess.Popen(cmd, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                         stderr=subprocess.DEVNULL, start_new_session=True)
        return True
    except Exception:
        return False
