"""RS-274X (Gerber X2) writer.

Emits KiCad-like output: a fixed X2 header, an aperture list (macros and
aperture definitions with their attributes) and a body of graphic objects.
Apertures are registered lazily on first use and deduplicated by
:meth:`~pcbstencil.gerber.BakedAperture.key`, so the body is buffered and the
whole file is assembled in :meth:`GerberWriter.render`.

Coordinates are written in the ``FSLAX46Y46`` / ``MOMM`` format: millimetres
scaled by 1e6 and rounded to integers.
"""
from __future__ import annotations

from datetime import datetime
from typing import Optional

from .gerber import (
    BakedAperture,
    Flash,
    GraphicObject,
    Region,
    Segment,
    Stroke,
    bake_aperture,
)
from .model import Transform

_SCALE = 1_000_000          # file units per mm (4.6 format)
_LINEAR = 1                 # G01
_CW = 2                     # G02
_CCW = 3                    # G03


def _nm(value: float) -> int:
    """Millimetres to file units (integer nanometres)."""
    return int(round(value * _SCALE))


class GerberWriter:
    """Builds one Gerber file from graphic objects and simple primitives."""

    def __init__(self, file_function: str, *, polarity: str = "Positive",
                 software: str = "pcbstencil", version: str = "0.1.0") -> None:
        """Create a writer for a layer with the given ``%TF.FileFunction``."""
        self.file_function = file_function
        self.polarity = polarity
        self.software = software
        self.version = version
        self.created = datetime.now().astimezone().isoformat(timespec="seconds")

        # Aperture list (assembled before the body in render()).
        self._macro_lines: list[str] = []
        self._macro_names: dict[str, str] = {}      # macro body -> macro name
        self._aperture_lines: list[str] = []
        self._aperture_codes: dict[str, int] = {}   # BakedAperture.key() -> D-code
        self._next_code = 10
        self._next_macro = 1

        # Body and graphics state.
        self._body: list[str] = []
        self._aperture: Optional[int] = None        # selected D-code
        self._x: Optional[int] = None               # current point, file units
        self._y: Optional[int] = None
        self._interp = _LINEAR                      # interpolation mode (G01/G02/G03)
        self._multi_quadrant = False                # True once an arc needs G75
        self._dark = True                           # current polarity (%LPD*%)

    # ----------------------------------------------------------------- state
    def _register(self, baked: BakedAperture) -> int:
        """Return the D-code for ``baked``, defining it on first use."""
        key = baked.key()
        code = self._aperture_codes.get(key)
        if code is not None:
            return code
        code = self._next_code
        self._next_code += 1
        self._aperture_codes[key] = code

        if baked.template == "MACRO":
            name = self._macro(baked)
            definition = f"%ADD{code}{name}*%"
        elif baked.modifiers:
            mods = "X".join(f"{m:.6f}" for m in baked.modifiers)
            definition = f"%ADD{code}{baked.template},{mods}*%"
        else:
            definition = f"%ADD{code}{baked.template}*%"

        for attr, value in baked.attrs.items():
            self._aperture_lines.append(
                f"%TA.{attr},{value}*%" if value else f"%TA.{attr}*%")
        self._aperture_lines.append(definition)
        if baked.attrs:
            self._aperture_lines.append("%TD*%")
        return code

    def _macro(self, baked: BakedAperture) -> str:
        """Return the macro name for a baked macro aperture, defining it once."""
        prims = [p.as_gerber() for p in baked.prims] or ["0 empty*"]
        body = "\n".join(prims)
        name = self._macro_names.get(body)
        if name is None:
            name = f"M{self._next_macro}"
            self._next_macro += 1
            self._macro_names[body] = name
            self._macro_lines.append(f"%AM{name}*\n{body}\n%")
        return name

    def _select(self, code: int) -> None:
        """Emit a D-code selection when the aperture changes."""
        if self._aperture != code:
            self._body.append(f"D{code}*")
            self._aperture = code

    def _set_polarity(self, dark: bool) -> None:
        """Emit ``%LPD*%`` / ``%LPC*%`` when the polarity changes."""
        if dark != self._dark:
            self._body.append("%LPD*%" if dark else "%LPC*%")
            self._dark = dark

    def _set_interp(self, mode: int) -> None:
        """Emit ``G01*`` / ``G02*`` / ``G03*`` when the interpolation changes."""
        if self._interp != mode:
            self._body.append(f"G0{mode}*")
            self._interp = mode

    def _move(self, x: int, y: int, force: bool = False) -> None:
        """Emit a ``D02`` move when the current point differs (or when forced)."""
        if force or self._x != x or self._y != y:
            self._body.append(f"X{x}Y{y}D02*")
            self._x, self._y = x, y

    def _segment(self, seg: Segment, tr: Transform) -> None:
        """Emit one ``D01`` interpolation for a segment under ``tr``."""
        x0, y0 = (_nm(v) for v in tr.apply(seg.x0, seg.y0))
        x1, y1 = (_nm(v) for v in tr.apply(seg.x1, seg.y1))
        if seg.arc:
            # Mirroring about the Y axis reverses the direction of travel.
            clockwise = seg.clockwise != bool(tr.mirror)
            cx, cy = (_nm(v) for v in tr.apply(seg.cx, seg.cy))
            self._multi_quadrant = True
            self._set_interp(_CW if clockwise else _CCW)
            self._body.append(f"X{x1}Y{y1}I{cx - x0}J{cy - y0}D01*")
        else:
            self._set_interp(_LINEAR)
            self._body.append(f"X{x1}Y{y1}D01*")
        self._x, self._y = x1, y1

    # ---------------------------------------------------------------- public
    def comment(self, text: str) -> None:
        """Append a ``G04`` comment line."""
        clean = (text.replace("\r", " ").replace("\n", " ")
                 .replace("*", "").replace("%", "").strip())
        self._body.append(f"G04 {clean}*")

    def add_object(self, obj: GraphicObject, tr: Transform) -> None:
        """Write a parsed graphic object transformed from board to sheet coords."""
        if isinstance(obj, Flash):
            self._add_flash(obj, tr)
        elif isinstance(obj, Stroke):
            self._add_stroke(obj, tr)
        elif isinstance(obj, Region):
            self._add_region(obj, tr)
        else:
            raise TypeError(f"cannot write object {obj!r}")

    def _add_flash(self, obj: Flash, tr: Transform) -> None:
        code = self._register(bake_aperture(obj.aperture, tr.mirror))
        self._set_polarity(obj.dark)
        self._select(code)
        x, y = (_nm(v) for v in tr.apply(obj.x, obj.y))
        self._body.append(f"X{x}Y{y}D03*")
        self._x, self._y = x, y

    def _add_stroke(self, obj: Stroke, tr: Transform) -> None:
        code = self._register(bake_aperture(obj.aperture, tr.mirror))
        self._set_polarity(obj.dark)
        self._select(code)
        x0, y0 = (_nm(v) for v in tr.apply(obj.seg.x0, obj.seg.y0))
        self._move(x0, y0)
        self._segment(obj.seg, tr)

    def _add_region(self, obj: Region, tr: Transform) -> None:
        contours = [c for c in obj.contours if c]
        if not contours:
            return
        self._set_polarity(obj.dark)
        self._body.append("G36*")
        for contour in contours:
            first = contour[0]
            x0, y0 = (_nm(v) for v in tr.apply(first.x0, first.y0))
            self._move(x0, y0, force=True)
            for seg in contour:
                self._segment(seg, tr)
        self._body.append("G37*")

    def add_circle(self, x: float, y: float, dia: float,
                   attrs: dict | None = None) -> None:
        """Flash a round aperture of diameter ``dia`` at sheet coordinates."""
        baked = BakedAperture("C", [float(dia)], [], dict(attrs or {}))
        code = self._register(baked)
        self._set_polarity(True)
        self._select(code)
        ix, iy = _nm(x), _nm(y)
        self._body.append(f"X{ix}Y{iy}D03*")
        self._x, self._y = ix, iy

    def add_polygon(self, points: list[tuple[float, float]]) -> None:
        """Fill a polygon (``G36``/``G37``) given in sheet coordinates."""
        pts: list[tuple[int, int]] = []
        for x, y in points:
            p = (_nm(x), _nm(y))
            if not pts or pts[-1] != p:
                pts.append(p)
        if len(pts) > 1 and pts[0] == pts[-1]:
            pts.pop()
        if len(pts) < 3:
            return
        self._set_polarity(True)
        self._body.append("G36*")
        self._move(*pts[0], force=True)
        self._set_interp(_LINEAR)
        for px, py in pts[1:] + [pts[0]]:
            self._body.append(f"X{px}Y{py}D01*")
            self._x, self._y = px, py
        self._body.append("G37*")

    def add_line(self, x0: float, y0: float, x1: float, y1: float,
                 width: float) -> None:
        """Stroke a straight line of the given width with a round aperture."""
        code = self._register(BakedAperture("C", [float(width)], [], {}))
        self._set_polarity(True)
        self._select(code)
        self._move(_nm(x0), _nm(y0))
        self._set_interp(_LINEAR)
        ix, iy = _nm(x1), _nm(y1)
        self._body.append(f"X{ix}Y{iy}D01*")
        self._x, self._y = ix, iy

    def add_rect_outline(self, x0: float, y0: float, x1: float, y1: float,
                         width: float) -> None:
        """Stroke the four sides of a rectangle."""
        self.add_line(x0, y0, x1, y0, width)
        self.add_line(x1, y0, x1, y1, width)
        self.add_line(x1, y1, x0, y1, width)
        self.add_line(x0, y1, x0, y0, width)

    def render(self) -> str:
        """Return the complete Gerber file text."""
        lines: list[str] = [
            f"%TF.GenerationSoftware,{self.software},pcbstencil,{self.version}*%",
            f"%TF.CreationDate,{self.created}*%",
            f"%TF.FileFunction,{self.file_function}*%",
            f"%TF.FilePolarity,{self.polarity}*%",
            "%FSLAX46Y46*%",
            "G04 Gerber Fmt 4.6, Leading zero omitted, Abs format (unit mm)*",
            f"G04 Created by pcbstencil {self.version}*",
            "%MOMM*%",
            "%LPD*%",
            "G01*",
            "G04 APERTURE LIST*",
        ]
        lines.extend(self._macro_lines)
        lines.extend(self._aperture_lines)
        lines.append("G04 APERTURE END LIST*")
        if self._multi_quadrant:
            lines.append("G75*")
        lines.extend(self._body)
        lines.append("M02*")
        return "\n".join(lines) + "\n"

    def write(self, path: str) -> str:
        """Write :meth:`render` to ``path`` (utf-8, LF newlines) and return it."""
        with open(path, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(self.render())
        return path
