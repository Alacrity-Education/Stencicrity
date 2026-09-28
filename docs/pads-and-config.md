# Pads and the configuration file

Two modules, one subject: `src/pads.rs` decides what a pad is and what its
default state should be, `src/config.rs` stores the decisions in
`./.stencicrity` and puts them back into the objects on the next run.

## What counts as a pad

`detect_pads(side, &DetectOptions)` walks the **copper** layer's objects and
keeps the dark `Flash`es that look like a component pad. A stroke or a region
is never a pad; neither is a clear (`%LPC`) flash.

The decision is `is_pad(function, attrs_p, include_tht)`, where `function` is
the first token of the aperture's `%TA.AperFunction`:

| Group | Values | Pad? |
| --- | --- | --- |
| `SMD_FUNCTIONS` | `SMDPad`, `BGAPad`, `HeatsinkPad`, `FiducialPad`, `TestPad`, `ConnectorPad` | yes |
| `THT_FUNCTIONS` | `ComponentPad`, `CastellatedPad` | only with `--include-tht` |
| `NON_PAD_FUNCTIONS` | `ViaPad`, `Conductor`, `NonConductor`, `EtchedComponent`, `Profile`, `WasherPad`, `AntiPad`, `Other`, `Drawing` | no |
| anything else, or no `AperFunction` at all | | yes when the flash carries a non-empty `%TO.P` object attribute |

The last row is the escape hatch for gerbers whose apertures are not
annotated: a flash that names a component and a pin is a pad whatever its
aperture says.

`ref_pin` reads `%TO.P`, which is `<ref>,<pin>[,<name>]`:

| `%TO.P` | reference | pin |
| --- | --- | --- |
| `U1,3,VDD` | `U1` | `3` |
| `U1` or `U1,` | `U1` | a per-side running index |
| `,2` | `?` | `2` |
| absent or empty | `?` | a per-side running index |

The running index counts only the flashes without a usable `%TO.P`, which is
why the keys of such pads are unstable: adding one renumbers the rest.

`pad_key(project, side, ref, pin, x, y)` is
`<project>/<side>/<REF>.<pin>@<x>,<y>` with three decimals of the **board**
coordinates, and that is the pad's identity in the configuration file. Board
coordinates, not sheet ones, so the key survives a different sheet size, a
different spacing and a different packing — only moving the footprint in KiCad
changes it.

## Which pads the paste layer already covers

Every paste object's geometry is built once into `paste_geoms`, and their
bounding boxes go into an `rstar::RTree`. For each pad, `paste_hits` queries
the tree for the openings whose envelope intersects the pad's, then:

* an opening that **contains the pad's centroid** makes it pasted outright;
* otherwise the intersection areas are summed, and the pad is pasted when the
  total is at least `min_overlap` (0.10) of the pad's own area.

Either way, every opening that touches the pad — centroid inside, or a
non-zero intersection — is recorded in `pad.paste_indices`, sorted. That list
is what makes *closing* a pad work: setting a pasted pad to `ignore` drops
exactly those openings from the output (`Side::closed_paste_indices`).

The centroid rule is what catches the common KiCad case of a large pad with
several small paste windows: the windows may cover well under 10 % of the
copper, but one of them sits in the middle.

`min_overlap` is a field of `DetectOptions` with no command line switch.

## Default states

A pad that the paste layer covers is `open` — it is on the stencil already.
A pad it does not cover is a **candidate**, and `default_state` gives it:

| Pad | Default |
| --- | --- |
| has paste | `open` |
| no paste, reference matches an ignore prefix | `ignore` |
| no paste, anything else | `undefined` |

`matches_prefix(ref, prefixes)` is `^(?:PREFIX)\d`, case insensitive: the
reference has to start with the prefix **and** the next character has to be a
digit. So with the default `NT TP`, `TP3` and `nt12` match, `TPS1`, `T1` and a
bare `TP` do not. A blank entry in the list never matches.

The default list is `DEFAULT_IGNORE_PREFIXES` = `["NT", "TP"]`: net ties and
test points start closed, so they do not have to be waved through one by one.

Detection happens **after** the configuration is read, because
`[rules] ignore_prefixes` is what this rule reads. `apply_config` then
overwrites the state of every pad the file knows; the default only applies to
a pad the file has never seen. Changing `ignore_prefixes` therefore does not
retroactively re-decide anything — delete the pads' lines to re-apply it.

## What open and close mean for the output

| State | Candidate (no paste) | Pasted pad |
| --- | --- | --- |
| `undefined` | no opening is cut | cannot happen |
| `open` | an opening is cut with the pad's own copper aperture | its source openings are copied |
| `ignore` | no opening is cut | its source openings are **dropped** |

An opened candidate is re-emitted as the very same `Flash` — same aperture,
same position — unless `--open-shrink MM` is given, in which case `cli::shrink`
offsets the pad geometry inward by that much with mitred joins and the result
is written as a region. A shape that shrinks away contributes nothing.

Ordering, for the file and for the TUI: `sorted_pads(&projects, &sides,
all_pads)` sorts by natural project name, top before bottom, natural
reference, natural pin, then x, then y. `Side::pads` itself is sorted by
reference / pin / x / y at detection time.

## The `.stencicrity` file

A hidden file in the folder with the gerbers. `--name` does not change it;
only `--config FILE` points somewhere else. It is written once as soon as the
projects are known and again when the TUI exits.

```ini
# stencicrity configuration and pad decisions (generated 2026-09-28T05:56:29).
# Edit by hand or through the TUI (run stencicrity in this folder).

[stencil]
size = 380x280            # 270x270 | 380x280 | ...
orientation = landscape   # landscape (long side horizontal) | portrait

[layout]
spacing = 30.0            # mm between neighbouring boards; ...
datum = slots             # slots | holes | none - alignment features cut into every cell
...
sort = height             # height (tallest boards first) | name

[rules]
ignore_prefixes = NT TP   # references whose pads without paste default to ignore (prefix + digit)

[sides]
# <project>/<side> = on | off
RBARF/top = on            # 13.4 x 11.6 mm, 50 paste openings, 0 pads to decide

[pads]
# <project>/<side>/<REF>.<pin>@<x>,<y> = undefined | open | ignore
# --- RBARF / bottom ---
RBARF/bottom/TP1.1@148.082,-99.568 = open   # SMDPad C ⌀1.00
```

The two comment lines at the top are regenerated on every save; the timestamp
is local time. Comments are aligned to column 26, or pushed out by at least
three spaces when the key is longer.

### Sections and keys

| Section | Keys |
| --- | --- |
| `[stencil]` | `size` (one of `STENCIL_SIZES`), `orientation` (`landscape` \| `portrait`) |
| `[layout]` | the 19 `LayoutParams` fields — see the table in [data-model.md](data-model.md) |
| `[rules]` | `ignore_prefixes`, whitespace or comma separated, upper-cased |
| `[sides]` | `<project>/<side> = on \| off` |
| `[pads]` | `<pad key> = undefined \| open \| ignore` |

`[layout]` keys are validated as they are read:

| Rule | Keys |
| --- | --- |
| must be positive | `hole_dia`, `slot_width`, `slot_length`, `slot_pitch`, `slot_web`, `pin_dia`, `marker_size`, `dot_dia`, `dot_pitch` |
| must not be negative | `spacing`, `slot_offset`, `dot_line_gap`, `dot_clearance`, `hole_grid` |
| any finite number | `hole_inset` |
| `on` / `off` | `marker`, `outer_border` |
| one of a list | `datum`, `sort` |

A value that fails warns and leaves the default in place. NaN and infinity are
rejected explicitly. A value in `[sides]` that is not a boolean warns and is
read as `on`; a state in `[pads]` that is not one of the three warns and is
read as `undefined`.

### Aliases, legacy spellings and obsolete keys

| In the file | Means | Why |
| --- | --- | --- |
| `gap` | `spacing` | older spelling of the same number |
| `border` | `outer_border` | older spelling |
| `holes = on` / `off` | `datum = holes` / `none` | the datum was a boolean before there were three of them |
| `slot_corner` | nothing | dropped when the slots moved onto the `slot_pitch` raster; read and discarded **without a warning** |

An explicit `datum` always wins over `holes`, whichever order the two come in:
`read_layout` remembers which keys the file has actually carried so far, and
only lets `holes` set the datum when no `datum` line has been seen — and
`datum` overwrites unconditionally.

A file in the **flat legacy format** — only `<pad key> = <state>` lines, no
section headers at all — still loads: the section starts out as `[pads]`, so
everything before the first header is read as a pad decision.

A `./<name>.stencil` left by a pre-0.1.2 run is picked up once when there is
no `.stencicrity` yet, reported with a `note: migrated … to .stencicrity`
line, and written to the new name. The old file is left where it is.

### Parsing rules

* Lines are split on `\n`, `\r\n` and bare `\r`.
* A line whose first non-space character is `#` is a comment. Elsewhere, a
  comment starts at the first whitespace **followed by** `#`, so a `#` inside
  a value is only a comment when something separates it from the value.
* Section names are lower-cased, and so are the keys of `[stencil]`,
  `[layout]` and `[rules]` and every value that is a word rather than a number
  (`landscape`, `slots`, `on`, `ignore`). `[rules] ignore_prefixes` is
  upper-cased instead. Side keys and pad keys are left alone: a project name
  may contain capitals and spaces.
* A key/value line splits on the **last** `=`, so a key containing one still
  works.
* An unknown section warns once and its lines are ignored; an unknown key in a
  known section warns and is dropped.
* `load_config` never fails on content. The only `Err` it can return is from
  reading the file, and even an unreadable file only warns and yields the
  defaults. A missing file is not an error at all.

### Stale entries

Keys that no longer match anything in the current gerbers are never thrown
away: a board that is temporarily not in the folder must not lose its
decisions. `format_config` writes the ones it could not place at the end of
their section, under

```
# --- not found in the current gerbers ---
```

and pads with paste whose opening was closed go under their own header, since
they are not in the per-side blocks:

```
# --- pads with paste whose opening is closed (state ignore) ---
```

Known sides and candidate pads are written in natural project order, with
`# --- <project> / <side> ---` headers; the stale runs follow in the sorted
order of the `BTreeMap`s.

### Atomic writes

`save_config` is `collect_config` plus `atomic_write`, which never truncates
an existing file:

1. create an exclusive `0600` temporary file `.stencicrity-<pid><nanos><n>.tmp`
   in the **same directory** (so the rename cannot cross a filesystem),
   retrying on a name collision;
2. write, `sync_all`;
3. `chmod` it to the mode the target already had, or to `0666 & ~umask` for a
   new file — the umask is read from `/proc/self/status`, falling back to
   `0o022`;
4. `rename` over the target.

On any failure the temporary file is removed. Missing parent directories are
created first.

## Precedence

For everything that can come from more than one place:

```
command line option  >  .stencicrity  >  built-in default
```

Every option that the file also stores defaults to `None` in the clap `Args`
struct — that is what "not given" means — and `apply_cli_config` only writes
the ones that are `Some`. The result is then saved back, so an option given
once sticks.

`--only` and `--exclude` run *after* `apply_config`, so they override the
stored side switches, and they are saved too. `--ignore-prefix` replaces the
stored list outright (it does not add to it); `--ignore-prefix ""` clears it.

The TUI mutates the objects and the scalar parts of the config in place and
never touches `config.sides` / `config.pads`; `cli::run` collects those from
the objects afterwards.

## `apply_config` vs `collect_config`

| | `apply_config(&config, &mut projects)` | `collect_config(&mut config, &projects)` |
| --- | --- | --- |
| direction | file → objects | objects → file |
| sides | `side.enabled = config.sides[key]`, default `true` | every *relevant* side is stored |
| candidate pads | the stored state when it is one of `STATES`, else `default_state()` | always stored, with its state |
| pasted pads | the stored state when it is `open` or `ignore`, else `open` | stored as `ignore` when closed; the key is **removed** when it is open, because open is the default |
| stale keys | ignored | left untouched, so they survive |

The asymmetry in the last two rows is the whole trick: the file only ever
grows entries that carry information, and never loses one it cannot currently
explain.
