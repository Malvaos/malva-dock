# Unreleased

- **Icons from the Malva icon theme.** The eleven built-in icons were hand-coded stroke glyphs on a
  gradient tile inside `ui/dock.slint`. They are now the theme's own SVGs (`assets/icons`), each one
  already drawn on its tile, so the dock looks like the rest of Malva OS. Names in `dock.conf` are
  unchanged. Calculator had no tile in the theme, so one was generated with the theme's tools
  (same recipe, sizing and centring rule). The letter-initial fallback for a custom entry without an
  image now sits on a plain Malva tile. See `assets/icons/README.md` for where each file comes from.

---

# Changelog: 0.1.0 → 0.2.0

Everything below was found by a full audit of the original sources, fixed, and locked
in with tests — the suite grew from **5 tests to 320** (every public function has at
least 5 tests). Run `cargo test` to see them all.

## Security fixes

1. **Regex injection into `hyprctl` (high).** `focus()` interpolated a window class
   straight into `class:^({class})$`. A class like `org.foo(bar)` or `a.b|c` from any
   application could change which window the selector matched, making the dock raise
   (or fail to raise) the wrong window. Classes are now escaped by
   `apps::escape_hypr_regex` before they reach `hyprctl dispatch`.
2. **Panic on NUL bytes in `exec` (crash / DoS).** A config line such as
   `exec = kitty\0anything` (NUL bytes are legal in a text file) reached
   `Command::arg`, which **panics** on interior NUL — one malformed line in the
   config file killed the whole dock. `apps::validate_exec` now rejects NUL bytes
   with a proper `LaunchError::InteriorNul`.
3. **World-writable config warning (privilege escalation surface).** The config file
   is a list of shell commands run as you. If another local user can write to it,
   they can run commands as you. `config::world_writable_warning` now detects this
   and the dock prints a loud warning with the exact `chmod o-w` fix.
4. **`unsafe` is now denied** across the crate (lib and bin). The only `allow` is
   scoped to Slint's generated vtables, which are upstream code.

## Bug fixes

5. **Window placement race on X11.** The dock read `window().size()` 250 ms after
   start-up to position itself. Before the first layout that size can be 0, placing
   the window wrong. The size is now computed from the config (mirroring the Slint
   formulas in `platform::window_logical_size`), scaled by the real scale factor for
   HiDPI, and clamped so an oversized window can never land at a negative
   coordinate (`platform::bottom_centre`).
6. **`--config` without a path was silently dropped**, and the dock started with the
   default config instead of telling you. It is now a usage error (message + help +
   exit code 2). Unknown options are also errors now instead of being ignored
   silently. `--config=PATH` spelling and `--version` were added.
7. **Malformed section headers were silently swallowed.** `[Apps] # comment` (no
   trailing `]`) was skipped without a word, so the icon just vanished. Header
   parsing is now strict and every drop is reported as a warning with its line
   number.
8. **`xrandr` parsing anchored.** The old parser split on the first `"current "` in
   the whole output; a monitor named `current-…` could hijack it. It now only reads
   lines that start with `Screen ` and checks `xrandr`'s exit status (previously
   ignored).
9. **`XDG_CONFIG_HOME` is validated.** A relative value used to be accepted, quietly
   pointing the dock at whatever directory it happened to start in; per the XDG
   basedir spec it is now ignored and the `~/.config` fallback is used.
10. **Out-of-range clicks could panic.** `State::activate` indexed the entry vector
    directly; a stray index from the UI layer would crash the dock. Both out-of-range
    and separator indices are now ignored.
11. **NaN / infinite `size` and `magnification`** (they parse fine from text like
    `NaN`, `inf`) flowed into the geometry and the Slint magnification formula,
    which would make icons vanish or the warp function produce `NaN`. All values are
    now sanitised (`config::sanitize_icon_size`, `config::sanitize_magnification`),
    in the parser *and* again at the geometry boundary.
12. **UTF-8 BOM** at the start of a config file made the first line (`size = …`)
    unparseable. It is stripped now.
13. **Empty `HYPRLAND_INSTANCE_SIGNATURE`** counted as "on Hyprland", causing a
    useless `hyprctl` spawn every poll. Set-but-empty now counts as off
    (`platform::env_flag`).
14. **Failed launches no longer hop.** The launch-hop animation used to play even
    when nothing was started (empty exec, spawn failure) — misleading feedback. The
    icon now only hops when a process was actually spawned, and failures are printed.
15. **`~` expansion edge cases.** A bare `~` was never expanded (now it is), a
    trailing slash on `$HOME` produced `//`, and home `/` produced empty paths.
    `platform::expand_home_with` handles all of these and is fully tested.

## Behaviour changes worth knowing

- Config parsing keeps `git config` comment semantics: `#` only starts a comment at
  the beginning of a line, so `exec = grep '#include' file` keeps working. Anything
  the parser has to skip is now a *warning* (with line number) instead of silence:
  `size = big`, unknown settings, malformed headers, separators with settings,
  entries that can do nothing, clamped values, bad themes.
- Clicking an entry with an unusable `exec` now explains itself on stderr instead of
  doing nothing silently.
- `theme = solarized` used to silently mean "dark"; it is still dark, but you get a
  warning now.

## Backend enhancements

- **Library/binary split.** The crate is now `src/lib.rs` (config, geometry, apps,
  state, platform) plus a thin `src/main.rs`. This is what makes the test suite
  possible at all: every function is reachable from tests, no display server needed.
- **Injectable seams everywhere.** `activate_with`, `detect_running_with`,
  `push_with_now`, `focus_with`, `process_names_from_proc`, `icon_for_with`,
  `expand_home_with`, `default_path_with`, `HyprClients::with_fetcher` — the pure
  core of every environment-touching function takes injected dependencies, and the
  wrappers stay one line thin.
- **Typed errors.** `apps::LaunchError` implements `Display` + `std::error::Error`;
  callers decide how to report.
- **Warning plumbing.** `config::load_detailed` returns every warning gathered from
  file permissions and parsing, so the binary prints them once at start-up.
- **`HyprClients` TTL cache** (750 ms) around `hyprctl clients`: the running-app poll
  reuses the list a focus call just fetched, instead of spawning `hyprctl` twice per
  interaction. Fetcher and clock are injected; the cache has its own tests.
- **Launched apps are detached** with `process_group(0)`: they no longer sit in the
  dock's process group, so terminal apps do not receive the dock's signals and
  outlive it cleanly. The reaper thread still prevents zombies.
- **Lint discipline.** `cargo clippy --all-targets` and `cargo fmt` are clean;
  `missing_docs` covers the public API; `unsafe_code` is denied in hand-written code.

## Testing

- **5 → 320 tests**: config 88, geometry 17, apps 89, platform 49, state 64, binary
  args 13. Every public function has ≥5 tests, including the env-dependent wrappers
  (serialised through a shared lock so parallel test threads cannot race the
  environment).
- Tests use a fake `/proc` tree, injected launchers/focusers/clock/fetchers, temp
  config files with real permission bits, a real embedded 1×1 PNG, and the real
  `/proc` for the process probe.
- CI (`.github/workflows/build.yml`) now runs `cargo fmt --check`, `cargo clippy
  --all-targets`, `cargo test` and the release build, and still attaches the Linux
  binary.

## Not changed on purpose

- `ui/dock.slint` is functionally untouched: the magnification maths, hit-testing
  invariant and visuals were audited and found sound. The Rust side now guarantees
  the invariants (finite sizes, clamped magnification) it depends on.
- The window title stays "Malva Dock" because `packaging/hyprland.conf` rules match
  on it.
