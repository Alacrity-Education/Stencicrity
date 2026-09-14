# Distributing `stencicrity` to a Windows + Arch Linux fleet — research report

Date: 2026-09-15. Research only — nothing was implemented, nothing in
`/home/alex.lucaci/comanda-stencil` was touched. All builds and downloads below happened in
`/tmp/claude-2017/…/scratchpad/dist-research/`.

---

## 0. Executive summary

**Recommendation — a three-channel combination, in this order:**

1. **Put the code in git first and host it** (GitHub private repo is enough). *Everything* else in
   this report is blocked on this: PyPI publishing, AUR, `pip install git+…`, `uv tool install
   git+…`, and CI-built Windows exes all need a URL. The project is currently not a git repo at all
   (verified: `git status` → "not a git repository") and has no `LICENSE` file.
2. **Arch machines → `uv tool install` (or `pipx install`) from the git URL.** One command, no
   root, no PEP 668 fight, dependencies arrive as prebuilt manylinux wheels, `uv tool upgrade
   stencicrity` updates. Skip the AUR entirely unless the tool is meant to be public — the AUR is a
   public repository and adds an SSH key, a `.SRCINFO`, a tagged-release cadence and a namcap pass
   for zero benefit on an internal tool. If you want `pacman -Syu` to carry it, build a
   `.pkg.tar.zst` and host a tiny custom pacman repo on the internal share (section 3) — that is
   strictly less work than the AUR and keeps the code private.
3. **Windows machines → a PyInstaller `--onedir` build produced by a GitHub Actions
   `windows-latest` job**, dropped on the internal share or a GitHub release. Nothing to install on
   the target, no Python, no `windows-curses`, no PEP 668. This is where most of the effort is, and
   it is the only channel that does not require the team to install anything first.
   - A cheaper Windows alternative that works *today* with ~1 hour of work: install `uv` on each
     Windows box (`winget install --id=astral-sh.uv -e`) and `uv tool install git+…`. uv downloads
     its own CPython, so the machines do not need Python at all. This is the fastest path to "it
     runs"; the .exe is the polish.

**The single most important finding:** the tool cannot run on Windows as it stands. `pcbstencil/tui.py`
does `import curses` at module level, and `pcbstencil/cli.py:388` imports `.tui` unconditionally at
the top of `_run()` — *even for `--batch`*. On Windows this is an `ImportError: No module named
'_curses'` on every invocation. The fix is one environment marker in `pyproject.toml` plus moving
one import; see section 6.

**Second finding:** `windows-curses` — the only way to get `curses` on Windows — **declares itself
unmaintained** ("This project is not actively maintained and is looking for maintainers", README on
GitHub), *but* it is currently in good shape: 2.4.2 was released 2026-03-31 with cp36–cp314 wheels
for win32 and win_amd64, which includes a Python 3.14 build. It is a real single-point-of-failure
for the pip/uv-on-Windows channel and a reason to prefer the frozen .exe there (where the `_curses`
extension is baked in and never needs to be resolved again).

---

## 1. Verified facts (measured or fetched today)

### 1.1 The tool

| Fact | Value |
|---|---|
| Name / package | `stencicrity` (dist) / `pcbstencil` (import) + `stencicrity.py` shim |
| Console script | `stencicrity = pcbstencil.cli:main` |
| requires-python | `>=3.11` |
| Deps | `shapely>=2.0`, `pillow>=10.1`, `numpy` |
| Size | 5 844 lines across 11 modules; `tui.py` alone is 1 585 lines |
| Build backend | `setuptools>=68`, `[tool.setuptools] packages = ["pcbstencil"]` → **pure-Python, `py3-none-any` wheel** |
| Version | Declared twice: `pyproject.toml` `version = "0.1.0"` and `pcbstencil/__init__.py` `__version__` |
| git | **Not a git repository**, no remote, no public hosting |
| LICENSE file | **Absent** |
| Platform-specific code | `pcbstencil/render.py:626` `sys.platform.startswith("win")` → `os.startfile`, else `xdg-open`/`open` — already correct |
| Path handling | `os.path` + `os.sep` throughout, `.replace(os.sep, "/")` when normalising zip members — portable |
| File I/O | All text I/O passes `encoding="utf-8"` explicitly (`config.py`, `writer.py`, `cli.py`) — portable |
| Atomic config write | `tempfile.mkstemp(dir=…)` + rename in `config.py:460` — works on Windows (same directory) |
| Font lookup | `render.py:65` `_FONT_PATHS = ("DejaVuSans.ttf", "/usr/share/fonts/TTF/…", "/usr/share/fonts/truetype/dejavu/…")` → **Linux-only paths** |
| curses API used | `wrapper`, `getch`, `KEY_RESIZE`, `KEY_BACKSPACE`, `curs_set`, `start_color`, `use_default_colors`, `init_pair`, `color_pair`, `A_BOLD`/`A_DIM`/`A_REVERSE`/`A_NORMAL`, `doupdate` |
| Non-ASCII glyphs in source | `—`(12) `…`(10) `·`(9) `⌀`(5) `↓`(5) `↑`(5) `→`(4) `é`(1) `●`(1) `←`(1) `×`(1) |
| Batch mode | `--batch` exists (skips the TUI) and `--no-open` (skips the preview launch) |

### 1.2 The Arch side (read off this machine)

```
python            3.14.7-2        (extra)      ← "python" on Arch today is 3.14
python-shapely    2.1.2-2         (extra)      depends: geos, python
python-pillow     12.3.0-1        (extra)
python-numpy      2.5.3-1         (extra)
python-build      1.6.0-1         (extra)
python-installer  1.0.1-1         (extra)
python-wheel      0.48.0-1        (extra)
python-setuptools 1:84.0.0-1      (extra)
python-pipx       1.15.0-1        (extra)
uv                0.12.13-1       (extra)
makepkg           7.1.0
/usr/lib/python3.14/EXTERNALLY-MANAGED   ← PEP 668 marker present
devtools, namcap  NOT installed (needed for pkgctl build / extra-x86_64-build / namcap)
```

All three runtime dependencies are in the **official repos**, none in the AUR. That makes an
Arch-native package trivial dependency-wise.

### 1.3 Wheel availability on PyPI (fetched from the JSON API today)

| Package | Latest | Released | requires-python | cp313 win_amd64 | cp314 win_amd64 | cp313/cp314 manylinux x86_64 |
|---|---|---|---|---|---|---|
| shapely | 2.1.2 | 2025-09-24 | >=3.10 | ✅ | ✅ (+ cp314t) | ✅ both |
| numpy | 2.5.3 | 2026-09-06 | **>=3.12** | ✅ | ✅ (+ arm64, cp315) | ✅ both |
| pillow | 12.3.0 | 2026-07-01 | >=3.10 | ✅ | ✅ (+ arm64, cp315) | ✅ both |
| windows-curses | 2.4.2 | 2026-03-31 | (unset) | ✅ | ✅ | n/a |

Notes:
- **shapely is the laggard.** 2.1.2 is ~12 months old and is the only one of the four with **no
  cp315 wheels**. When Arch moves `python` to 3.15, `pip`/`uv` installs of shapely will fall back to
  building from the sdist (needs GEOS + a compiler) unless shapely ships 2.1.3/2.2 first. Arch's own
  `python-shapely` will be rebuilt by the Arch maintainers, so the pacman channel is immune.
- `numpy>=2.5` requires Python ≥3.12, so on a Python 3.11 box pip resolves an older numpy. Since the
  project pins nothing, this is fine, but it means "requires-python >=3.11" is not really tested.
- **windows-curses has no `win_arm64` wheels** (open issue
  [#85](https://github.com/zephyrproject-rtos/windows-curses/issues/85), 2026-07-02) and no sdist.
  Windows-on-ARM machines cannot run the TUI from pip at all.

### 1.4 windows-curses, in detail

- Repo: <https://github.com/zephyrproject-rtos/windows-curses>. Last push **2026-03-31** ("Bump to
  2.4.2"). Not archived. 214 stars, 20 open issues.
- README: *"Maintainers Wanted — This project is not actively maintained and is looking for
  maintainers."* PyPI classifier is `Development Status :: 4 - Beta`.
- Python 3.14 support: issue [#76](https://github.com/zephyrproject-rtos/windows-curses/issues/76)
  ("Support for Python 3.14") **closed 2026-03-31**, shipped in 2.4.2. 3.13 was
  [#69](https://github.com/zephyrproject-rtos/windows-curses/issues/69), closed 2025-01-04. So the
  lag from a new CPython release to a windows-curses wheel has historically been ~2–5 months.
- What the wheel contains (I unpacked `windows_curses-2.4.2-cp314-cp314-win_amd64.whl`, 86 KB):
  only `_curses.cp314-win_amd64.pyd` and `_curses_panel.cp314-win_amd64.pyd`. The `curses` *package*
  is stdlib and ships on Windows already (CPython's `PC/layout/main.py` excludes only
  `*.pyc`/`__pycache__`/`*.pickle` and tkinter); windows-curses only supplies the missing C
  extension. **This is why PyInstaller bundles it correctly with no hook** — see 4.1.
- Build flags (`setup.py`): `PDC_WIDE`, `HAVE_NCURSESW`, `HAVE_CURSES_RESIZE_TERM`,
  `NCURSES_MOUSE_VERSION=2`. Wide-character support, **UTF-8 forced as the encoding**, `get_wch()`
  available.
- Resize: since 2.0 the wheels auto-call `resize_term(0, 0)` whenever `getch`/`getkey`/`get_wch`
  return `KEY_RESIZE`, so the TUI's `KEY_RESIZE` branch needs no change. **But** open issue
  [#74](https://github.com/zephyrproject-rtos/windows-curses/issues/74) (2025-01-13, 0 comments)
  reports *"Window resize crashes applications (visidata, Glances)"* on Windows 10. A 1 585-line
  curses TUI that redraws on resize is exactly the shape of app that hits this. **Test resizing
  early.** Also open: [#75](https://github.com/zephyrproject-rtos/windows-curses/issues/75)
  (`cursyncup` silent crash — not used by this TUI).
- `use_default_colors()` / `init_pair(n, fg, -1)`: **supported.** PDCurses `pdcurses/color.c`
  documents `use_default_colors` and `assume_default_colors` as ncurses-compatible extensions with
  "Y" support in the wincon port.
- **`A_DIM` is a no-op on Windows.** PDCurses `curses.h` line 459: `#define A_DIM  A_NORMAL`, and
  `wincon/pdcdisp.c` maps only `A_REVERSE`/`A_BOLD`/underscore/grid to console attributes. The TUI
  uses `A_DIM` to grey out `STATE_IGNORE` rows (`tui.py:838`, `842`) and `_boost()` strips it
  (`tui.py:863`). On Windows those rows will render at normal intensity — ignored pads lose their
  visual distinction. Cosmetic, but it changes how the TUI reads. Colour still differentiates them
  (blue vs green vs yellow).

### 1.5 Glyphs in a Windows console

I downloaded Cascadia Code v2407.24 (the Windows Terminal default family) and parsed its `cmap`
(format 12, 2 426 codepoints) for every non-ASCII character the TUI prints:

| Glyph | Codepoint | In Cascadia Mono |
|---|---|---|
| `·` `→` `↑` `↓` `←` `×` `●` `…` `—` `é` | U+00B7, 2192, 2191, 2193, 2190, 00D7, 25CF, 2026, 2014, 00E9 | **PRESENT** |
| `⌀` | **U+2300** | **MISSING** |

`⌀` (DIAMETER SIGN, used 5× — it labels round pads, e.g. `SMDPad C ⌀1.00`) is not in Cascadia Mono.
Windows Terminal will DirectWrite-font-fallback it (Segoe UI Symbol covers U+2300), so it will
almost certainly still render, possibly at the wrong width. *Unverified in practice — I have no
Windows machine here.* In **legacy conhost** (`cmd.exe` launched directly, especially with a raster
font) expect a box or a blank. Safe fix: make the glyph configurable or fall back to `dia` /
`D` when `sys.stdout.encoding` is not UTF-8 (see section 6).

### 1.6 Glyphs in the rendered PNG (Pillow)

Different problem, same cause. `render.py:_font()` tries `DejaVuSans.ttf` then two absolute Linux
paths, then `ImageFont.load_default(size=…)`.

On Windows, Pillow's `truetype()` fallback search covers **only `%WINDIR%\fonts`**
(`PIL/ImageFont.py:892-898` — verified in the installed Pillow 12.3.0). DejaVu is not shipped with
Windows, so all three paths fail and the code falls back to Pillow's bundled Aileron font. I tested
that font's coverage by rendering each character and comparing to `.notdef`:

```
'A' present   '·' present   '…' present
'⌀' MISSING   '→' MISSING   '↑' MISSING   '↓' MISSING
'×' MISSING   '●' MISSING   '—' MISSING
```

`render.py` draws `·` (cell labels, line 566), `…` (ellipsis truncation, line 117) and **`—`** (the
`"   —  DOES NOT FIT"` warning, line 583). So on Windows the *"DOES NOT FIT"* warning in the preview
PNG renders with a `.notdef` box where the em-dash should be. Fix: add `arial.ttf`/`segoeui.ttf` to
`_FONT_PATHS`, or ship `DejaVuSans.ttf` as package data (~750 KB, Bitstream Vera licence — freely
redistributable).

### 1.7 Name availability

`stencicrity` and `pcbstencil` are both **free on PyPI** (HTTP 404 on the JSON API), and
`stencicrity` / `python-stencicrity` are both **free on the AUR** (RPC `resultcount: 0`). PyPI names
are not purely first-come-first-served (PEP 541 / similarity rules), but nothing collides today.

---

## 2. Option 1 — pip / PyPI / pipx / uv

### 2.1 Requirements on each machine

| | Arch | Windows |
|---|---|---|
| Python | Already there (`python` 3.14.7). System Python is **PEP 668 externally-managed**. | **Not present by default.** Either install CPython from python.org/Microsoft Store/`winget`, or let `uv` supply one. |
| Installer | `pipx` (`extra/python-pipx` 1.15.0) or `uv` (`extra/uv` 0.12.13) via pacman | `uv` (`winget install --id=astral-sh.uv -e`, or `powershell -ExecutionPolicy ByPass -c "irm https://astral.sh/uv/install.ps1 \| iex"`) or `pipx` |
| git | Needed for `git+…` installs | Needed for `git+…` installs |
| Compiler | **No.** manylinux wheels exist for cp313/cp314. | **No.** win_amd64 wheels exist for cp313/cp314. |

**uv downloads its own Python.** uv's default `python-downloads = automatic` means `uv tool install`
and `uv run` fetch a managed CPython (astral-sh/python-build-standalone; Windows, macOS, Linux) when
no suitable interpreter is found. That is the single biggest argument for uv on Windows: *the
machines need nothing but uv.*

### 2.2 PEP 668 on Arch

`/usr/lib/python3.14/EXTERNALLY-MANAGED` exists. Per the Arch Wiki (`Python`, §"Installation"):
*"Previous versions of pip could install third-party packages system-wide, but this caused a number
of problems outlined in PEP 668. The system-wide environment is now marked as an externally managed
environment, and pip no longer allows system-wide installation."*

Consequences:
- `pip install --user stencicrity` → **fails** with `error: externally-managed-environment`.
- `pip install --break-system-packages` works but is exactly the footgun PEP 668 exists to prevent;
  a `pacman -Syu` that bumps `python` to 3.15 silently orphans every `--break-system-packages`
  install in `~/.local/lib/python3.14/`. **Do not tell the team to do this.**
- `pipx install …` and `uv tool install …` are the sanctioned routes: each tool gets its own venv
  under `~/.local/share/pipx/venvs/` or `~/.local/share/uv/tools/`, with a shim on `PATH`.
- Caveat that bites on Arch specifically: a pipx venv created with `--system-site-packages` or
  built against `/usr/bin/python3.14` **breaks when Arch bumps `python` to 3.15** — the venv's
  symlinked interpreter disappears. `pipx reinstall-all` is the fix; `uv tool install` with a
  uv-managed Python avoids it entirely because uv owns the interpreter. Recommend
  `uv tool install --python 3.13 …` (a uv-managed build) rather than the system interpreter.

### 2.3 Route A — publish to PyPI

Steps (described, not executed):

1. `git init`, first commit, push to a host. **PyPI is public**, so this route only makes sense if
   the tool is meant to be public.
2. Add a `LICENSE` file and fill in the `[project]` table: `license`, `authors`, `urls`,
   `classifiers`, `keywords`. Fold the duplicated version into one source of truth
   (`[project] dynamic = ["version"]` + `[tool.setuptools.dynamic] version = {attr = "pcbstencil.__version__"}`).
3. Add the Windows conditional dependency (section 6).
4. Create a PyPI account. **2FA is mandatory on PyPI** for every account (TOTP or a FIDO security
   key, plus recovery codes).
5. Configure a **pending trusted publisher** on PyPI (Your projects → Publishing) naming the GitHub
   repo, workflow filename and environment. Trusted publishing uses OIDC — no API token to store,
   no token to rotate, and the action mints a 15-minute token at publish time.
6. Add `.github/workflows/release.yml`: on tag push → `python -m build` (produces
   `stencicrity-0.1.0.tar.gz` + `stencicrity-0.1.0-py3-none-any.whl`) → upload artifact → a publish
   job with `permissions: id-token: write`, `environment: pypi`, and
   `uses: pypa/gh-action-pypi-publish@release/v1`. Since v1.11.0 the action also emits PEP 740
   attestations automatically. Dry-run against TestPyPI first.
7. Users then run `uv tool install stencicrity` / `pipx install stencicrity`.

Updates propagate by `uv tool upgrade stencicrity` / `pipx upgrade stencicrity` (or `--all`). There
is no push mechanism — someone has to run the command, or you wrap it in a scheduled task /
systemd timer.

**Effort:** 3–5 h the first time (most of it PyPI account + trusted-publisher setup + one
throwaway TestPyPI round-trip). **Maintenance:** ~15 min per release (tag and push).
**Blockers:** the project must become public; no LICENSE today; not in git today.

### 2.4 Route B — install straight from git (recommended for an internal tool)

No PyPI, no index, no build step. On either OS:

```
uv tool install git+https://github.com/<org>/stencicrity          # tip of default branch
uv tool install git+https://github.com/<org>/stencicrity@v0.1.0   # a tag
uv tool upgrade stencicrity
pipx install git+https://github.com/<org>/stencicrity             # pipx equivalent
```

uv documents `git+https://…`, `@branch`, `@tag`, `@commit` forms. For a **private** repo, either use
`git+ssh://git@github.com/org/repo` with the machine's SSH key, or a PAT in the URL (avoid — it
lands in shell history and in `uv tool list` metadata). SSH is the clean option; on Windows that
means OpenSSH + a key in `ssh-agent`, which is the main per-machine friction.

Because the wheel is `py3-none-any` and built on the fly by setuptools, nothing is compiled; only
shapely/numpy/pillow are fetched, as prebuilt wheels.

**Effort:** 1–2 h total (git init/push + a one-line install instruction). **Maintenance:** near
zero. **Blocker:** git must be installed on every machine; private-repo auth on Windows.

### 2.5 Route C — a private index

`pip install --index-url https://pypi.internal/simple stencicrity`, backed by devpi, `pypiserver`,
or Artifactory/Nexus if you already run one. Worth it only if you also want to mirror/pin the
dependency wheels for offline installs. If you already have an internal file share, a directory of
wheels served over HTTP with `--find-links` is the 30-minute version of this.

**Effort:** 1 day for a real private index; 1 h for a `--find-links` directory. **Maintenance:**
you now own an availability-critical service.

### 2.6 How `windows-curses` gets pulled in only on Windows

An environment marker in `pyproject.toml`:

```toml
dependencies = [
    "shapely>=2.0",
    "pillow>=10.1",
    "numpy",
    "windows-curses>=2.4.2; sys_platform == 'win32'",
]
```

The marker is evaluated by the installer at install time, so the wheel stays `py3-none-any` and the
same artifact serves both OSes. `sys_platform == "win32"` is true on 64-bit Windows too (it is the
`sys.platform` string, not the architecture). `platform_system == "Windows"` is equivalent and
arguably clearer. Pin the floor at `>=2.4.2` so a Python 3.14 box cannot resolve to an older wheel
that has no cp314 build.

---

## 3. Option 2 — the AUR

### 3.1 Naming

Arch's official Python packaging guidelines (developer manual, *Package Guidelines → Python*,
<https://manual.archlinux.page/package-guidelines/python/>) are explicit:

> For Python 3 library modules, use `python-modulename`. Also use the prefix if the package provides
> a program that is strongly coupled to the Python ecosystem (e.g. pip or tox). **For other
> applications, use only the program name.**

`stencicrity` is an end-user application, not a Python-ecosystem tool → the package name is
**`stencicrity`**, not `python-stencicrity`. Lowercase. `arch=('any')` because the wheel is pure
Python (the compiled bits live in the *dependencies*, which are separately packaged).

### 3.2 What the PKGBUILD needs

Following the official PEP 517 recipe from the same page, and modelled on the real
`extra/python-pipx` PKGBUILD (which I fetched from `gitlab.archlinux.org` to confirm the idiom):

```
pkgname=stencicrity
pkgver=0.1.0
pkgrel=1
pkgdesc='Merge KiCad paste/copper gerbers of several projects into one stencil order'
arch=(any)
url='https://github.com/<org>/stencicrity'
license=(<SPDX id>)                       # ← needs a LICENSE decision first
depends=(python python-shapely python-pillow python-numpy)
makedepends=(git python-build python-installer python-wheel python-setuptools)
source=("git+$url.git#tag=v$pkgver")       # or a release tarball + sha256sums
sha256sums=('SKIP')

build()   { cd stencicrity; python -m build --wheel --no-isolation; }
package() { cd stencicrity
            python -m installer --destdir="$pkgdir" dist/*.whl
            install -Dm644 LICENSE -t "$pkgdir/usr/share/licenses/$pkgname/"; }
```

Dependency check — **all three runtime deps are in `extra`, none in the AUR**:
`python-shapely 2.1.2-2` (which itself depends on `geos` and lists `python-numpy` only as an
*optdepend*, for `shapely.vectorized` — so `python-numpy` must be listed explicitly since
`pcbstencil` imports numpy directly), `python-pillow 12.3.0-1`, `python-numpy 2.5.3-1`.
`python-build`/`python-installer`/`python-wheel`/`python-setuptools` are all in `extra` too.

The guidelines also note that `--no-isolation` builds against what is installed on the system, and
that the build backend (`setuptools` here) must be in `makedepends`.

### 3.3 Submission mechanics (Arch Wiki, *AUR submission guidelines*)

1. Create an AUR account at <https://aur.archlinux.org>.
2. Generate a **dedicated** SSH key (`ssh-keygen -f ~/.ssh/aur`), paste the public key into *My
   Account*, and add to `~/.ssh/config`:
   `Host aur.archlinux.org / IdentityFile ~/.ssh/aur / User aur`.
3. `git clone ssh://aur@aur.archlinux.org/stencicrity.git` (empty repo is created on first push).
4. Write `PKGBUILD`, then `makepkg --printsrcinfo > .SRCINFO`.
5. `git add PKGBUILD .SRCINFO` (+ any `.install` / patches / a package-source licence),
   commit, `git push`. **Pushes are only accepted to `master`**, and **the push is rejected if
   `.SRCINFO` is missing from the last commit**. Regenerate `.SRCINFO` on *every* metadata change
   including `pkgver()` bumps, or the AUR shows a stale version.
6. Users install with `yay -S stencicrity` / `paru -S stencicrity` (or `git clone` + `makepkg -si`).

Rules that matter here:
- The submitted PKGBUILD must not duplicate anything already in the official repos — fine.
- Packages that don't support `x86_64` are not allowed — fine.
- A VCS package must carry the `-git` suffix (`stencicrity-git`) and a `pkgver()` function;
  the guidelines additionally recommend `git -C "$srcdir/$pkgname" clean -dfx` in `prepare()` for
  Python VCS packages, to stop stale wheels leaking between builds.
- **The AUR is public.** The PKGBUILD, the URL it points at, and therefore the source must be
  publicly reachable. That is the hard blocker: the code is not in git and has no public host.

### 3.4 Quality gates

- `namcap PKGBUILD` and `namcap stencicrity-0.1.0-1-any.pkg.tar.zst` — flags missing deps,
  over-declared deps, wrong `arch`, missing licence. **Not installed here** (`pacman -Q namcap` →
  not found); `pacman -S namcap` first.
- `pkgctl build` (or `extra-x86_64-build`) from the `devtools` package builds in a **clean chroot**
  under `/var/lib/archbuild`, which is the only reliable way to catch a dependency you forgot to
  declare because it happens to be installed on your workstation. **Also not installed here**
  (`pacman -Q devtools` → not found). `pkgctl build` is the modern entry point; `extra-x86_64-build`
  is the older per-repo script.

### 3.5 `-git` vs tagged releases

| | `stencicrity` (tagged) | `stencicrity-git` (VCS) |
|---|---|---|
| `source=` | release tarball or `git+…#tag=v$pkgver` with a checksum | `git+…` (no tag), `sha256sums=('SKIP')` |
| `pkgver` | bumped by hand, `.SRCINFO` regenerated, pushed | computed by a `pkgver()` function from `git describe` |
| Update cadence | you push to the AUR per release | `yay -Syu --devel` rebuilds from HEAD |
| Fit here | good once you cut tags | good for a fast-moving internal tool — but still needs a public repo |

**Effort:** 4–6 h first time (account, key, PKGBUILD, namcap/chroot iteration), **on top of**
making the repo public. **Maintenance:** ~20 min per release (bump `pkgver`, regenerate `.SRCINFO`,
push) plus fielding AUR comments from strangers.
**Verdict: skip it for an internal tool.** It buys nothing over section 4 except discoverability by
people outside the team, and it costs you a public repository.

---

## 4. Option 3 — an Arch package file, and a private pacman repo

This is the AUR's payload without the AUR's publicity. Same PKGBUILD as 3.2, but
`source=("file://$PWD/stencicrity-$pkgver.tar.gz")` or a `git+ssh://` URL to the private repo.

### 4.1 One-off package file

1. `makepkg -f` in the PKGBUILD directory → `stencicrity-0.1.0-1-any.pkg.tar.zst` (a few tens of KB,
   since it is just the `.py` files under `/usr/lib/python3.14/site-packages/pcbstencil/` plus
   `/usr/bin/stencicrity`).
2. Ship the file (share, scp, whatever) and `sudo pacman -U stencicrity-0.1.0-1-any.pkg.tar.zst`.
   pacman resolves `python-shapely`/`python-pillow`/`python-numpy` from the official repos
   automatically.

**Caveat:** because the package installs into the versioned
`/usr/lib/python3.14/site-packages/…`, it must be **rebuilt when Arch bumps `python` to 3.15**, or
the files land in a directory the new interpreter does not scan. This is exactly what Arch's own
Python rebuilds do (note `python-shapely` is at `pkgrel=2`). With a custom repo you just rebuild and
`repo-add`; with loose `.pkg.tar.zst` files you have to remember.

**Effort:** 2–3 h. **Maintenance:** a rebuild per release *and* per Arch Python major bump.

### 4.2 A tiny custom pacman repository (recommended Arch channel)

From the Arch Wiki, *Pacman/Tips and tricks* §"Custom local repository":

1. Lay the tree out per architecture:
   ```
   /srv/repo/x86_64/stencicrity-0.1.0-1-any.pkg.tar.zst
                    alacrity.db -> alacrity.db.tar.zst
                    alacrity.db.tar.zst
                    alacrity.files -> alacrity.files.tar.zst
   ```
2. `repo-add /srv/repo/x86_64/alacrity.db.tar.zst /srv/repo/x86_64/*.pkg.tar.zst`
   (the db and the packages must live in the same directory for clients; `repo-add` adds entries in
   **command-line order**, so when several versions are present make sure the newest is last —
   shell glob order is locale-dependent and differs from `vercmp` order).
   `repo-remove <db> <pkgname>` to drop one.
3. Serve `/srv/repo` over HTTP (nginx, `python -m http.server`, or an existing internal web server).
   SMB works too via a `file://` URL on a mounted share, but HTTP is less brittle.
4. On every Arch machine, append to `/etc/pacman.conf` — the repo name is the db filename without
   extensions, and it must come **before** the official repos only if you intend to shadow them
   (you don't):
   ```
   [alacrity]
   SigLevel = Optional TrustAll
   Server = http://files.internal/repo/$arch
   ```
   `/etc/pacman.conf` already ships this exact stanza commented out at lines 111–115.
5. `sudo pacman -Sy stencicrity` once; thereafter **`pacman -Syu` upgrades it along with everything
   else.** That is the one-command update story for the Arch half of the fleet, and it is the only
   channel in this whole report where updates ride an existing habit instead of a new one.

**Signing (optional but recommended if it is served over plain HTTP):**
- `gpg --full-gen-key` a repo key, `makepkg --sign` (or `PACKAGER=`/`GPGKEY=` in `makepkg.conf`) to
  produce `*.pkg.tar.zst.sig`, `repo-add --sign` to sign the db.
- Distribute the public key and `pacman-key --add` + `pacman-key --lsign-key <fpr>` on each client,
  then set `SigLevel = Required DatabaseOptional` (or `Required`) instead of `Optional TrustAll`.
- Note `/etc/pacman.conf` already has `LocalFileSigLevel = Optional` and a commented
  `RemoteFileSigLevel = Required`.

**Effort:** 4–6 h including hosting (add 2–4 h for signing and key distribution).
**Maintenance:** `makepkg && repo-add && rsync` per release — scriptable to one command, and
schedulable in CI on a Linux runner.

### 4.3 Windows side of this option

There is none. **pacman does not exist on Windows** outside MSYS2, and MSYS2's pacman serves MSYS2's
own package universe — a native-Windows CPython cannot consume an `any`-arch Arch package, the
paths (`/usr/lib/python3.14/site-packages`) are meaningless there, and MSYS2's own Python is a
separate runtime that would need its own shapely/pillow/numpy. Building an MSYS2 package would mean
maintaining a second, parallel packaging stack for a handful of machines. **Don't.** The Windows
half needs section 5 (an .exe) or section 2 (uv).

---

## 5. Option 4 — native executable wrappers

### 5.1 PyInstaller — **measured here**

I built the real project (copied into the scratchpad) with PyInstaller 6.22.3 on Arch, Python
3.14.7, shapely 2.1.2 / pillow 12.3.0 / numpy 2.5.3:

| | `--onedir` (default) | `--onefile` |
|---|---|---|
| Build time (Linux, warm) | **5.3 s** | **7.3 s** |
| Output size | **100 MB** (folder) | **39 MB** (single ELF, compressed) |
| `--help` wall time | **0.095 s** | **0.26 s** (cold and warm) |
| Baseline `python stencicrity.py --help` | 0.077 s | — |

Both binaries ran (`--version` → `stencicrity 0.1.0`, `--help` → full usage). Bundle contents
confirmed:

- `shapely.libs/libgeos_c-abcdd5fa.so.1.19.2` **and** `libgeos-3ef06f11.so.3.13.1`, plus copies at
  the top level — the `pyinstaller-hooks-contrib` `hook-shapely.py` handled it with zero
  configuration.
- `python3.14/lib-dynload/_curses.cpython-314-x86_64-linux-gnu.so` — **curses bundled
  automatically**, even though `from .tui import run_tui` is a *function-local* import at
  `cli.py:388` (PyInstaller's modulegraph scans all code objects, not just module-level imports).
- numpy pulled in `libscipy_openblas64_*.so` (~35 MB) — that alone is most of the size. If size
  matters, check whether numpy is load-bearing; `pcbstencil` imports it in `render.py` and
  `pads.py`, and shapely's STRtree path uses it, so it probably is.
- **No `.ttf` is bundled** — the frozen build finds DejaVu on the Arch *system*. On Windows it will
  not (see 1.6). If you go the PyInstaller route on Windows, add
  `--add-data "DejaVuSans.ttf:."` and add `os.path.join(sys._MEIPASS, "DejaVuSans.ttf")` to
  `_FONT_PATHS`, or just add `arial.ttf` to that tuple.

**Does it bundle `windows-curses` correctly?** Structurally, yes, and for a clear reason: the wheel
contains nothing but a top-level `_curses.cp3XX-win_amd64.pyd` (+ `_curses_panel`), and the stdlib
`curses` package — which *does* ship on Windows — does `import _curses`. PyInstaller's analysis
follows that import and collects the `.pyd` exactly as it collected the `.so` here. There is no
hook-shapely-style special case needed and no hook in `pyinstaller-hooks-contrib` for curses (I
enumerated the repo tree: the only relevant hooks are `hook-shapely.py`, `hook-PIL*.py`,
`hook-numpy.py`). *Unverified on an actual Windows machine — I could not test there.* PDCurses links
against `user32/advapi32/gdi32/comdlg32/shell32`, all system DLLs, so nothing else needs collecting.

**Shapely on Windows specifically:** the win_amd64 wheel (I unpacked
`shapely-2.1.2-cp314-cp314-win_amd64.whl`) contains `shapely.libs/geos-<hash>.dll`,
`shapely.libs/geos_c-<hash>.dll`, `shapely.libs/msvcp140-<hash>.dll`, and a delvewheel patch at the
top of `shapely/__init__.py` that calls `os.add_dll_directory(<parent>/shapely.libs)`. The current
`hook-shapely.py` collects `<site-packages>/Shapely.libs/*` into a `Shapely.libs` destination and
asserts a file starting with `geos_c` is present. The case mismatch (`Shapely.libs` vs
`shapely.libs`) is harmless **because NTFS is case-insensitive**; the hashed name still starts with
`geos_c`, so the assertion passes. This is the historically fragile part of shapely+PyInstaller
(issues [#6745](https://github.com/pyinstaller/pyinstaller/issues/6745),
[#2834](https://github.com/pyinstaller/pyinstaller/issues/2834),
[shapely#1326](https://github.com/shapely/shapely/issues/1326)) — **smoke-test a real run on
Windows, not just `--help`**, because `--help` never touches GEOS.

**Cross-compilation is not possible.** PyInstaller's docs: *"If you need to distribute your
application for more than one OS … you must install PyInstaller on each platform and bundle your app
separately on each."* So the Windows exe must be built on Windows — in practice a
`runs-on: windows-latest` GitHub Actions job. PyInstaller 6.22.3 (2026-09-12) declares
`requires-python <3.16,>=3.8` and classifiers through 3.15, so Python 3.13/3.14 on the runner is fine.

**SmartScreen / antivirus.** This is the real cost of the Windows exe, not the build:
- Unsigned executables have no reputation, so **SmartScreen shows the "Windows protected your PC"
  interstitial** on first run on each machine. Users must click *More info → Run anyway*.
- PyInstaller `--onefile` binaries are structurally indistinguishable from a self-extracting packer
  and are routinely flagged. `--onedir` "produces a folder with dependencies, and since nothing
  needs to be extracted at runtime, the behavior looks less suspicious to AV software" — **prefer
  `--onedir`** for this reason as well as for the 3× faster startup measured above.
- Code signing "dramatically reduc[es] false positives, especially with Windows Defender and
  SmartScreen", but "a valid code signing certificate does not stop antivirus detection" outright —
  Defender layers ML/heuristics on top. A standard OV code-signing certificate still needs to build
  reputation over time; an **EV certificate** gets immediate SmartScreen trust but costs
  meaningfully more and requires a hardware token / cloud HSM.
- Free mitigations: submit the binary to Microsoft's Security Intelligence developer
  false-positive portal; on a managed fleet, add a Defender path/publisher exclusion by GPO/Intune
  for the install directory. **On an internal fleet with central management, the GPO exclusion is
  usually cheaper and faster than buying a certificate.**

**Updates:** there is no update mechanism. You re-copy the folder from the share, or wrap it in a
`.msi`/Chocolatey/winget package, or add a "check for updates" ping in the app. Simplest workable
thing on a managed fleet: publish `\\share\tools\stencicrity\` and have a login script rsync it.

**Effort:** 1–2 days first time (CI workflow on `windows-latest`, `.spec` tuning, the font fix, a
real end-to-end test in a Windows console, SmartScreen/AV triage). **Maintenance:** ~30 min per
release once the CI job exists, plus re-testing whenever Windows Defender's heuristics shift.

### 5.2 Nuitka

- Version 4.2.1 (2026-09-05), actively developed (last push 2026-09-13, 15.1k stars), classifiers
  through 3.14. **Licensed AGPL-3.0** (both the GitHub repo's declared licence and the PyPI
  `License ::` classifier). The commonly quoted "Apache 2.0" refers to contributions back to the
  author, not the distribution licence. The AGPL applies to *Nuitka itself*; Nuitka Commercial is a
  separate paid product for obfuscation/constant-hiding. Using AGPL Nuitka as a build tool for an
  internal, non-distributed application is normally uncontroversial, but **run it past whoever owns
  licence policy** before adopting — this is the kind of thing that turns into a procurement
  conversation, and it is a real differentiator against PyInstaller (GPL-2.0-or-later *with* a
  well-known exception for the bundled application).
- Build requirements: a real C compiler. On Windows, MSVC (Visual Studio Build Tools) or MinGW-w64
  — Nuitka will offer to download a MinGW toolchain. On Arch, `gcc` (present). Build times are
  **minutes, not seconds** (versus PyInstaller's 5–7 s here) because it actually compiles the Python
  to C.
- Payoff: smaller and faster-starting binaries than PyInstaller, and genuinely compiled code. For a
  tool whose runtime is dominated by shapely/GEOS geometry work, the startup win (a fraction of a
  second) is not worth the toolchain.
- **Verdict: not worth it here.** Same "must build on each OS", same AV/SmartScreen exposure, plus a
  compiler dependency, minutes-long builds and an AGPL conversation.

### 5.3 PyApp

`ofek/pyapp` — a Rust launcher that, by default, **bootstraps at runtime**: first run downloads uv
and a CPython distribution and materialises ~122 MB into `~/.local/share/pyapp`, and **fails
outright with no network**. `PYAPP_DISTRIBUTION_EMBED=1` (since v0.1.55) embeds the distribution at
build time and makes the artifact genuinely standalone at the cost of size. It also requires a Rust
toolchain to build. For a fleet where you control the machines, this is strictly more machinery than
`uv tool install` (which does the same bootstrap, better documented) or a PyInstaller folder.
**Verdict: skip.**

### 5.4 PyOxidizer

**Effectively dead.** The maintainer (indygreg) has stated in the project's own issue tracker that
shifting priorities have de-prioritised it and its future is "uncertain, possibly dead"; latest
documented release is 0.23.0 and he has invited someone to take ownership. The useful half of it —
`python-build-standalone` — was transferred to Astral and now powers uv's managed Pythons.
**Verdict: do not adopt.**

### 5.5 shiv / pex / zipapp

All three produce a zipapp-style artifact that **still needs a Python interpreter on the target**,
which is exactly the problem on Windows.

- **`zipapp`** (stdlib): only genuinely works for pure-Python dependency trees. shapely, numpy and
  pillow are all compiled extensions that cannot be imported from inside a zip — you'd need
  `zipapp` + site-packages on the side, at which point you have reinvented a worse venv.
- **`shiv`** 1.0.8, released **2024-11-01**, classifiers stop at 3.11 — **stale**, and a hard no for
  a Python 3.14 target. It unpacks wheels to `~/.shiv` at first run, so it does handle compiled
  deps, but the wheels it bundles are platform-specific: one `.pyz` per OS anyway.
- **`pex`** 2.102.0 (2026-09-13), classifiers through 3.15 — actively maintained and the strongest
  of the three. It can build multi-platform PEXes if you feed it the right wheels. But it still
  requires a Python on the target, and on Windows PEX support has historically been the weakest
  part of the story.

**Verdict:** none of these solve the Windows problem, and on Arch `uv tool install` is better in
every dimension. Skip.

### 5.6 cx_Freeze

8.7.0 (2026-08-22), `requires-python <3.16,>=3.10`, classifiers through 3.15 — healthy and current.
Its distinguishing feature versus PyInstaller is first-class **MSI generation on Windows**
(`bdist_msi`), which matters if the fleet is managed by Intune/SCCM and you want a real installer
rather than a folder on a share. Downsides: a smaller hook ecosystem than
`pyinstaller-hooks-contrib` (so shapely's GEOS DLLs and `_curses.pyd` are more likely to need
explicit `include_files`/`packages` entries), and the same build-on-each-OS constraint.
**Verdict: worth a look *only* if you specifically need an MSI.** Otherwise PyInstaller `--onedir`
plus a copy job is less work.

### 5.7 Briefcase

0.4.5 (2026-09-08), `requires-python >=3.11`, classifiers through 3.15 — actively developed (BeeWare).
It is designed for **GUI applications** — it produces signed MSIs, app bundles, and store-ready
packages, and assumes a windowed app with an icon, an installer and app metadata. A terminal curses
TUI is the one shape it is least suited to: Briefcase's Windows app template builds a windowed
binary and wiring a console app through it is fighting the tool. **Verdict: wrong tool.**

### 5.8 Does the curses TUI actually work in a Windows console?

Summarising 1.4/1.5, and flagging what I could not test:

| Concern | Status |
|---|---|
| `_curses` available | ✅ via `windows-curses` 2.4.2 (cp313 + cp314 wheels), or baked into a PyInstaller build |
| Wide chars / UTF-8 output | ✅ PDCurses built with `PDC_WIDE` + UTF-8 forced |
| `use_default_colors()` + `init_pair(n, fg, -1)` | ✅ PDCurses implements both (wincon: supported) |
| `KEY_RESIZE` | ✅ auto `resize_term(0,0)` since windows-curses 2.0 — **but** open issue #74 reports resize crashes in comparable apps. **Test this first.** |
| `A_DIM` | ⚠️ **no-op** (`#define A_DIM A_NORMAL`) — ignored/closed pads lose their dimmed look |
| `A_BOLD`, `A_REVERSE` | ✅ mapped to console attributes |
| Glyphs `· → ↑ ↓ ← × ● … —` | ✅ present in Cascadia Mono (Windows Terminal default) |
| Glyph `⌀` (U+2300) | ⚠️ **not in Cascadia Mono**; relies on DirectWrite font fallback. Likely fine in Windows Terminal, likely a box in legacy conhost with a raster font. *Unverified.* |
| Windows Terminal vs conhost | Windows Terminal is the safe target. Legacy `conhost` (`cmd.exe` direct) has no font fallback worth trusting and a limited palette. Document "run it in Windows Terminal". |
| Windows-on-ARM | ❌ no `win_arm64` windows-curses wheel (issue #85). A PyInstaller build on an ARM runner would hit the same wall. |

---

## 6. Option 5 — cross-cutting

### 6.1 Python version pinning strategy

- **Arch's `python` is 3.14.7 today** (verified on this box). Arch tracks upstream closely and will
  move to 3.15 within months of its release, rebuilding every `python-*` package as it goes.
- Consequences per channel:
  - *pacman/AUR:* immune. The Arch maintainers rebuild `python-shapely` et al.; you rebuild
    `stencicrity` once (bump `pkgrel`) because of the versioned `site-packages` path.
  - *pipx against the system interpreter:* **breaks** on the bump (dangling venv symlink);
    `pipx reinstall-all` recovers.
  - *uv tool with a uv-managed Python:* immune — uv owns the interpreter and nothing changes under it.
  - *PyInstaller:* immune — the interpreter is inside the bundle.
- **Recommendation:** pin the *build* to a mainstream, well-supported version rather than the
  bleeding edge. Build the Windows exe on **Python 3.13** (every dependency, including
  `windows-curses`, has had cp313 wheels for over a year and a half) and install on Arch with
  `uv tool install --python 3.13`. Keep `requires-python = ">=3.11"` in the metadata so nothing
  artificially excludes older boxes, but do not *ship* on 3.14 until shapely has shipped a release
  newer than 2025-09-24. Revisit when shapely 2.2 lands.
- The current `requires-python = ">=3.11"` is probably untrue-in-practice: `numpy>=2.5` requires
  ≥3.12, so a 3.11 install silently resolves an older numpy that nobody tests. Either raise the
  floor to `>=3.12` or add a lower bound on numpy.

### 6.2 3.14 wheel compatibility on Windows — summary

All four required distributions have cp314 win_amd64 wheels **today**: shapely 2.1.2, numpy 2.5.3,
pillow 12.3.0, windows-curses 2.4.2. So a Python 3.14 Windows install works right now with zero
compilation. The residual risks are (a) shapely's slow release cadence as CPython 3.15 approaches,
and (b) windows-curses's declared unmaintained status and its historical 2–5 month lag behind each
new CPython.

### 6.3 One-command install/update, per OS

| | Install | Update |
|---|---|---|
| **Arch (recommended)** | `uv tool install --python 3.13 git+ssh://git@github.com/<org>/stencicrity` | `uv tool upgrade stencicrity` |
| **Arch (custom repo)** | one-time `/etc/pacman.conf` stanza, then `sudo pacman -S stencicrity` | **`sudo pacman -Syu`** (rides an existing habit) |
| **Windows (fast path)** | `winget install --id=astral-sh.uv -e` then `uv tool install git+…` | `uv tool upgrade stencicrity` |
| **Windows (recommended)** | copy `\\share\tools\stencicrity\` locally, run `stencicrity.exe` | re-copy the folder / login-script rsync |

### 6.4 Where to host artifacts

- **Code:** a private GitHub (or internal GitLab) repo. Non-negotiable prerequisite for everything.
- **Windows exe:** GitHub Releases (built by the `windows-latest` CI job) *and* mirrored to the
  internal file share, because the share is what the team will actually reach for and what works
  without a GitHub login. GitHub Releases gives you an immutable, versioned URL for free.
- **Arch packages:** the same internal share, served over HTTP, with `repo-add` maintaining the db
  — buildable from the same CI on a Linux runner.
- One repo, one tag, one CI run producing all three artifacts is the endgame; it is also roughly
  a day of work on top of everything else.

### 6.5 Code changes the tool needs before *any* of this

Ordered by necessity. None of these are implemented — this is the list, not the patch.

**Blocking — Windows will not start without these:**

1. **`pyproject.toml`: conditional Windows dependency.**
   `"windows-curses>=2.4.2; sys_platform == 'win32'"`. Without it, every install on Windows
   produces `ModuleNotFoundError: No module named '_curses'`.
2. **`cli.py:388`: make the TUI import lazy for real.** `from .tui import run_tui` sits at the top
   of `_run()` and executes unconditionally — including under `--batch`, which never uses the TUI.
   Move it inside the `if not args.batch:` branch at line 485, and wrap it so a missing `_curses`
   produces an actionable message ("install windows-curses, or use --batch") instead of a traceback.
   This also makes `--batch` usable on a Windows box with no curses at all.

**Blocking for publishing (PyPI or AUR), not for running:**

3. **Add a `LICENSE` file** and a `license` field. The AUR PKGBUILD needs `license=()` and an
   `install -Dm644 LICENSE …`; PyPI needs the metadata; your own team needs to know what the terms
   are. There is no LICENSE in the project today.
4. **Put the project under git and host it.** Everything in sections 2–5 depends on it.
5. **Single-source the version.** `0.1.0` currently appears in both `pyproject.toml` and
   `pcbstencil/__init__.py`. Use `dynamic = ["version"]` +
   `[tool.setuptools.dynamic] version = {attr = "pcbstencil.__version__"}` so a release tag cannot
   drift from `--version`.
6. **Flesh out `[project]`:** `authors`, `urls` (Homepage/Source), `classifiers`, `keywords`.
   `python -m build` works without them; humans and package managers don't.

**Correctness on Windows (cosmetic but visible):**

7. **`render.py:65` `_FONT_PATHS`:** add Windows fonts — `"C:/Windows/Fonts/arial.ttf"` /
   `"segoeui.ttf"` (Pillow's Windows fallback searches only `%WINDIR%\fonts`, and DejaVu is not
   there). Or ship `DejaVuSans.ttf` as package data and look it up relative to `__file__`
   (also fixes the frozen-build case, where no `.ttf` is bundled — verified above). Without this,
   the `—` in the *"DOES NOT FIT"* legend renders as a `.notdef` box on Windows, because Pillow's
   bundled default font has no U+2014 (measured).
8. **`tui.py`: degrade the `⌀` glyph.** U+2300 is absent from Cascadia Mono. Pick it from a small
   table based on `sys.stdout.encoding` / a `--ascii` flag, falling back to `dia ` or `D`.
   Cheap insurance against legacy conhost.
9. **`tui.py`: don't rely on `A_DIM` alone** to mark ignored pads — it is `A_NORMAL` under PDCurses.
   The colour pair already differentiates them; consider a leading marker character
   (e.g. `-`/`x`/`o` per state) so the distinction survives both a monochrome terminal and Windows.
10. **Console encoding:** all file I/O already passes `encoding="utf-8"` explicitly (good), but
    `print()` to stdout/stderr in `cli.py` uses the console encoding. Python 3.6+ on Windows uses
    UTF-16 for the console via `_WindowsConsoleIO` so interactive output is fine; a **redirected**
    stdout (`stencicrity > log.txt`) uses the ANSI code page and will raise
    `UnicodeEncodeError` on `·`/`—`/`⌀`. Either set `PYTHONIOENCODING`/`sys.stdout.reconfigure(
    encoding="utf-8", errors="replace")` early in `main()`, or keep non-ASCII out of `print()`.
11. **`render.py:626` `os.startfile`** — already correct, no change needed. Worth noting that in a
    frozen build it opens the PNG with whatever is registered for `.png`; if nothing is, it raises
    `OSError`, which the existing `except Exception: return False` already swallows. Fine.

**Nice to have:**

12. A `tests/` directory and a CI smoke test (`--version`, `--help`, and a headless `--batch` run
    against one of the sample zips) — this is what makes the release pipeline trustworthy, and there
    is currently nothing to run in CI.
13. `tui.py` already notes it keeps the fit-line/footer logic free of curses ("footer key-help
    wrapping and the fit line do not touch curses") — that separation is what would let you add a
    non-curses fallback front-end later if windows-curses ever goes bad. Worth preserving
    deliberately.

---

## 7. Comparison table

| Option | Needs Python on target | Windows | Arch | Initial effort | Maintenance | Update story | Artifact size | Key blocker |
|---|---|---|---|---|---|---|---|---|
| **PyPI + uv/pipx** | yes (or uv fetches one) | ✅ (needs `windows-curses`) | ✅ | 3–5 h | ~15 min/release | `uv tool upgrade` (pull) | ~50 KB wheel + ~60 MB deps | must be **public**; no LICENSE; not in git |
| **git URL + uv/pipx** ⭐ | yes (or uv fetches one) | ✅ | ✅ | **1–2 h** | ~0 | `uv tool upgrade` (pull) | same | needs git on each box; SSH auth on Windows |
| **Private index** | yes | ✅ | ✅ | 1 day (1 h for `--find-links`) | you own a service | `uv tool upgrade` | same | infra ownership |
| **AUR (`stencicrity`)** | no (pacman deps) | ❌ | ✅ | 4–6 h + public repo | ~20 min/release + AUR comments | `yay -Syu` | ~50 KB | **must be public**; AUR account + SSH key |
| **AUR `-git`** | no | ❌ | ✅ | 4–6 h + public repo | ~0 (auto `pkgver()`) | `yay -Syu --devel` | ~50 KB | same |
| **`.pkg.tar.zst` by hand** | no | ❌ | ✅ | 2–3 h | rebuild per release **and** per Arch Python bump | manual `pacman -U` | ~50 KB | no update mechanism |
| **Custom pacman repo** ⭐ | no | ❌ | ✅ | 4–6 h (+2–4 h signing) | scriptable, per release | **`pacman -Syu`** (rides existing habit) | ~50 KB | needs internal HTTP host |
| **PyInstaller `--onedir`** ⭐ | **no** | ✅ | ✅ | 1–2 days | ~30 min/release | copy folder / login script | **100 MB** (measured) | build on Windows; SmartScreen/AV |
| **PyInstaller `--onefile`** | **no** | ✅ | ✅ | 1–2 days | ~30 min/release | copy one file | **39 MB** (measured) | worse AV profile, 3× slower start |
| **Nuitka** | **no** | ✅ | ✅ | 2–4 days | high | copy | smaller than PyInstaller | **AGPL-3.0**; needs MSVC/MinGW; minutes-long builds |
| **cx_Freeze** | **no** | ✅ (+MSI) | ✅ | 2–3 days | medium | MSI via Intune/SCCM | similar to PyInstaller | thinner hook ecosystem (shapely DLLs, `_curses.pyd` likely manual) |
| **PyApp** | no (bootstraps) | ✅ | ✅ | 1–2 days | medium | rebuild | 122 MB unpacked, or embedded | needs Rust; network-dependent unless `PYAPP_DISTRIBUTION_EMBED=1` |
| **PyOxidizer** | no | — | — | — | — | — | — | **de-prioritised / possibly dead** |
| **shiv** | **yes** | ✅ | ✅ | 1 day | low | copy `.pyz` | ~60 MB | **stale** (last release 2024-11, no 3.12+ classifiers) |
| **pex** | **yes** | ~ | ✅ | 1 day | low | copy `.pex` | ~60 MB | still needs Python; weak Windows story |
| **zipapp** | **yes** | ❌ | ❌ | — | — | — | — | cannot import shapely/numpy/pillow from a zip |
| **Briefcase** | no | ✅ (MSI) | ✅ | 2–4 days | medium | MSI | large | designed for **GUI** apps, not a console TUI |

⭐ = recommended combination.

---

## 8. Suggested plan of record

| Phase | Work | Effort |
|---|---|---|
| 0 | `git init` + push to a private GitHub repo; add `LICENSE`; fill in `[project]` metadata; single-source the version | 2–3 h |
| 1 | Code changes 1, 2, 7, 8, 9, 10 from §6.5 (Windows survivability) | 4–8 h |
| 2 | Document `uv tool install git+…` for both OSes — the whole team can run the tool from here | 1–2 h |
| 3 | GitHub Actions: `windows-latest` job → PyInstaller `--onedir` → upload to a GitHub Release and mirror to the share; Defender path exclusion by GPO | 1–2 days |
| 4 | Linux job in the same workflow → `makepkg` → `repo-add` → rsync to the internal HTTP repo; one-time `/etc/pacman.conf` stanza on each Arch box | 4–6 h |
| 5 | *Only if the tool goes public:* PyPI trusted publishing + an AUR package | +6–10 h |

Phases 0–2 alone get every machine in the fleet running the tool. Phases 3–4 are what make it feel
like a product rather than a checkout.

---

## Sources

Verified locally (this machine, 2026-09-15): `pacman -Q`/`-Si` for `python 3.14.7-2`,
`python-shapely 2.1.2-2`, `python-pillow 12.3.0-1`, `python-numpy 2.5.3-1`, `python-build 1.6.0-1`,
`python-installer 1.0.1-1`, `python-wheel 0.48.0-1`, `python-setuptools 1:84.0.0-1`,
`python-pipx 1.15.0-1`, `uv 0.12.13-1`; `makepkg 7.1.0`; `/usr/lib/python3.14/EXTERNALLY-MANAGED`;
`/etc/pacman.conf` lines 111–115; PyInstaller 6.22.3 builds (sizes, timings, bundle contents);
Pillow 12.3.0 `ImageFont.load_default` glyph coverage and `PIL/ImageFont.py:892-898`; unpacked
`shapely-2.1.2-cp314-cp314-win_amd64.whl` and `windows_curses-2.4.2-cp314-cp314-win_amd64.whl`;
Cascadia Code v2407.24 `cmap` parse.

- [PyPI JSON API: shapely](https://pypi.org/pypi/shapely/json), [numpy](https://pypi.org/pypi/numpy/json), [pillow](https://pypi.org/pypi/pillow/json), [windows-curses](https://pypi.org/pypi/windows-curses/json), [windows-curses 2.4.2](https://pypi.org/pypi/windows-curses/2.4.2/json)
- [windows-curses on GitHub](https://github.com/zephyrproject-rtos/windows-curses) — README ("Maintainers Wanted"), [setup.py](https://raw.githubusercontent.com/zephyrproject-rtos/windows-curses/main/setup.py), issues [#74](https://github.com/zephyrproject-rtos/windows-curses/issues/74), [#75](https://github.com/zephyrproject-rtos/windows-curses/issues/75), [#76](https://github.com/zephyrproject-rtos/windows-curses/issues/76), [#85](https://github.com/zephyrproject-rtos/windows-curses/issues/85)
- [PDCurses `pdcurses/color.c`](https://github.com/wmcbrine/PDCurses/blob/master/pdcurses/color.c), [`curses.h`](https://github.com/wmcbrine/PDCurses/blob/master/curses.h) (`#define A_DIM A_NORMAL`), [`wincon/pdcdisp.c`](https://github.com/wmcbrine/PDCurses/blob/master/wincon/pdcdisp.c), [PDCurses User's Guide](https://pdcurses.org/docs/USERS.html)
- [CPython `PC/layout/main.py`](https://github.com/python/cpython/blob/main/PC/layout/main.py) (stdlib `curses` package ships on Windows)
- [pyinstaller-hooks-contrib `hook-shapely.py`](https://github.com/pyinstaller/pyinstaller-hooks-contrib/blob/master/_pyinstaller_hooks_contrib/stdhooks/hook-shapely.py)
- [PyInstaller usage docs](https://pyinstaller.org/en/stable/usage.html) (no cross-compilation), issues [#6745](https://github.com/pyinstaller/pyinstaller/issues/6745), [#2834](https://github.com/pyinstaller/pyinstaller/issues/2834), [#8164](https://github.com/pyinstaller/pyinstaller/issues/8164), [shapely#1326](https://github.com/shapely/shapely/issues/1326)
- [Antivirus false positives with PyInstaller](https://www.pythonguis.com/faq/problems-with-antivirus-software-and-pyinstaller/), [Microsoft Q&A on PyInstaller AV flags](https://learn.microsoft.com/en-us/answers/questions/4078397/where-executables-created-by-pyinstaller-are-being), [signed EXE flagged as virus](https://my-ssl.com/learn/signed-exe-flagged-as-virus)
- [Nuitka on GitHub](https://github.com/Nuitka/Nuitka) (AGPL-3.0), [Nuitka commercial licence](https://nuitka.net/doc/commercial-license.html)
- [ofek/pyapp](https://github.com/ofek/pyapp), [astral-sh/python-build-standalone](https://github.com/astral-sh/python-build-standalone), [Astral: a new home for python-build-standalone](https://astral.sh/blog/python-build-standalone)
- [indygreg/PyOxidizer](https://github.com/indygreg/PyOxidizer) (de-prioritised)
- [uv: tools guide](https://docs.astral.sh/uv/guides/tools/), [uv: Python versions](https://docs.astral.sh/uv/concepts/python-versions/), [uv: installation](https://docs.astral.sh/uv/getting-started/installation/)
- [PyPI Trusted Publishers](https://docs.pypi.org/trusted-publishers/), [PyPI help (2FA, name rules)](https://pypi.org/help/), [Publishing with GitHub Actions](https://packaging.python.org/en/latest/guides/publishing-package-distribution-releases-using-github-actions-ci-cd-workflows/)
- Arch Wiki (raw): [Python](https://wiki.archlinux.org/title/Python) (PEP 668), [AUR submission guidelines](https://wiki.archlinux.org/title/AUR_submission_guidelines), [Pacman/Tips and tricks §Custom local repository](https://wiki.archlinux.org/title/Pacman/Tips_and_tricks), [DeveloperWiki:Building in a clean chroot](https://wiki.archlinux.org/title/DeveloperWiki:Building_in_a_clean_chroot), [namcap](https://wiki.archlinux.org/title/Namcap)
- [Arch developer manual — Python package guidelines](https://manual.archlinux.page/package-guidelines/python/); real PKGBUILDs from [`python-pipx`](https://gitlab.archlinux.org/archlinux/packaging/packages/python-pipx) and [`python-shapely`](https://gitlab.archlinux.org/archlinux/packaging/packages/python-shapely)
- [microsoft/cascadia-code releases](https://github.com/microsoft/cascadia-code/releases) (v2407.24), [Cascadia Code docs](https://learn.microsoft.com/en-us/windows/terminal/cascadia-code)
