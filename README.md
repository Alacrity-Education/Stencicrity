# Stencicrity

Stencicrity merges the solder-paste gerbers of several KiCad projects into one
stencil order. We make many small boards, and each of them would use a few
square centimetres of a 380 x 280 mm stencil sheet. So we put every board side
on one sheet of a standard size, copy all paste openings over, mark the border
of every board with dotted lines so the sheet can be cut apart, and add
dowel-pin holes on a fixed grid so every cut-out piece fits the same fixture
plate. The result is a single `F_Paste` gerber, packed in a zip with a copper
reference layer, that a stencil house cuts like any other.

One thing the tool cannot decide on its own: copper pads that have no paste
opening in the source (test points, jumper pads, connectors soldered by hand).
Those are collected and shown in a terminal UI where we decide, pad by pad,
whether they get an opening. The decisions live in a plain text file next to
the gerbers, so a second run over the same boards needs no interaction.

## Quick start

Put the KiCad gerber zips of all the projects in one folder and run the script
there:

    cd boards/
    python3 /path/to/stencicrity.py

A preview PNG opens in the image viewer and the TUI starts in the terminal.
Walk through the pads, press Enter, and `stencil-out/stencil.zip` is the order.
`pip install -e .` in this repository installs a `stencicrity` command so the
path is not needed; `uv run --with shapely --with pillow --with numpy
stencicrity.py` works as well.

## How a session goes

1. Discovery. Every `*.zip` in the current folder and every subfolder that
   holds gerbers becomes a project (paths can also be given explicitly).
   Layers are recognised by their X2 `%TF.FileFunction` header, falling back
   to KiCad file name patterns (`-F_Paste`, `-B_Cu`, `.gtp`, `.gbl`, ...). The
   project name comes from `%TF.ProjectId`, else from the zip or folder name
   with a `GERBER-` prefix stripped. Top and bottom each become one *side*.
   The board size comes from `Edge_Cuts`, else from the bounding box of the
   copper and paste layers. Our own output is never read back as input.
2. Configuration. `./stencil.stencil` is read if it exists, the command line
   options override it, the pads are detected, and the file is written back
   straight away so it exists even if the run is interrupted.
3. Preview. The sheet is laid out and rendered to
   `stencil-out/stencil-preview.png`, which opens in the desktop viewer.
4. The TUI. Four pages: Pads, Sides, Stencil, Layout. Every change re-packs
   the sheet; `p` re-renders the preview so the viewer shows the current state.
5. Generation. Enter on the Pads page or `g` on any other page writes the
   gerbers, the zip, the final preview and the report into `stencil-out/`.
   `q` leaves without generating; the configuration is saved anyway.

## The sheet

Every enabled side becomes a *cell*: the board's bounding box plus at least
half the spacing on each of its four sides, so two neighbouring boards end up
at least `spacing` (default 30 mm) apart; exactly that when the hole grid is
off. Cells are not rotated. They are placed with a MaxRects bin packer; three
heuristics (bottom-left, best short side fit, best area fit) are tried and the
one that places the most cells in the smallest block wins. The block is then
centred on the sheet. Cells are offered to the packer tallest board first
(`sort = height`) or alphabetically (`sort = name`); the top side of a board
always comes right before its bottom.

The border of every cell is marked with dots: openings of `dot_dia` (0.5 mm)
every `dot_pitch` (3 mm), evenly spaced along each edge. Each edge is marked by
two parallel dotted lines `dot_line_gap` (2.5 mm) apart, centred on the edge,
and the cut runs between them; `dot_line_gap = 0` gives a single line on the
edge itself. A shared edge gives one pair of lines, collinear pieces are merged
into one straight line, lines of different edges that land on the same
coordinate are merged again so nothing is dotted twice, and the outer boundary
of the whole block is left blank because nothing has to be cut apart there
(`outer_border = on` dots it too).

Every cell gets four dowel-pin holes (`hole_dia` 5 mm) inside its corners,
`hole_inset` (2 mm) from the cell edge (the centre line of the dotted border)
to the hole edge; a negative inset moves the hole onto the edge, and two cells
that would share a hole get one. With `hole_grid` set (default 8 mm) every hole
centre on the sheet lies on one common grid of that pitch, measured from the
sheet origin. This is for a fixture plate with pins on an 8 mm raster that
takes every cut-out piece without adjustment. To get there, each cell is grown
until its hole-to-hole distance is a multiple of the pitch (the board stays
centred, so the padding grows past half the spacing), the packer only puts
cell corners where the holes land on the grid, and the block is centred by a
whole number of pitches. Cells therefore do not always touch; the slivers
between them are free space. The report says whether every hole is on the grid
and how much cell area the grid cost. `hole_grid = 0` switches the grid off,
`holes = off` cuts no holes.

Bottom sides are placed mirrored (x to -x) so the piece matches the board once
the board is flipped over; `--no-mirror-bottom` places them as they are.
Sheet coordinates start at (0, 0) in the bottom left corner of the stencil, in
millimetres; the report and the rulers of the preview use them.

Sheet sizes: 380 x 280 (default), 420 x 320, 450 x 350, 460 x 460, 520 x 420,
600 x 600 and 700 x 600 mm, each in landscape (long side horizontal) or
portrait. The Stencil page shows for every size and both orientations whether
the current block fits. A cell that fits nowhere is parked to the right of the
sheet, the header says DOES NOT FIT, the preview grows to show it, and
generating asks for a second confirmation. The gerbers are written regardless.

## Deciding pads

Paste openings from the source gerbers are copied to the stencil as they are:
same aperture, same shape, same position. What needs a decision are the
copper pads the paste layer does not cover. A pad is a copper flash whose
`AperFunction` is one of the SMD functions (`SMDPad`, `BGAPad`, `HeatsinkPad`,
`FiducialPad`, `TestPad`, `ConnectorPad`); through-hole pads (`ComponentPad`,
`CastellatedPad`) only count with `--include-tht`. A pad is pasted when an
opening contains its centroid or the openings cover at least 10 % of its
area. The rest are *candidates*, each in one of three states:

| state | meaning |
| --- | --- |
| `undefined` | not decided yet; no opening is cut |
| `open` | an opening is cut with the copper pad's own aperture (`--open-shrink MM` shrinks it inward) |
| `ignore` | deliberately left closed |

Candidates start `undefined`, except those whose reference is one of the
`[rules] ignore_prefixes` followed by a digit (`TP3`, `NT12`; not `TPS1`). The
default list is `NT TP`, so net ties and test points start as `ignore` and do
not have to be waved through one by one.

The other direction works too: a pad that already has paste can be *closed*.
The Pads page lists the pasted pads on `*`; setting one to `ignore` drops the
paste openings that cover it from the stencil. We use that for a connector or
a shield that is soldered by hand on an otherwise finished board.

### The `.stencil` file

Everything a run needs is in `./stencil.stencil` (`--name` changes the stem,
`--config FILE` the path). It is written as soon as the projects are known and
again when the TUI exits, so it always reflects the last run, and it is meant
to be edited by hand. Options given on the command line override the file and
are saved back into it.

```ini
[stencil]
size = 380x280            # 380x280 | 420x320 | 450x350 | 460x460 | 520x420 | 600x600 | 700x600
orientation = landscape   # landscape (long side horizontal) | portrait

[layout]
spacing = 30.0            # mm between neighbouring boards; the dotted border runs in the middle
holes = on                # cut dowel pin holes near every cell corner
hole_dia = 5.0            # mm
hole_inset = 2.0          # mm from the dotted line to the hole edge (negative = onto the line)
dot_dia = 0.5             # mm
dot_pitch = 3.0           # mm
dot_line_gap = 2.5        # mm between the two dotted border lines (0 = single line)
hole_grid = 8.0           # dowel hole centres snap to this grid, cells grow to fit (0 = off)
outer_border = off        # also dot the cell edges on the outer boundary of the block
sort = height             # height (tallest boards first) | name

[rules]
ignore_prefixes = NT TP

[sides]
RBARF/top = on            # 13.4 x 11.6 mm, 50 paste openings, 0 pads to decide
RBARF/bottom = on         # 13.4 x 11.6 mm, 13 paste openings, 3 pads to decide

[pads]
# <project>/<side>/<REF>.<pin>@<x>,<y> = undefined | open | ignore
RBARF/bottom/TP1.1@148.082,-99.568 = open   # SMDPad C ⌀1.00
# --- pads with paste whose opening is closed (state ignore) ---
RBARF/top/D1.1@148.775,-96.120 = ignore   # SMDPad R 0.70x0.70
```

Pad keys use board coordinates with three decimals, so they stay stable as
long as the layout in KiCad does not move. Every candidate is listed with its
state; a pasted pad only appears when it was closed. Sections, keys and values
are case insensitive, `#` starts a comment, booleans accept `on/off`,
`yes/no`, `true/false` and `1/0`. A bad value is reported and the default is
kept; the run never stops because of the file. Entries that no longer match
the current gerbers are kept at the end of their section under a "not found"
comment instead of being thrown away. Changing `ignore_prefixes` only affects
pads the file does not know yet; delete their lines to re-apply the rule.

## The TUI

Four pages, switched with `1`-`4`, `Tab` and `Shift-Tab`. The session starts
on Pads when there is anything to decide, else on Sides. The header always
shows the sheet size, the block size with FITS or DOES NOT FIT, and the counts
of `undefined`, `open` and `ignore` candidates plus `closed` openings. The
footer lists the keys of the current page.

| page | what happens there |
| --- | --- |
| Pads | the candidates of the enabled sides: state, reference and pin, project, side, aperture function, shape, position; `*` adds the pasted pads (dimmed, marked with a `·`) so they can be closed |
| Sides | switch board sides on and off; each row shows size, paste count and pads to decide |
| Stencil | pick the sheet size; every row shows whether the block fits in landscape and in portrait |
| Layout | spacing, holes, hole diameter and inset, dot diameter and pitch, dotted line gap, hole grid, outer border, sort order |

| key | action |
| --- | --- |
| `↑`/`↓`, `j`/`k`, `PgUp`/`PgDn`, `Home`/`End` | move |
| `space` | Pads: cycle undefined → open → ignore → open ...; Sides: toggle; Stencil: pick; Layout: flip a switch or cycle the sort order |
| `o` / `i` | Pads: set open / ignore (`o` on the Stencil page toggles the orientation) |
| `a` | Pads: apply the current pad's state to every pad of the same component |
| `n` / `N` | Pads: next / previous undefined pad |
| `*` | Pads: show all pads, including the ones with paste |
| `A` / `N` | Sides: all on / all off |
| `+` / `-` (also `→` / `←`) | Layout: step a number by 0.5 mm (1 mm for the hole grid) |
| `e` or `Enter` | Layout: type a value (typing replaces, Backspace edits, Enter accepts, Esc cancels) |
| `p` / `v` | re-render the preview / re-render and open it |
| `Enter` on Pads, `g` elsewhere | generate; asks a second time when the layout does not fit |
| `q` or `Esc` | quit without generating; the configuration is saved |

On the Pads page `g` and `G` jump to the first and last row instead.

`/` starts a search on the Pads and Sides pages, vim style: the query is typed
into the footer and the list filters while you type. A row is kept when any
word of the query is a substring of any of its fields (reference, pin,
project, side, function, shape, state, position; pasted pads also answer to
`paste`). Rows are ranked by how many words they match, rows that match every
word are shown bold, and the sub-header says how many rows are shown and how
many are full matches. Enter or Esc leaves the search, clears the filter and
keeps the cursor on the row it was on.

## Output files

Everything goes to `--out` (default `./stencil-out`), prefixed with `--name`
(default `stencil`):

| file | content |
| --- | --- |
| `stencil-F_Paste.gbr` | the stencil: paste openings, opened pads, border dots, dowel holes |
| `stencil-F_Cu.gbr` | the copper of every board, for checking the alignment (`--no-copper` leaves it out) |
| `stencil-Edge_Cuts.gbr` | the sheet rectangle as a 0.1 mm outline, only with `--outline` |
| `stencil.zip` | the gerbers above; this is what we upload |
| `stencil-preview.png` | the preview |
| `stencil-report.txt` | sheet and block size, the winning heuristic, whether every dowel hole is on the grid and what the grid cost in cell area, every cell with its board rectangle and dowel hole coordinates (on the sheet and relative to the board corner), pad counts, dot count, file list |

The gerbers are RS-274X with X2 attributes in the format KiCad writes
(`FSLAX46Y46`, `MOMM`). The preview shows the whole sheet on a dark background
with millimetre rulers along the left and bottom edges, a faint dashed guide
under every dotted line, a label in every cell and a legend underneath:

| colour | meaning |
| --- | --- |
| red | everything that becomes an opening: paste, opened pads, dots, holes |
| yellow | undefined candidates (no opening); a component with undefined pads gets a labelled yellow box |
| blue | ignored candidates and closed paste openings |
| grey | copper, and pads that already have paste |
| light grey | board outlines and the sheet boundary |
| cyan | the pad under the cursor when the preview is rendered from the TUI |

## Options worth knowing

`stencicrity --help` lists everything. The ones we reach for:

- `inputs...` - zip files or gerber directories instead of scanning the folder.
- `--name NAME`, `--out DIR`, `--config FILE` - names and places.
- `--size WxH`, `--portrait`, `--gap MM`, `--no-holes`, `--hole-grid MM`,
  `--dot-line-gap MM`, `--sort name` and the other layout numbers; all of them
  are saved into the `.stencil` file.
- `--only PROJECT[:top|bottom]`, `--exclude PROJECT[:top|bottom]` - switch
  sides on or off; repeatable, case insensitive, a substring is enough
  (`--exclude photo` drops `GERBER-PhotoAmp`); saved as well.
- `--ignore-prefix PREFIX` - replaces the stored `ignore_prefixes` list;
  repeatable; `--ignore-prefix ""` clears it.
- `--include-tht`, `--open-shrink MM`, `--no-mirror-bottom`.
- `--batch` - no TUI; undefined pads stay closed and a warning says how many.
- `--no-open`, `--px-per-mm N` - preview handling (default 20 px/mm, reduced
  automatically so the image stays below 12000 px).
- `--outline`, `--no-copper` - which layers go into the zip.

Exit codes: 0 success, 1 aborted in the TUI or interrupted (the configuration
is still saved), 2 bad input (no projects, unreadable or unsupported gerbers).

## Requirements and installing

Linux with Python 3.11 or newer, `shapely` 2, `pillow` 10.1 or newer and
`numpy`. The TUI uses the standard library `curses` module, which Python on
Linux ships. The preview opens with `xdg-open`; when no viewer can be started
the PNG is still written.

    pip install shapely pillow numpy
    python3 stencicrity.py            # from the folder with the zips

or, for a `stencicrity` command on the PATH:

    pip install -e /path/to/Stencicrity
    cd boards/ && stencicrity

`uv tool install git+https://github.com/Alacrity-Education/Stencicrity`
installs the command into its own environment without touching the system
Python.

### Installing from the release packages

Every release on GitHub carries two packages, both architecture independent:

| file | for |
| --- | --- |
| `stencicrity-<version>-1-any.pkg.tar.zst` | Arch Linux and derivatives |
| `stencicrity_<version>-1_all.deb` | Debian and Ubuntu |

On Arch:

    sudo pacman -U stencicrity-0.1.0-1-any.pkg.tar.zst

pacman pulls `python-shapely`, `python-pillow` and `python-numpy` from the
repositories.

On Debian and Ubuntu:

    sudo apt install ./stencicrity_0.1.0-1_all.deb

The leading `./` is what makes `apt` treat the argument as a file; it then
resolves `python3-shapely`, `python3-pil` and `python3-numpy` from the
archive. `sudo dpkg -i stencicrity_0.1.0-1_all.deb` installs the same file but
leaves the dependencies to a following `sudo apt --fix-broken install`.

The .deb needs Shapely 2, so it installs on Debian 13 (trixie, ships
`python3-shapely` 2.1.0) and Ubuntu 24.04 LTS (noble, 2.0.3) or newer. Debian
12 (bookworm, 1.8.5) and Ubuntu 22.04 (jammy, 1.8.0) are too old and neither
has the newer Shapely in backports; use `pip` or `uv` there.

## Notes and limitations

- An `undefined` pad gets no opening. The header counts them, the report and
  the batch mode warn about the ones left over; check the yellow in the
  preview before ordering.
- The stencil house cuts the paste layer only. `F_Cu` is in the zip for our
  own alignment check; tell them which layer to cut, or drop it with
  `--no-copper`.
- Bottom sides are mirrored. Look at the preview with that in mind.
- Cells are never rotated. A long board that does not fit one way needs the
  other orientation of the sheet or a larger size.
- Paste openings are copied as they are; nothing is shrunk or expanded except
  candidates opened with `--open-shrink`.
- A side that ends up without a single opening (no paste, nothing opened) is
  dropped from the sheet and reported.
- The gerber reader covers what KiCad emits: standard apertures, the common
  macro primitives, arcs, regions, polarity and X2 attributes. Layer detection
  relies on X2 `FileFunction` headers or KiCad style file names. Gerbers from
  other tools may not be recognised.
- Pads without a `%TO.P` attribute get the reference `?` and a running pin
  number; their keys in the `.stencil` file are less stable.

## License

Stencicrity is released under the GNU Affero General Public License,
version 3. See `LICENSE`.
