"""Minimal RS-274X (Gerber X2) reader with shapely geometry.

Supports what KiCad emits: standard apertures (C/R/O/P), aperture macros
(primitives 1, 2/20, 21, 4, 5, 7), linear and circular interpolation,
regions (G36/G37), polarity and X2 file/aperture/object attributes.
"""
from __future__ import annotations

import math
import re
from dataclasses import dataclass, field
from typing import Iterator, Optional, Union

from shapely import affinity
from shapely.geometry import LineString, MultiPolygon, Point, Polygon, box
from shapely.geometry.base import BaseGeometry
from shapely.ops import unary_union
from shapely.validation import make_valid


class GerberError(Exception):
    """Malformed or unsupported Gerber input."""


ARC_TOLERANCE = 0.004  # mm chord error used when discretising arcs
_TWO_PI = 2.0 * math.pi


# --------------------------------------------------------------------------- #
# Geometry helpers
# --------------------------------------------------------------------------- #
def _arc_steps(radius: float, sweep: float) -> int:
    """Number of chords needed so the chord error stays below ARC_TOLERANCE."""
    sweep = abs(sweep)
    if radius <= ARC_TOLERANCE:
        theta = math.pi / 4
    else:
        theta = 2.0 * math.acos(max(-1.0, min(1.0, 1.0 - ARC_TOLERANCE / radius)))
        theta = min(theta, math.pi / 4)
    return max(1, min(4000, int(math.ceil(sweep / theta))))


def arc_sweep(x0: float, y0: float, x1: float, y1: float,
              cx: float, cy: float, clockwise: bool) -> float:
    """Signed sweep angle (radians) from start to end around the centre."""
    a0 = math.atan2(y0 - cy, x0 - cx)
    a1 = math.atan2(y1 - cy, x1 - cx)
    if clockwise:
        sweep = -((a0 - a1) % _TWO_PI)
    else:
        sweep = (a1 - a0) % _TWO_PI
    if abs(sweep) < 1e-9 and math.hypot(x1 - x0, y1 - y0) < 1e-9:
        sweep = -_TWO_PI if clockwise else _TWO_PI  # full circle
    return sweep


def arc_points(x0: float, y0: float, x1: float, y1: float,
               cx: float, cy: float, clockwise: bool) -> list[tuple[float, float]]:
    """Discretise an arc into points including both end points."""
    r0 = math.hypot(x0 - cx, y0 - cy)
    r1 = math.hypot(x1 - cx, y1 - cy)
    sweep = arc_sweep(x0, y0, x1, y1, cx, cy, clockwise)
    if abs(sweep) < 1e-9:
        return [(x0, y0), (x1, y1)]
    n = _arc_steps(max(r0, r1), sweep)
    a0 = math.atan2(y0 - cy, x0 - cx)
    pts = []
    for i in range(n + 1):
        t = i / n
        r = r0 + (r1 - r0) * t
        a = a0 + sweep * t
        pts.append((cx + r * math.cos(a), cy + r * math.sin(a)))
    pts[0] = (x0, y0)
    pts[-1] = (x1, y1)
    return pts


def _rotate(x: float, y: float, deg: float) -> tuple[float, float]:
    if not deg:
        return x, y
    a = math.radians(deg)
    c, s = math.cos(a), math.sin(a)
    return x * c - y * s, x * s + y * c


def _fix(geom: BaseGeometry) -> BaseGeometry:
    if geom.is_empty:
        return geom
    if not geom.is_valid:
        geom = make_valid(geom)
    return geom


def _polygonal(geom: BaseGeometry) -> BaseGeometry:
    """Drop any non-area parts (points/lines) that boolean ops may leave behind."""
    if geom.is_empty or isinstance(geom, (Polygon, MultiPolygon)):
        return geom
    polys = polygons_of(geom)
    if not polys:
        return Polygon()
    return _fix(unary_union(polys))


def polygons_of(geom: BaseGeometry) -> list[Polygon]:
    """Flatten any geometry into a list of polygons (drops lines/points)."""
    if geom is None or geom.is_empty:
        return []
    if isinstance(geom, Polygon):
        return [geom]
    out: list[Polygon] = []
    for part in getattr(geom, "geoms", []):
        out.extend(polygons_of(part))
    return out


# --------------------------------------------------------------------------- #
# Aperture macros
# --------------------------------------------------------------------------- #
class _Expr:
    """Tiny recursive-descent evaluator for macro expressions ($1+$1, 2x$3, ...)."""

    def __init__(self, text: str, variables: dict[int, float]):
        self.s = "".join(text.split())
        self.i = 0
        self.vars = variables

    def _peek(self) -> str:
        return self.s[self.i] if self.i < len(self.s) else ""

    def _take(self) -> str:
        c = self.s[self.i]
        self.i += 1
        return c

    def parse(self) -> float:
        v = self._expr()
        if self.i != len(self.s):
            raise GerberError(f"bad macro expression: {self.s!r}")
        return v

    def _expr(self) -> float:
        v = self._term()
        while self._peek() in ("+", "-") and self._peek():
            op = self._take()
            t = self._term()
            v = v + t if op == "+" else v - t
        return v

    def _term(self) -> float:
        v = self._factor()
        while self._peek() in ("x", "X", "/") and self._peek():
            op = self._take()
            f = self._factor()
            v = v * f if op in ("x", "X") else v / f
        return v

    def _factor(self) -> float:
        c = self._peek()
        if c == "-":
            self._take()
            return -self._factor()
        if c == "+":
            self._take()
            return self._factor()
        if c == "(":
            self._take()
            v = self._expr()
            if self._take() != ")":
                raise GerberError(f"missing ')' in {self.s!r}")
            return v
        if c == "$":
            self._take()
            j = self.i
            while self._peek().isdigit():
                self._take()
            return self.vars.get(int(self.s[j:self.i] or "0"), 0.0)
        j = self.i
        while self._peek() and (self._peek().isdigit() or self._peek() == "."):
            self._take()
        if j == self.i:
            raise GerberError(f"unexpected {c!r} in macro expression {self.s!r}")
        return float(self.s[j:self.i])


@dataclass
class MacroPrim:
    """A fully evaluated macro primitive, reduced to a circle or an outline."""
    kind: str                 # "circle" | "outline"
    exposure: bool
    d: float = 0.0
    cx: float = 0.0
    cy: float = 0.0
    points: list[tuple[float, float]] = field(default_factory=list)

    def geometry(self) -> BaseGeometry:
        if self.kind == "circle":
            return Point(self.cx, self.cy).buffer(self.d / 2.0, quad_segs=24)
        return _polygonal(_fix(Polygon(self.points)))

    def mirrored(self) -> "MacroPrim":
        if self.kind == "circle":
            return MacroPrim("circle", self.exposure, self.d, -self.cx, self.cy)
        return MacroPrim("outline", self.exposure, points=[(-x, y) for x, y in self.points])

    def as_gerber(self) -> str:
        exp = "1" if self.exposure else "0"
        if self.kind == "circle":
            return f"1,{exp},{self.d:.6f},{self.cx:.6f},{self.cy:.6f}*"
        pts = list(self.points)
        if len(pts) > 1 and _close(pts[0], pts[-1]):
            pts.pop()
        coords = ",".join(f"{x:.6f},{y:.6f}" for x, y in pts + [pts[0]])
        return f"4,{exp},{len(pts)},{coords},0*"


def _close(a: tuple[float, float], b: tuple[float, float]) -> bool:
    return abs(a[0] - b[0]) < 1e-9 and abs(a[1] - b[1]) < 1e-9


@dataclass
class Macro:
    name: str
    blocks: list[str]

    def evaluate(self, params: list[float]) -> list[MacroPrim]:
        variables = {i + 1: p for i, p in enumerate(params)}
        prims: list[MacroPrim] = []
        for raw in self.blocks:
            blk = raw.strip()
            if not blk or blk[0] == "0":
                continue  # comment
            if blk[0] == "$":
                name, expr = blk.split("=", 1)
                variables[int(name[1:])] = _Expr(expr, variables).parse()
                continue
            parts = blk.split(",")
            code = int(parts[0])
            mods = [_Expr(p, variables).parse() for p in parts[1:]]
            prims.extend(self._primitive(code, mods))
        return prims

    def _primitive(self, code: int, m: list[float]) -> list[MacroPrim]:
        if code == 1:
            exp = m[0] != 0
            d, cx, cy = m[1], m[2], m[3]
            rot = m[4] if len(m) > 4 else 0.0
            cx, cy = _rotate(cx, cy, rot)
            return [MacroPrim("circle", exp, d, cx, cy)]
        if code in (2, 20):
            exp = m[0] != 0
            w, x1, y1, x2, y2 = m[1:6]
            rot = m[6] if len(m) > 6 else 0.0
            dx, dy = x2 - x1, y2 - y1
            length = math.hypot(dx, dy)
            if length < 1e-12:
                return [MacroPrim("circle", exp, w, *_rotate(x1, y1, rot))]
            nx, ny = -dy / length * w / 2.0, dx / length * w / 2.0
            pts = [(x1 + nx, y1 + ny), (x2 + nx, y2 + ny), (x2 - nx, y2 - ny), (x1 - nx, y1 - ny)]
            return [MacroPrim("outline", exp, points=[_rotate(x, y, rot) for x, y in pts])]
        if code == 21:
            exp = m[0] != 0
            w, h, cx, cy = m[1:5]
            rot = m[5] if len(m) > 5 else 0.0
            pts = [(cx - w / 2, cy - h / 2), (cx + w / 2, cy - h / 2),
                   (cx + w / 2, cy + h / 2), (cx - w / 2, cy + h / 2)]
            return [MacroPrim("outline", exp, points=[_rotate(x, y, rot) for x, y in pts])]
        if code == 4:
            exp = m[0] != 0
            n = int(round(m[1]))
            coords = m[2:2 + 2 * (n + 1)]
            rot = m[2 + 2 * (n + 1)] if len(m) > 2 + 2 * (n + 1) else 0.0
            pts = [(coords[i], coords[i + 1]) for i in range(0, len(coords) - 1, 2)]
            if len(pts) > 1 and _close(pts[0], pts[-1]):
                pts.pop()
            return [MacroPrim("outline", exp, points=[_rotate(x, y, rot) for x, y in pts])]
        if code == 5:
            exp = m[0] != 0
            n = int(round(m[1]))
            cx, cy, d = m[2], m[3], m[4]
            rot = m[5] if len(m) > 5 else 0.0
            pts = [(cx + d / 2 * math.cos(_TWO_PI * i / n), cy + d / 2 * math.sin(_TWO_PI * i / n))
                   for i in range(n)]
            return [MacroPrim("outline", exp, points=[_rotate(x, y, rot) for x, y in pts])]
        if code == 7:
            cx, cy, od, idia, gap = m[0:5]
            rot = m[5] if len(m) > 5 else 0.0
            ring = Point(cx, cy).buffer(od / 2, quad_segs=24).difference(
                Point(cx, cy).buffer(idia / 2, quad_segs=24))
            cross = unary_union([box(cx - od, cy - gap / 2, cx + od, cy + gap / 2),
                                 box(cx - gap / 2, cy - od, cx + gap / 2, cy + od)])
            geom = affinity.rotate(ring.difference(cross), rot, origin=(0, 0))
            return [MacroPrim("outline", True, points=list(p.exterior.coords)[:-1])
                    for p in polygons_of(geom)]
        if code == 6:
            return []  # moiré: decoration only, ignore
        raise GerberError(f"unsupported macro primitive {code} in {self.name}")


# --------------------------------------------------------------------------- #
# Apertures
# --------------------------------------------------------------------------- #
@dataclass
class Aperture:
    code: int
    template: str                   # "C", "R", "O", "P" or macro name
    modifiers: list[float]
    attrs: dict[str, str] = field(default_factory=dict)
    macro: Optional[Macro] = None
    _geom: Optional[BaseGeometry] = field(default=None, repr=False)
    _prims: Optional[list[MacroPrim]] = field(default=None, repr=False)

    @property
    def function(self) -> str:
        """First token of the AperFunction attribute (e.g. 'SMDPad')."""
        return self.attrs.get("AperFunction", "").split(",")[0].strip()

    def prims(self) -> list[MacroPrim]:
        if self.macro is None:
            return []
        if self._prims is None:
            self._prims = self.macro.evaluate(self.modifiers)
        return self._prims

    def geometry(self) -> BaseGeometry:
        """Aperture shape centred on the origin (mm)."""
        if self._geom is None:
            self._geom = self._build_geometry()
        return self._geom

    def _build_geometry(self) -> BaseGeometry:
        m = self.modifiers
        t = self.template
        hole = 0.0
        if t == "C":
            geom = Point(0, 0).buffer(m[0] / 2.0, quad_segs=24)
            hole = m[1] if len(m) > 1 else 0.0
        elif t == "R":
            geom = box(-m[0] / 2, -m[1] / 2, m[0] / 2, m[1] / 2)
            hole = m[2] if len(m) > 2 else 0.0
        elif t == "O":
            w, h = m[0], m[1]
            if abs(w - h) < 1e-9:
                geom = Point(0, 0).buffer(w / 2.0, quad_segs=24)
            elif w > h:
                geom = LineString([(-(w - h) / 2, 0), ((w - h) / 2, 0)]).buffer(h / 2.0, quad_segs=24)
            else:
                geom = LineString([(0, -(h - w) / 2), (0, (h - w) / 2)]).buffer(w / 2.0, quad_segs=24)
            hole = m[2] if len(m) > 2 else 0.0
        elif t == "P":
            d, n = m[0], int(round(m[1]))
            rot = m[2] if len(m) > 2 else 0.0
            pts = [_rotate(d / 2 * math.cos(_TWO_PI * i / n), d / 2 * math.sin(_TWO_PI * i / n), rot)
                   for i in range(n)]
            geom = Polygon(pts)
            hole = m[3] if len(m) > 3 else 0.0
        elif self.macro is not None:
            geom = Polygon()
            for prim in self.prims():
                g = prim.geometry()
                geom = geom.union(g) if prim.exposure else geom.difference(g)
                geom = _polygonal(_fix(geom))
        else:
            raise GerberError(f"unknown aperture template {t!r} for D{self.code}")
        if hole > 0:
            geom = geom.difference(Point(0, 0).buffer(hole / 2.0, quad_segs=24))
        return geom

    def describe(self) -> str:
        m = self.modifiers
        t = self.template
        if t == "C":
            return f"C ⌀{m[0]:.2f}"
        if t in ("R", "O"):
            return f"{t} {m[0]:.2f}x{m[1]:.2f}"
        if t == "P":
            return f"P ⌀{m[0]:.2f} n={int(m[1])}"
        minx, miny, maxx, maxy = self.geometry().bounds
        return f"{t} {maxx - minx:.2f}x{maxy - miny:.2f}"


@dataclass
class BakedAperture:
    """An aperture with all macro variables resolved; what the writer emits."""
    template: str                   # "C", "R", "O", "P" or "MACRO"
    modifiers: list[float]
    prims: list[MacroPrim]
    attrs: dict[str, str]

    def key(self) -> str:
        attrs = ";".join(f"{k}={v}" for k, v in sorted(self.attrs.items()))
        if self.template == "MACRO":
            body = "".join(p.as_gerber() for p in self.prims)
            return f"MACRO|{body}|{attrs}"
        mods = "X".join(f"{v:.6f}" for v in self.modifiers)
        return f"{self.template},{mods}|{attrs}"


def bake_aperture(ap: Aperture, mirror: bool = False) -> BakedAperture:
    """Resolve an aperture for output, optionally mirrored about the Y axis."""
    if ap.macro is not None:
        prims = ap.prims()
        if mirror:
            prims = [p.mirrored() for p in prims]
        return BakedAperture("MACRO", [], prims, dict(ap.attrs))
    mods = list(ap.modifiers)
    if ap.template == "P" and mirror:
        rot = mods[2] if len(mods) > 2 else 0.0
        if len(mods) > 2:
            mods[2] = 180.0 - rot
        else:
            mods.append(180.0 - rot)
    return BakedAperture(ap.template, mods, [], dict(ap.attrs))


# --------------------------------------------------------------------------- #
# Graphic objects
# --------------------------------------------------------------------------- #
@dataclass
class Segment:
    """One contour segment of a region, or the path of a stroke."""
    x0: float
    y0: float
    x1: float
    y1: float
    arc: bool = False
    cx: float = 0.0
    cy: float = 0.0
    clockwise: bool = False

    def points(self) -> list[tuple[float, float]]:
        if self.arc:
            return arc_points(self.x0, self.y0, self.x1, self.y1, self.cx, self.cy, self.clockwise)
        return [(self.x0, self.y0), (self.x1, self.y1)]


@dataclass
class Flash:
    x: float
    y: float
    aperture: Aperture
    attrs: dict[str, str]
    dark: bool = True


@dataclass
class Stroke:
    seg: Segment
    aperture: Aperture
    attrs: dict[str, str]
    dark: bool = True


@dataclass
class Region:
    contours: list[list[Segment]]
    attrs: dict[str, str]
    dark: bool = True


GraphicObject = Union[Flash, Stroke, Region]


def object_geometry(obj: GraphicObject) -> BaseGeometry:
    """Shapely geometry of a graphic object in file coordinates (mm)."""
    if isinstance(obj, Flash):
        return affinity.translate(obj.aperture.geometry(), obj.x, obj.y)
    if isinstance(obj, Stroke):
        ap = obj.aperture
        pts = obj.seg.points()
        if ap.template == "C":
            width = ap.modifiers[0]
            if len(pts) < 2 or all(_close(p, pts[0]) for p in pts):
                return Point(pts[0]).buffer(width / 2.0, quad_segs=24)
            return LineString(pts).buffer(width / 2.0, quad_segs=24)
        # Non-round stroke aperture: hull of the aperture swept along the path
        shapes = [affinity.translate(ap.geometry(), x, y) for x, y in pts]
        return unary_union(shapes).convex_hull
    if isinstance(obj, Region):
        polys = []
        for contour in obj.contours:
            pts: list[tuple[float, float]] = []
            for seg in contour:
                sp = seg.points()
                if pts and _close(pts[-1], sp[0]):
                    sp = sp[1:]
                pts.extend(sp)
            if len(pts) >= 3:
                polys.append(_fix(Polygon(pts)))
        if not polys:
            return Polygon()
        return _polygonal(_fix(unary_union(polys)))
    raise TypeError(f"unknown object {obj!r}")


def contour_points(contour: list[Segment]) -> list[tuple[float, float]]:
    pts: list[tuple[float, float]] = []
    for seg in contour:
        sp = seg.points()
        if pts and _close(pts[-1], sp[0]):
            sp = sp[1:]
        pts.extend(sp)
    return pts


# --------------------------------------------------------------------------- #
# File container and parser
# --------------------------------------------------------------------------- #
@dataclass
class GerberFile:
    name: str = ""
    file_attrs: dict[str, str] = field(default_factory=dict)
    macros: dict[str, Macro] = field(default_factory=dict)
    apertures: dict[int, Aperture] = field(default_factory=dict)
    objects: list[GraphicObject] = field(default_factory=list)
    unit: str = "mm"

    @property
    def file_function(self) -> str:
        return self.file_attrs.get("FileFunction", "")

    def bounds(self) -> Optional[tuple[float, float, float, float]]:
        geoms = [object_geometry(o) for o in self.objects]
        geoms = [g for g in geoms if not g.is_empty]
        if not geoms:
            return None
        minx = min(g.bounds[0] for g in geoms)
        miny = min(g.bounds[1] for g in geoms)
        maxx = max(g.bounds[2] for g in geoms)
        maxy = max(g.bounds[3] for g in geoms)
        return minx, miny, maxx, maxy


_TOKEN_RE = re.compile(r"([A-Z])([+-]?[0-9.]*)")
_AD_RE = re.compile(r"^ADD(\d+)([A-Za-z_.$][A-Za-z0-9_.$]*)(?:,(.*))?$")


def _iter_blocks(text: str) -> Iterator[tuple[str, str]]:
    i, n = 0, len(text)
    while i < n:
        c = text[i]
        if c in " \r\n\t":
            i += 1
            continue
        if c == "%":
            j = text.find("%", i + 1)
            if j < 0:
                raise GerberError("unterminated extended command")
            yield "ext", text[i + 1:j]
            i = j + 1
        else:
            j = text.find("*", i)
            if j < 0:
                raise GerberError("unterminated data block")
            yield "data", text[i:j].strip()
            i = j + 1


class GerberParser:
    def __init__(self) -> None:
        self.gf = GerberFile()
        self.int_digits = 4
        self.dec_digits = 6
        self.trailing_zero_omission = False
        self.incremental = False
        self.scale = 1.0          # to mm
        self.cur_ta: dict[str, str] = {}
        self.cur_to: dict[str, str] = {}
        self.aperture: Optional[Aperture] = None
        self.interp = 1           # 1 linear, 2 cw, 3 ccw
        self.multi_quadrant = True
        self.dark = True
        self.x = 0.0
        self.y = 0.0
        self.in_region = False
        self.contours: list[list[Segment]] = []
        self.region_attrs: dict[str, str] = {}
        self.done = False

    # -- public --------------------------------------------------------------
    def parse(self, text: str, name: str = "") -> GerberFile:
        self.gf.name = name
        for kind, content in _iter_blocks(text):
            if self.done:
                break
            if kind == "ext":
                self._extended(content)
            else:
                self._data(content)
        if self.in_region:
            self._end_region()
        return self.gf

    # -- extended commands ---------------------------------------------------
    def _extended(self, content: str) -> None:
        parts = [p.strip() for p in content.split("*")]
        parts = [p for p in parts if p]
        if not parts:
            return
        if parts[0].startswith("AM"):
            name = parts[0][2:].strip()
            self.gf.macros[name] = Macro(name, parts[1:])
            return
        for cmd in parts:
            self._ext_cmd(cmd)

    def _ext_cmd(self, cmd: str) -> None:
        code = cmd[:2]
        if code == "FS":
            m = re.match(r"FS([LT]?)([AI]?)X(\d)(\d)Y(\d)(\d)", cmd)
            if not m:
                raise GerberError(f"bad format spec {cmd!r}")
            self.trailing_zero_omission = m.group(1) == "T"
            self.incremental = m.group(2) == "I"
            self.int_digits, self.dec_digits = int(m.group(3)), int(m.group(4))
        elif code == "MO":
            unit = cmd[2:4]
            self.scale = 25.4 if unit == "IN" else 1.0
            self.gf.unit = "mm"
        elif code == "AD":
            self._define_aperture(cmd)
        elif code == "LP":
            self.dark = cmd[2:3] != "C"
        elif code == "TF":
            k, v = self._split_attr(cmd[2:])
            self.gf.file_attrs[k] = v
        elif code == "TA":
            k, v = self._split_attr(cmd[2:])
            self.cur_ta[k] = v
        elif code == "TO":
            k, v = self._split_attr(cmd[2:])
            self.cur_to[k] = v
        elif code == "TD":
            name = cmd[2:].lstrip(".")
            if not name:
                self.cur_ta.clear()
                self.cur_to.clear()
            else:
                self.cur_ta.pop(name, None)
                self.cur_to.pop(name, None)
        elif code in ("LM", "LR", "LS"):
            val = cmd[2:]
            if (code == "LM" and val != "N") or (code == "LR" and float(val or 0) != 0) \
                    or (code == "LS" and float(val or 1) != 1):
                raise GerberError(f"aperture transformation {cmd!r} is not supported")
        elif code == "SR":
            if re.search(r"X([2-9]|\d\d)|Y([2-9]|\d\d)", cmd):
                raise GerberError("step and repeat (SR) is not supported")
        elif code in ("OF", "SF", "MI", "AS"):
            if re.search(r"[AB]-?0*[1-9]|[AB]0*\.0*[1-9]", cmd) and code in ("OF", "SF", "MI"):
                raise GerberError(f"deprecated transformation {cmd!r} is not supported")
        # IP, IN, LN, IJ, IR ... are ignored

    @staticmethod
    def _split_attr(body: str) -> tuple[str, str]:
        body = body.lstrip(".")
        if "," in body:
            k, v = body.split(",", 1)
            return k.strip(), v.strip()
        return body.strip(), ""

    def _define_aperture(self, cmd: str) -> None:
        m = _AD_RE.match(cmd)
        if not m:
            raise GerberError(f"bad aperture definition {cmd!r}")
        code = int(m.group(1))
        template = m.group(2)
        mods = [float(v) * self.scale for v in m.group(3).split("X")] if m.group(3) else []
        macro = None
        if template not in ("C", "R", "O", "P"):
            macro = self.gf.macros.get(template)
            if macro is None:
                raise GerberError(f"aperture D{code} uses unknown macro {template!r}")
            # macro parameters are not lengths in every position; KiCad only
            # uses mm so we simply do not scale them
            mods = [float(v) for v in m.group(3).split("X")] if m.group(3) else []
        self.gf.apertures[code] = Aperture(code, template, mods, dict(self.cur_ta), macro)

    # -- data blocks ----------------------------------------------------------
    def _coord(self, raw: str) -> float:
        neg = raw.startswith("-")
        digits = raw.lstrip("+-")
        if "." in digits:
            val = float(digits)
        else:
            if self.trailing_zero_omission:
                digits = digits.ljust(self.int_digits + self.dec_digits, "0")
            val = int(digits or "0") / (10 ** self.dec_digits)
        return (-val if neg else val) * self.scale

    def _data(self, block: str) -> None:
        if not block:
            return
        if block.startswith("G04") or block.startswith("G4 "):
            return
        if block in ("M02", "M00", "M2", "M0"):
            self.done = True
            return
        x, y, i, j = None, None, 0.0, 0.0
        op = None
        for letter, num in _TOKEN_RE.findall(block):
            if letter == "G":
                g = int(num or 0)
                if g in (1, 2, 3):
                    self.interp = g
                elif g == 36:
                    self._begin_region()
                elif g == 37:
                    self._end_region()
                elif g == 74:
                    self.multi_quadrant = False
                elif g == 75:
                    self.multi_quadrant = True
                elif g == 70:
                    self.scale = 25.4
                elif g == 71:
                    self.scale = 1.0
                elif g == 91:
                    self.incremental = True
                elif g == 90:
                    self.incremental = False
                # G54/G55 are harmless prefixes
            elif letter == "X":
                x = self._coord(num)
            elif letter == "Y":
                y = self._coord(num)
            elif letter == "I":
                i = self._coord(num)
            elif letter == "J":
                j = self._coord(num)
            elif letter == "D":
                d = int(num or 0)
                if d >= 10:
                    ap = self.gf.apertures.get(d)
                    if ap is None:
                        raise GerberError(f"undefined aperture D{d}")
                    self.aperture = ap
                else:
                    op = d
            elif letter == "M":
                if int(num or 0) in (0, 2):
                    self.done = True
                    return
        if op is None:
            if x is not None or y is not None:
                # bare coordinates: deprecated, means repeat last op (D01)
                op = 1
            else:
                return
        if x is None:
            nx = self.x
        else:
            nx = self.x + x if self.incremental else x
        if y is None:
            ny = self.y
        else:
            ny = self.y + y if self.incremental else y
        if op == 2:
            if self.in_region:
                self._close_contour()
            self.x, self.y = nx, ny
        elif op == 1:
            seg = self._segment(nx, ny, i, j)
            if self.in_region:
                if not self.contours:
                    self.contours.append([])
                self.contours[-1].append(seg)
            else:
                if self.aperture is None:
                    raise GerberError("D01 without a selected aperture")
                self.gf.objects.append(Stroke(seg, self.aperture, dict(self.cur_to), self.dark))
            self.x, self.y = nx, ny
        elif op == 3:
            if self.aperture is None:
                raise GerberError("D03 without a selected aperture")
            self.x, self.y = nx, ny
            self.gf.objects.append(Flash(nx, ny, self.aperture, dict(self.cur_to), self.dark))

    def _segment(self, nx: float, ny: float, i: float, j: float) -> Segment:
        if self.interp == 1:
            return Segment(self.x, self.y, nx, ny)
        cw = self.interp == 2
        if self.multi_quadrant:
            cx, cy = self.x + i, self.y + j
        else:
            cx, cy = self._single_quadrant_centre(nx, ny, abs(i), abs(j), cw)
        return Segment(self.x, self.y, nx, ny, True, cx, cy, cw)

    def _single_quadrant_centre(self, nx: float, ny: float, i: float, j: float,
                                cw: bool) -> tuple[float, float]:
        best, best_err = (self.x + i, self.y + j), float("inf")
        for sx in (1, -1):
            for sy in (1, -1):
                cx, cy = self.x + sx * i, self.y + sy * j
                r0 = math.hypot(self.x - cx, self.y - cy)
                r1 = math.hypot(nx - cx, ny - cy)
                sweep = arc_sweep(self.x, self.y, nx, ny, cx, cy, cw)
                if abs(sweep) > math.pi / 2 + 1e-6:
                    continue
                err = abs(r0 - r1)
                if err < best_err:
                    best, best_err = (cx, cy), err
        return best

    def _begin_region(self) -> None:
        self.in_region = True
        self.contours = []
        self.region_attrs = dict(self.cur_to)

    def _close_contour(self) -> None:
        if self.contours and self.contours[-1]:
            self.contours.append([])

    def _end_region(self) -> None:
        contours = [c for c in self.contours if len(c) >= 2]
        if contours:
            self.gf.objects.append(Region(contours, self.region_attrs, self.dark))
        self.in_region = False
        self.contours = []


def parse_gerber(text: str, name: str = "") -> GerberFile:
    return GerberParser().parse(text, name)
