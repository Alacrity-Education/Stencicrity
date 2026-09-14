"""Command line front-end: gerbers in, one merged stencil order out.

Run it from the folder that holds the KiCad gerber zips::

    python3 stencicrity.py

It discovers the projects, loads ``./<name>.stencil`` (the project's
configuration: stencil size, layout, the rules that give fresh pads their
default state, which sides to place, what to do with copper pads without paste
and which paste openings to close), applies the options given on the command
line, shows a preview and a curses TUI, saves the configuration again and writes the
``F_Paste`` / ``F_Cu`` gerbers, a preview PNG, a report and a zip.

Precedence: an explicitly given command line option wins over the ``.stencil``
file, which wins over the built-in default.  Options that can come from the
file default to ``None`` ("not given") instead of a value.
"""
from __future__ import annotations

import argparse
import os
import sys
import zipfile
from typing import Optional, Sequence

from . import __version__
from .gerber import GerberError, polygons_of
from .model import (
    ORIENTATION_LANDSCAPE,
    ORIENTATION_PORTRAIT,
    SIDE_BOTTOM,
    SIDE_TOP,
    SORT_ORDERS,
    STATE_IGNORE,
    STATE_OPEN,
    STATE_UNDEFINED,
    STENCIL_SIZES,
    Config,
    Layout,
    Pad,
    Project,
    Side,
    parse_size,
    size_label,
)

DEFAULT_OUT = "./stencil-out"
DEFAULT_NAME = "stencil"


# --------------------------------------------------------------------------- #
# Argument parsing
# --------------------------------------------------------------------------- #
def build_parser() -> argparse.ArgumentParser:
    """The full ``stencicrity`` command line."""
    parser = argparse.ArgumentParser(
        prog="stencicrity",
        description="Merge KiCad paste gerbers of several projects into one "
                    "stencil order.",
        epilog="Options marked 'from the .stencil file' keep the value stored "
               "in the configuration file when they are not given; when they "
               "are given they override it and are saved back.",
    )
    parser.add_argument(
        "inputs", nargs="*",
        help="zip files or gerber directories "
             "(default: every *.zip and gerber subdirectory of the current folder)")
    parser.add_argument("--out", default=DEFAULT_OUT, metavar="DIR",
                        help="output directory (default: %(default)s)")
    parser.add_argument("--name", default=DEFAULT_NAME, metavar="NAME",
                        help="base name of the generated files (default: %(default)s)")
    parser.add_argument("--config", default=None, metavar="FILE",
                        help="configuration and pad decision file "
                             "(default: ./<NAME>.stencil)")
    # Old name of --config, kept working but no longer advertised.
    parser.add_argument("--overrides", dest="config", default=None,
                        metavar="FILE", help=argparse.SUPPRESS)

    group = parser.add_argument_group("stencil sheet")
    group.add_argument("--size", default=None,
                       choices=[size_label(s) for s in STENCIL_SIZES],
                       help="stencil sheet size in mm (from the .stencil file, "
                            "else 380x280)")
    orientation = group.add_mutually_exclusive_group()
    orientation.add_argument("--landscape", dest="orientation",
                             action="store_const", const=ORIENTATION_LANDSCAPE,
                             default=None, help="long side horizontal")
    orientation.add_argument("--portrait", dest="orientation",
                             action="store_const", const=ORIENTATION_PORTRAIT,
                             help="long side vertical")

    group = parser.add_argument_group("layout (mm; from the .stencil file)")
    group.add_argument("--gap", type=float, default=None, metavar="MM",
                       help="spacing between neighbouring boards; the dotted "
                            "border runs in the middle of it (default 30)")
    holes = group.add_mutually_exclusive_group()
    holes.add_argument("--holes", dest="holes", action="store_const",
                       const=True, default=None,
                       help="cut dowel pin holes near every cell corner (default)")
    holes.add_argument("--no-holes", dest="holes", action="store_const",
                       const=False, help="do not cut dowel pin holes")
    group.add_argument("--hole-dia", type=float, default=None, metavar="MM",
                       help="dowel pin hole diameter (default 5)")
    group.add_argument("--hole-inset", type=float, default=None, metavar="MM",
                       help="dotted line to hole edge; negative puts the hole "
                            "onto the line (default 2)")
    group.add_argument("--dot-dia", type=float, default=None, metavar="MM",
                       help="divider dot diameter (default 0.5)")
    group.add_argument("--dot-pitch", type=float, default=None, metavar="MM",
                       help="divider dot spacing (default 3)")
    group.add_argument("--dot-line-gap", type=float, default=None, metavar="MM",
                       help="distance between the two dotted lines of a cell "
                            "edge; 0 draws a single line on the edge "
                            "(default 2.5)")
    group.add_argument("--hole-grid", type=float, default=None, metavar="MM",
                       help="put every dowel hole on one common grid of this "
                            "pitch; cells grow until they fit it, 0 switches it "
                            "off (default 8)")
    border = group.add_mutually_exclusive_group()
    border.add_argument("--outer-border", dest="outer_border",
                        action="store_const", const=True, default=None,
                        help="also dot the cell edges on the outer boundary of the block")
    border.add_argument("--no-outer-border", dest="outer_border",
                        action="store_const", const=False,
                        help="dot only the shared edges (default)")
    group.add_argument("--sort", choices=SORT_ORDERS, default=None,
                       help="order the cells are packed in: 'height' offers "
                            "the tallest boards first (tighter packing), "
                            "'name' keeps projects alphabetical; top always "
                            "comes before bottom of the same board "
                            "(default height)")

    group = parser.add_argument_group("pads")
    group.add_argument("--no-mirror-bottom", action="store_true",
                       help="do not mirror bottom sides (they normally are)")
    group.add_argument("--include-tht", action="store_true",
                       help="also offer through hole pads as candidates")
    group.add_argument("--open-shrink", type=float, default=0.0, metavar="MM",
                       help="shrink openings cut for copper pads by this much "
                            "(default: %(default)s)")
    group.add_argument("--ignore-prefix", action="append", default=None,
                       metavar="PREFIX",
                       help="reference prefix whose pads without paste default "
                            "to 'ignore' (prefix + digit, e.g. TP3); "
                            "repeatable, replaces the stored list and is saved "
                            "to the .stencil file; pass an empty string to "
                            "clear it (from the file, else NT TP)")

    group = parser.add_argument_group("selection")
    group.add_argument("--exclude", action="append", default=[],
                       metavar="PROJECT[:top|bottom]",
                       help="switch a project (or one of its sides) off; "
                            "repeatable, saved to the .stencil file")
    group.add_argument("--only", action="append", default=[],
                       metavar="PROJECT[:top|bottom]",
                       help="switch everything else off; repeatable, saved to "
                            "the .stencil file")

    group = parser.add_argument_group("output")
    group.add_argument("--px-per-mm", type=float, default=20.0,
                       help="preview resolution (default: %(default)s)")
    group.add_argument("--no-open", action="store_true",
                       help="do not open the preview in an image viewer")
    group.add_argument("--batch", action="store_true",
                       help="no TUI: undefined pads are left closed")
    group.add_argument("--outline", action="store_true",
                       help="also write the stencil rectangle as Edge_Cuts")
    group.add_argument("--no-copper", action="store_true",
                       help="do not write the F_Cu reference layer")
    parser.add_argument("--version", action="version",
                        version=f"stencicrity {__version__}")
    return parser


def _check_args(parser: argparse.ArgumentParser, args: argparse.Namespace) -> None:
    """Reject impossible numbers before anything is loaded."""
    for option, value, ok, requirement in (
            ("--gap", args.gap, lambda v: v >= 0, "must not be negative"),
            ("--hole-dia", args.hole_dia, lambda v: v > 0, "must be positive"),
            ("--dot-dia", args.dot_dia, lambda v: v > 0, "must be positive"),
            ("--dot-pitch", args.dot_pitch, lambda v: v > 0, "must be positive"),
            ("--dot-line-gap", args.dot_line_gap, lambda v: v >= 0,
             "must not be negative"),
            ("--hole-grid", args.hole_grid, lambda v: v >= 0,
             "must not be negative"),
            ("--px-per-mm", args.px_per_mm, lambda v: v > 0, "must be positive"),
            ("--open-shrink", args.open_shrink, lambda v: v >= 0,
             "must not be negative")):
        if value is not None and not ok(value):
            parser.error(f"{option} {requirement}")


def apply_cli_config(config: Config, args: argparse.Namespace) -> None:
    """Overwrite the stored configuration with the options actually given."""
    from .config import parse_prefixes

    if args.size is not None:
        config.size = parse_size(args.size)
    if args.ignore_prefix is not None:
        prefixes: list[str] = []
        for spec in args.ignore_prefix:
            for prefix in parse_prefixes(spec):
                if prefix not in prefixes:
                    prefixes.append(prefix)
        config.ignore_prefixes = tuple(prefixes)
    if args.orientation is not None:
        config.orientation = args.orientation
    params = config.layout
    for attr, value in (("gap", args.gap),
                        ("holes", args.holes),
                        ("hole_dia", args.hole_dia),
                        ("hole_inset", args.hole_inset),
                        ("dot_dia", args.dot_dia),
                        ("dot_pitch", args.dot_pitch),
                        ("dot_line_gap", args.dot_line_gap),
                        ("hole_grid", args.hole_grid),
                        ("outer_border", args.outer_border),
                        ("sort", args.sort)):
        if value is not None:
            setattr(params, attr, value)


# --------------------------------------------------------------------------- #
# Project / side selection
# --------------------------------------------------------------------------- #
def parse_filter(spec: str) -> tuple[str, Optional[str]]:
    """``"board:bottom"`` -> ``("board", "bottom")``; side is optional."""
    name, sep, side = spec.rpartition(":")
    if sep and side.strip().lower() in (SIDE_TOP, SIDE_BOTTOM):
        return name.strip().lower(), side.strip().lower()
    return spec.strip().lower(), None


def filter_matches(side: Side, spec: tuple[str, Optional[str]]) -> bool:
    """True when a ``--only`` / ``--exclude`` spec selects this side."""
    name, want_side = spec
    project = side.project.name.lower()
    if name and name != project and name not in project:
        return False
    return want_side is None or want_side == side.name


def apply_selection(projects: Sequence[Project], only: Sequence[str],
                    exclude: Sequence[str]) -> bool:
    """Switch sides on/off from ``--only`` / ``--exclude``; True when used."""
    only_specs = [parse_filter(s) for s in only]
    exclude_specs = [parse_filter(s) for s in exclude]
    if not only_specs and not exclude_specs:
        return False
    for project in projects:
        for side in project.sides():
            if only_specs:
                side.enabled = any(filter_matches(side, f) for f in only_specs)
            if side.enabled and any(filter_matches(side, f) for f in exclude_specs):
                side.enabled = False
    return True


def side_table(sides: Sequence[Side]) -> list[str]:
    """Rendered overview table of every relevant side."""
    head = (f"{'project':<24} {'side':<7} {'use':<4} {'mir':<4} "
            f"{'board (mm)':>16} {'paste':>7} {'closed':>7} {'cand':>6}")
    lines = [head, "-" * len(head)]
    for side in sides:
        project = side.project
        size = f"{project.width:.1f} x {project.height:.1f}"
        lines.append(f"{project.name[:24]:<24} {side.name:<7} "
                     f"{'on' if side.enabled else 'off':<4} "
                     f"{'yes' if side.mirror else 'no':<4} {size:>16} "
                     f"{len(side.paste_objects):>7} "
                     f"{len(side.closed_pads):>7} {len(side.candidates):>6}")
    return lines


# --------------------------------------------------------------------------- #
# Generation
# --------------------------------------------------------------------------- #
def _pad_counts(side: Side) -> dict[str, int]:
    counts = {STATE_UNDEFINED: 0, STATE_OPEN: 0, STATE_IGNORE: 0}
    for pad in side.candidates:
        if pad.state in counts:
            counts[pad.state] += 1
    return counts


def _write_paste(layout: Layout, path: str, name: str, open_shrink: float) -> str:
    """Write the paste layer: source openings, decided pads, dots and holes.

    Openings of pads the user closed are left out (``side.active_paste_objects``).
    """
    from shapely.affinity import affine_transform

    from .writer import GerberWriter

    writer = GerberWriter("Paste,Top")
    writer.comment(f"stencil {name} - merged paste layer")
    for area in layout.areas:
        side = area.side
        suffix = " (mirrored)" if area.transform.mirror else ""
        writer.comment(f"--- {side.label}{suffix} ---")
        for obj in side.active_paste_objects:
            writer.add_object(obj, area.transform)
        for pad in side.open_pads:
            if open_shrink <= 0:
                writer.add_object(pad.flash, area.transform)
                continue
            poly = pad.geom.buffer(-open_shrink, join_style="mitre")
            if poly.is_empty:
                continue
            for part in polygons_of(affine_transform(poly, area.transform.affine())):
                writer.add_polygon(list(part.exterior.coords))
    if layout.dots:
        writer.comment("--- border dots ---")
        for x, y in layout.dots:
            writer.add_circle(x, y, layout.params.dot_dia)
    if any(area.holes for area in layout.areas):
        writer.comment("--- dowel pin holes ---")
        for area in layout.areas:
            for x, y in area.holes:
                writer.add_circle(x, y, layout.params.hole_dia)
    return writer.write(path)


def _write_copper(layout: Layout, path: str, name: str) -> str:
    """Write the copper reference layer (not cut, only for checking)."""
    from .writer import GerberWriter

    writer = GerberWriter("Copper,L1,Top")
    writer.comment(f"stencil {name} - copper reference")
    for area in layout.areas:
        suffix = " (mirrored)" if area.transform.mirror else ""
        writer.comment(f"--- {area.side.label}{suffix} ---")
        for obj in area.side.copper_objects:
            writer.add_object(obj, area.transform)
    return writer.write(path)


def _write_outline(layout: Layout, path: str, name: str) -> str:
    """Write the stencil rectangle (0, 0) - (W, H) as a profile layer."""
    from .writer import GerberWriter

    writer = GerberWriter("Profile,NP")
    writer.comment(f"stencil {name} - sheet outline")
    writer.add_rect_outline(0.0, 0.0, layout.width, layout.height, 0.1)
    return writer.write(path)


def _zip_files(zip_path: str, paths: Sequence[str]) -> str:
    """Pack the written gerbers into one deflate zip."""
    with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED) as archive:
        for path in paths:
            archive.write(path, os.path.basename(path))
    return zip_path


def _fit_text(layout: Layout) -> str:
    return "fits" if layout.fits else "DOES NOT FIT"


def _summary(layout: Layout, config: Config, dropped: Sequence[Side],
             disabled: Sequence[Side], files: Sequence[str],
             config_path: str, undefined: int) -> list[str]:
    """Human readable summary shown on stdout and stored in the report."""
    lines = [f"stencil: {layout.width:.1f} x {layout.height:.1f} mm "
             f"({config.size_label}, {config.orientation})",
             f"block:   {layout.block_width:.1f} x {layout.block_height:.1f} mm, "
             f"{len(layout.areas)} area(s) — {_fit_text(layout)}"]
    for area in layout.areas:
        counts = _pad_counts(area.side)
        suffix = " (mirrored)" if area.transform.mirror else ""
        lines.append(
            f"  {area.side.label + suffix:<34} at ({area.x:7.2f}, {area.y:7.2f}) "
            f"{area.w:6.1f} x {area.h:6.1f}  "
            f"paste={len(area.side.active_paste_objects):<5} "
            f"closed={len(area.side.closed_pads):<4} "
            f"open={counts[STATE_OPEN]:<4} ignore={counts[STATE_IGNORE]:<4} "
            f"undefined={counts[STATE_UNDEFINED]}")
    for side in dropped:
        lines.append(f"  dropped (no openings): {side.label}")
    for side in disabled:
        lines.append(f"  switched off: {side.label}")
    lines.append(f"configuration: {config_path}")
    lines.append("files:")
    lines.extend(f"  {path}" for path in files)
    if undefined:
        lines.append(f"note: {undefined} pad(s) are still undefined and got NO opening")
    return lines


# --------------------------------------------------------------------------- #
# Main flow
# --------------------------------------------------------------------------- #
def _run(args: argparse.Namespace) -> int:
    """The whole pipeline; raises GerberError/OSError on user level problems."""
    # Imported here so --help/--version work while the package is incomplete.
    from .config import apply_config, load_config, save_config
    from .layout import layout_report, pack
    from .pads import (closed_count, detect_pads, sorted_candidates,
                       sorted_pads, state_counts)
    from .project import discover_projects
    from .render import open_file, render_preview
    from .tui import run_tui

    cwd = os.getcwd()
    out_dir = os.path.abspath(args.out)
    name = args.name
    config_path = args.config or os.path.join(cwd, f"{name}.stencil")

    def warn(message: str) -> None:
        """Print a warning to stderr, keeping it in order with stdout."""
        sys.stdout.flush()
        print(f"warning: {message}", file=sys.stderr)
        sys.stderr.flush()

    # 1. discover ---------------------------------------------------------- #
    projects = discover_projects(args.inputs or None, cwd,
                                 mirror_bottom=not args.no_mirror_bottom,
                                 exclude_dirs=[out_dir], warn=warn)
    if not projects:
        print("error: no gerber projects found (pass zip files or directories)",
              file=sys.stderr)
        return 2
    print(f"{len(projects)} project(s) found")

    # 2. configuration ------------------------------------------------------ #
    # The [rules] section decides the default state of a pad nobody decided on
    # yet, so the configuration has to be read before the pads are detected.
    existed = os.path.exists(config_path)
    config = load_config(config_path, warn=warn)
    apply_cli_config(config, args)
    for project in projects:
        for side in project.sides():
            detect_pads(side, include_tht=args.include_tht,
                        ignore_prefixes=config.ignore_prefixes)
    apply_config(config, projects)
    apply_selection(projects, args.only, args.exclude)
    save_config(config_path, config, projects)
    if existed:
        print(f"configuration: {config_path} "
              f"({len(config.sides)} side switch(es), "
              f"{len(config.pads)} pad decision(s))")
    else:
        print(f"configuration: {config_path} (new)")

    # 3. the sides we can place --------------------------------------------- #
    sides = [side for project in projects for side in project.sides()
             if side.is_relevant()]
    if not sides:
        print("error: no side has paste openings or pads to decide on",
              file=sys.stderr)
        return 2
    print()
    for line in side_table(sides):
        print(line)
    print()

    # 4. first preview ------------------------------------------------------ #
    os.makedirs(out_dir, exist_ok=True)
    preview_path = os.path.join(out_dir, f"{name}-preview.png")
    layout = pack(sides, config)
    print(f"stencil: {layout.width:.1f} x {layout.height:.1f} mm "
          f"({config.size_label}, {config.orientation})")
    print(f"block:   {layout.block_width:.1f} x {layout.block_height:.1f} mm "
          f"— {_fit_text(layout)}")

    def compute_layout(cfg: Optional[Config] = None) -> Layout:
        """Layout of the currently enabled sides (for the TUI)."""
        return pack(sides, cfg or config)

    def on_preview(pad: Optional[Pad] = None, do_open: bool = False) -> str:
        """Re-pack with the live configuration and re-render the preview."""
        live = pack(sides, config)
        if not live.areas:
            return preview_path
        path = render_preview(live, preview_path, px_per_mm=args.px_per_mm,
                              selected=pad, title=name)
        if do_open:
            open_file(path)
        return path

    if layout.areas:
        render_preview(layout, preview_path, px_per_mm=args.px_per_mm, title=name)
        print(f"preview: {preview_path}")
        if not args.no_open:
            open_file(preview_path)
    else:
        warn("every side is switched off, nothing to preview")

    # 5. pad decisions -------------------------------------------------------- #
    all_pads = sorted_pads(projects, sides, all_pads=True)
    candidates = [pad for pad in all_pads if pad.is_candidate]
    counts = state_counts(candidates)
    print(f"candidate pads (copper without paste): {len(candidates)} "
          f"(undefined {counts[STATE_UNDEFINED]}, open {counts[STATE_OPEN]}, "
          f"ignore {counts[STATE_IGNORE]})")
    print(f"closed openings: {closed_count(all_pads)}")

    # 6. the TUI ------------------------------------------------------------- #
    if not args.batch:
        # The TUI gets every pad: it shows the candidates by default and the
        # pads that already have paste on demand, so they can be closed.
        title = f"{name}: {len(candidates)} pad(s) without paste"
        confirmed = run_tui(all_pads, sides, config, on_preview=on_preview,
                            compute_layout=compute_layout, title=title)
        save_config(config_path, config, projects)
        if not confirmed:
            print(f"aborted — configuration saved to {config_path}, "
                  f"nothing generated")
            return 1
    else:
        undefined = state_counts(candidates)[STATE_UNDEFINED]
        if undefined:
            warn(f"--batch: {undefined} undefined pad(s) are treated as closed "
                 f"(edit {config_path} to change them)")
        if not layout.fits:
            warn(f"block {layout.block_width:.1f} x {layout.block_height:.1f} mm "
                 f"does not fit the {layout.width:.0f} x {layout.height:.0f} mm "
                 f"stencil")

    # 7. generate ------------------------------------------------------------ #
    final_sides = [side for side in sides if side.enabled and side.has_openings()]
    dropped = [side for side in sides if side.enabled and not side.has_openings()]
    disabled = [side for side in sides if not side.enabled]
    for side in dropped:
        print(f"dropped {side.label}: no openings at all")
    if not final_sides:
        print("error: no enabled side has a single opening, nothing to generate",
              file=sys.stderr)
        return 2
    layout = pack(final_sides, config)
    if not layout.fits:
        warn(f"the block of boards ({layout.block_width:.1f} x "
             f"{layout.block_height:.1f} mm) does not fit the "
             f"{layout.width:.0f} x {layout.height:.0f} mm stencil — "
             f"use a larger size, a smaller spacing or fewer boards")

    gerbers = [_write_paste(layout, os.path.join(out_dir, f"{name}-F_Paste.gbr"),
                            name, args.open_shrink)]
    if not args.no_copper:
        gerbers.append(_write_copper(layout, os.path.join(out_dir, f"{name}-F_Cu.gbr"),
                                     name))
    if args.outline:
        gerbers.append(_write_outline(
            layout, os.path.join(out_dir, f"{name}-Edge_Cuts.gbr"), name))
    zip_path = _zip_files(os.path.join(out_dir, f"{name}.zip"), gerbers)

    render_preview(layout, preview_path, px_per_mm=args.px_per_mm, title=name)

    remaining = state_counts(sorted_candidates(projects, final_sides))[STATE_UNDEFINED]
    files = list(gerbers) + [zip_path, preview_path]
    report_path = os.path.join(out_dir, f"{name}-report.txt")
    summary = _summary(layout, config, dropped, disabled, files + [report_path],
                       config_path, remaining)
    with open(report_path, "w", encoding="utf-8") as handle:
        handle.write(layout_report(layout, config).rstrip("\n") + "\n\n")
        handle.write("\n".join(summary) + "\n")

    print()
    print("\n".join(summary))
    return 0


def main(argv: Optional[list[str]] = None) -> int:
    """Entry point: parse the command line and run the pipeline."""
    parser = build_parser()
    args = parser.parse_args(argv)
    _check_args(parser, args)
    try:
        return _run(args)
    except (GerberError, OSError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2
    except KeyboardInterrupt:
        print("interrupted", file=sys.stderr)
        return 1
