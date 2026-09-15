# Pads and the configuration file

Two modules, one subject: `pcbstencil/pads.py` decides what a pad is and what
its default state should be, `pcbstencil/config.py` stores the decisions in
`./.stencicrity` and pushes them back into the objects on the next run.

## What counts as a pad

`pads.detect_pads(side, *, include_tht=False, min_overlap=0.10,
ignore_prefixes=DEFAULT_IGNORE_PREFIXES)` walks `side.copper_objects` and keeps
the flashes that are pads. Strokes and regions are never pads, and a flash with
clear polarity (`dark == False`) is skipped as well.

`_is_pad()` decides in this order, on the first token of the aperture's
`%TA.AperFunction`:

| Set | Members | Result |
| --- | --- | --- |
| `NON_PAD_FUNCTIONS` | `ViaPad`, `Conductor`, `NonConductor`, `EtchedComponent`, `Profile`, `WasherPad`, `AntiPad`, `Other`, `Drawing` | never a pad |
| `SMD_FUNCTIONS` | `SMDPad`, `BGAPad`, `HeatsinkPad`, `FiducialPad`, `TestPad`, `ConnectorPad` | always a pad |
| `THT_FUNCTIONS` | `ComponentPad`, `CastellatedPad` | a pad only with `--include-tht` |
| anything else, or no `AperFunction` at all | | a pad when the flash carries a `%TO.P` object attribute |

The last row is the fallback for gerbers without aperture attributes: if the
exporter told us which component pin the flash belongs to, it is a pad.

`%TO.P` is also where the identity comes from. `_ref_pin()` splits it on commas:
the first field is the reference (`U2`), the second the pin (`1`). A flash
without `%TO.P` gets the reference `?` and a running index as the pin, counted
per side over the flashes that had no attribute - so those keys shift as soon as
the board changes. The pad key is

```
<project>/<side>/<REF>.<pin>@<x>,<y>      RBARF/bottom/TP1.1@148.082,-99.568
```

with the board coordinates at three decimals. `detect_pads()` sorts the result
by `natural_key(ref)`, `natural_key(pin)`, x, y - `natural_key` compares
embedded numbers numerically, so `R2` comes before `R10`.

## Which pads the paste layer already covers

Every non-empty paste geometry of the side goes into a shapely `STRtree`.
For each pad, `_paste_hits()` queries the tree with the pad geometry and then,
for every candidate opening:

* `inside` - the opening *contains the pad's centroid*;
* `overlap` - the area of the intersection.

An opening is recorded in `pad.paste_indices` when it contains the centroid or
overlaps at all. The pad counts as pasted when

```
centred  or  (area > 0 and total_overlap >= min_overlap * area)
```

with `min_overlap = 0.10`, i.e. the openings together cover a tenth of the
copper. The centroid rule is what catches a small opening in the middle of a
large thermal pad; the area rule is what catches a pad covered by several
openings, none of which holds the centroid. `min_overlap` is a keyword argument
with no command line switch - the CLI always uses 0.10.

`paste_indices` holds indices into `side.paste_objects`, which is why closing a
pad is exact: `Side.closed_paste_indices()` is the union over the closed pads
and `active_paste_objects` drops precisely those entries. Two pads sharing one
opening both reference it, so closing either removes it.

## Default states

`default_state(pad, ignore_prefixes)` is used twice: once by `detect_pads()`
while building the pad, and once by `apply_config()` for every pad the file
says nothing (usable) about.

| Pad | Default |
| --- | --- |
| has paste | `open` - its openings are already on the stencil |
| candidate whose reference matches an ignore prefix | `ignore` |
| any other candidate | `undefined` |

`matches_prefix(ref, prefixes)` compiles the prefixes into
`^(?:NT|TP)\d` with `re.IGNORECASE` (cached with `lru_cache` on the prefix
tuple), so the prefix has to be followed by a **digit**: `TP1`, `NT12` and `tp3`
match, `TPS1`, `T1` and `R1` do not. An empty prefix list matches nothing.
`config.parse_prefixes("nt, TP")` is the parser for the file and the
`--ignore-prefix` option: it splits on commas and whitespace, upper-cases and
drops duplicates.

Changing `ignore_prefixes` only affects pads the file does not know yet, because
a pad already listed in `[pads]` takes its state from the file. Deleting those
lines re-applies the rule.

## What open and close mean for the output

| State | Effect on the written `F_Paste` |
| --- | --- |
| candidate `undefined` | nothing is cut. The pad is counted in the warnings and drawn yellow in the preview. |
| candidate `open` | `_write_paste()` re-emits the pad's own copper `Flash` (same aperture, same position). With `--open-shrink MM` the copper geometry is buffered inwards by that much with a mitre join and the result is written as a filled region instead; an opening that shrinks to nothing is skipped. |
| candidate `ignore` | nothing is cut. |
| pasted pad `open` | its paste openings are written unchanged. |
| pasted pad `ignore` (*closed*) | the openings listed in `paste_indices` are left out. |

Opening a pad therefore copies the *copper* aperture into the paste layer,
attributes included. Closing a pad removes source openings, which is how a
connector or a shield that is soldered by hand gets left off an otherwise
finished board.

A side whose openings all end up removed has `has_openings() == False` and is
dropped from the final packing with a message.

## The `.stencicrity` file

One text file next to the gerbers, hidden, always called `.stencicrity`
(`--name` does not change it; `--config FILE` points somewhere else). It is
written as soon as the projects are known and again when the TUI exits.

```ini
# pcbstencil configuration and pad decisions (generated 2026-09-15T04:05:22).
# Edit by hand or through the TUI (python3 stencicrity.py).

[stencil]
size = 380x280            # 380x280 | 420x320 | ...
orientation = landscape   # landscape (long side horizontal) | portrait

[layout]
spacing = 30.0            # mm between neighbouring boards
datum = slots             # slots | holes | none
hole_dia = 5.0
hole_inset = 2.0
slot_width = 4.5
slot_length = 12.0
slot_offset = 0.5
slot_pitch = 30.0
slot_web = 3.0
pin_dia = 3.0
dot_dia = 0.5
dot_pitch = 3.0
dot_line_gap = 2.5
hole_grid = 8.0
outer_border = off
sort = height

[rules]
ignore_prefixes = NT TP

[sides]
# <project>/<side> = on | off
RBARF/top = on            # 13.4 x 11.6 mm, 50 paste openings, 0 pads to decide

[pads]
# <project>/<side>/<REF>.<pin>@<x>,<y> = undefined | open | ignore
# --- RBARF / bottom ---
RBARF/bottom/TP1.1@148.082,-99.568 = ignore   # SMDPad C ⌀1.00
```

### Sections and keys

| Section | Key | Value | Attribute |
| --- | --- | --- | --- |
| `[stencil]` | `size` | one of `STENCIL_SIZES` as `WxH`, either order | `Config.size` |
| | `orientation` | `landscape` \| `portrait` | `Config.orientation` |
| `[layout]` | `spacing` (alias `gap`) | float >= 0 | `LayoutParams.gap` |
| | `datum` | `slots` \| `holes` \| `none` | `datum` |
| | `holes` | bool, **legacy only** | mapped onto `datum`, see below |
| | `hole_dia` | float > 0 | `hole_dia` |
| | `hole_inset` | any float, negative allowed | `hole_inset` |
| | `slot_width` | float > 0 | `slot_width` |
| | `slot_length` | float > 0 | `slot_length` |
| | `slot_offset` | float >= 0 | `slot_offset` |
| | `slot_pitch` | float > 0 | `slot_pitch` (the modular jig raster) |
| | `slot_corner` | float, **obsolete** | read and dropped in silence |
| | `slot_web` | float > 0 | `slot_web` |
| | `pin_dia` | float > 0 | `pin_dia` |
| | `dot_dia` | float > 0 | `dot_dia` |
| | `dot_pitch` | float > 0 | `dot_pitch` |
| | `dot_line_gap` | float >= 0 | `dot_line_gap` |
| | `hole_grid` | float >= 0 | `hole_grid` (`holes` datum only) |
| | `outer_border` (alias `border`) | bool | `outer_border` |
| | `sort` | `height` \| `name` | `sort` |
| `[rules]` | `ignore_prefixes` | prefixes separated by commas or spaces | `Config.ignore_prefixes` |
| `[sides]` | `<project>/<side>` | bool | `Config.sides[key]` |
| `[pads]` | `<pad key>` | `undefined` \| `open` \| `ignore` | `Config.pads[key]` |

### `datum` and the legacy `holes` switch

Before the slots datum existed the `[layout]` section had a single boolean,
`holes = on | off`. `datum` replaced it, and `format_config()` no longer writes
a `holes` line at all, but `load_config()` still understands one so an existing
`.stencicrity` keeps behaving the way it did:

| in the file | result |
| --- | --- |
| `datum = slots \| holes \| none` | that mode; anything else warns and keeps the current value |
| `holes = on` (no `datum`) | `datum = holes` |
| `holes = off` (no `datum`) | `datum = none` |
| both keys, in either order | `datum` wins, the `holes` line is ignored |
| neither | the default, `datum = slots` |

`_read_layout()` gets the set of `[layout]` keys the file has already produced,
so the "`datum` wins" rule works whichever order the two lines come in: reading
`datum` records it, and the `holes` branch only assigns when `datum` is not in
that set. A `holes` value that is not a boolean warns and changes nothing. The
mapping is one-way - once the file is written back, only `datum` is in it.

### Obsolete keys

`_OBSOLETE_LAYOUT_KEYS` lists `[layout]` keys that the program once wrote and
no longer uses. They are recognised and dropped **without a warning**, unlike a
key that is simply unknown. There is one entry, `slot_corner`: it placed the
two bottom slots before they moved onto the `slot_pitch` raster, so an older
`.stencicrity` still loads silently and loses the key the next time it is
written.

### Parsing rules

* Section names, keys and values are lower-cased before they are matched, so
  the whole file is case insensitive. Side and pad *keys* keep their case (they
  have to match a project name).
* A line is split at the **last** `=` (`line.rpartition("=")`), so a value may
  not contain one but a key may.
* `#` at the start of a line is a comment. Elsewhere it starts a comment only
  when preceded by whitespace (the regex is `\s#`), which is what lets a pad key
  contain other punctuation safely.
* Booleans accept `on/true/yes/1` and `off/false/no/0`; `parse_bool()` returns
  `None` for anything else and the caller warns.
* Numbers must be finite; NaN and infinities are rejected with a warning.
* Nothing in `load_config()` raises. A missing file gives the built-in defaults;
  an unreadable one, an unknown section, an unknown key, a line without `=`, a
  bad number, a bad boolean, an unknown size, an unknown orientation, an unknown
  sort order or an unknown pad state each produce one `warning: <path>:<line>:
  …` on stderr and the default is kept. An unknown state becomes `undefined`;
  an unparsable side switch becomes `on`.
* Lines **before the first section header** are read as `[pads]`. That is the
  old flat format: a file that is nothing but `<pad key> = <state>` lines still
  loads.

### Stale entries

`format_config()` writes the sides and pads of the current projects, in natural
name order, with a comment giving the board size and the counts (for a side) or
the aperture function and shape (for a pad). Pads with paste are written only
when they are closed, under

```
# --- pads with paste whose opening is closed (state ignore) ---
```

Keys in `config.sides` / `config.pads` that match nothing in the current
projects are appended to their section under

```
# --- not found in the current gerbers ---
```

so a decision made for a board that is temporarily not in the folder survives.
`collect_config()` is what preserves them: it only ever adds or removes keys for
pads it can see.

### The legacy file

Before 0.1.2 the configuration lived in `./<name>.stencil` (default
`stencil.stencil`). When `--config` was not given and `./.stencicrity` does not
exist, `_run()` looks for that file; if it is there it is loaded instead, a
`note: migrated <path> to .stencicrity` line is printed, and everything is
written to the new name. The old file is left untouched.

### Atomic writes

`save_config()` calls `collect_config()` and then `_atomic_write()`, which
creates a `.stencicrity-*.tmp` file in the same directory, chmods it to the
existing file's mode (or `0666 & ~umask` for a new file, since `mkstemp` makes
0600 files) and `os.replace()`s it into place. An interrupted write can never
truncate a good configuration.

## Precedence

Command line > file > built-in default. The mechanism is that every option that
can come from the file defaults to `None` in the parser, meaning "not given":

1. `Config()` starts at the dataclass defaults.
2. `load_config()` overwrites what the file has.
3. `apply_cli_config(config, args)` overwrites what the user actually typed -
   `--size`, `--landscape` / `--portrait`, `--ignore-prefix` and the ten layout
   numbers. `--only` / `--exclude` are applied to the objects a moment later by
   `apply_selection()`, after `apply_config()`, so they win over `[sides]` too.
4. `save_config()` writes the result back, which is why a command line option is
   sticky: give `--gap 20` once and every later run uses 20 until something
   changes it.

Options that are *not* stored in the file and have to be repeated:
`--include-tht`, `--open-shrink`, `--no-mirror-bottom`, `--out`, `--name`,
`--px-per-mm`, `--batch`, `--no-open`, `--outline`, `--no-copper`.

`_check_args()` rejects impossible numbers before anything is loaded (`--gap`
and `--dot-line-gap` and `--hole-grid` and `--open-shrink` must not be negative;
`--hole-dia`, `--dot-dia`, `--dot-pitch` and `--px-per-mm` must be positive) and
exits through `parser.error()`, i.e. with code 2.

## `apply_config` vs `collect_config`

They are the only two functions that cross between the `Config` dictionaries
and the object graph, and they are not symmetric.

| | `apply_config(config, projects)` | `collect_config(config, projects)` |
| --- | --- | --- |
| Direction | dictionaries -> objects | objects -> dictionaries |
| Sides | `side.enabled = config.sides.get(side.key, True)` for **every** side | writes `config.sides[side.key]` only for sides where `is_relevant()` |
| Candidates | the stored state if it is one of `STATES`, else `default_state()` | always stored with the current state |
| Pasted pads | the stored state if it is `open` or `ignore`, else `open` | stored as `ignore` when closed; the key is **removed** when it is open, because open is the default |
| Unknown keys | ignored | left untouched, so they stay in the file |

`save_config()` is `collect_config()` followed by the write, and it is called
twice per run (right after discovery, and again when the TUI exits). The TUI
itself never touches the two dictionaries - it mutates `pad.state`,
`side.enabled` and the scalar configuration, and `collect_config()` picks that
up afterwards.

See [architecture.md](architecture.md) for where these calls sit in the
pipeline and [data-model.md](data-model.md) for the field definitions.
