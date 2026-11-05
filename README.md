# Malva Dock

The dock for Malva OS: a macOS-style dock written in Rust with [Slint](https://slint.dev).

- Fisheye magnification: the icon under the pointer grows and its neighbours make room, so nothing overlaps
- Name label above the hovered icon, a dot under running apps, and a hop while an app starts
- Eleven built-in Malva icons (one glyph on a gradient tile), or your own SVG / PNG files
- Dark and light themes; black-and-white glass with the Malva palette on the icons
- Click an icon to start the app, or to raise it when it is already running (Hyprland, or X11 with `wmctrl`)
- Pinned apps, icon size and magnification come from a small config file
- A hardened backend: every config problem is reported with a line number, Hyprland window classes are
  regex-escaped, NUL bytes and non-finite numbers are rejected, and 320 unit tests keep it that way
  (see [CHANGELOG.md](CHANGELOG.md) for everything that was fixed in 0.2.0)

## Build and run

Debian / Ubuntu:

```sh
sudo apt install build-essential pkg-config libfontconfig1-dev libxkbcommon-dev
cargo run --release
```

Fedora: `sudo dnf install gcc pkgconf-pkg-config fontconfig-devel libxkbcommon-devel`
Arch: `sudo pacman -S base-devel fontconfig libxkbcommon`

`cargo test` runs the 320 unit tests (config parsing, geometry, hit-testing, process probing,
launching, focusing, the launch-hop state machine and the CLI). Without a local toolchain, push the
project to GitHub: `.github/workflows/build.yml` lints, tests, builds it and attaches a Linux binary
to the run.

```
--config PATH   read this config file instead of ~/.config/malva/dock.conf
--light / --dark
--print-config  print the built-in config
--version       print the version
-h, --help      show the help
```

Unknown options and a valueless `--config` exit with code 2 after printing the usage.

## Configure

```sh
mkdir -p ~/.config/malva
malva-dock --print-config > ~/.config/malva/dock.conf
```

The file is commented. Every `[Section]` is one icon, in order; `[separator]` draws a divider.

```ini
size = 56
magnification = 1.7
theme = dark

[Terminal]
icon = terminal
exec = kitty || alacritty || foot
```

`exec` is a shell command, and `a || b` starts `b` when `a` is not installed. `icon` is a built-in name
(`apps files browser notes calculator fonts terminal keyboard settings downloads trash`) or the path to an
SVG or PNG (`~` is expanded). Running apps are found by process name (or window class on Hyprland); `match`
overrides the names, and an empty `match =` never shows a dot.

Comments follow `git config` rules: `#` only starts a comment at the start of a line, so shell values like
`grep '#include' file` keep their `#`. Anything the parser has to ignore or clamp is printed as a warning
with its line number when the dock starts.

## Using it as a real dock

A Slint window is a normal window, so the dock needs a little help from the session.

- **X11:** it places itself at the bottom centre of the screen and stays on top.
- **Hyprland:** add the rules in `packaging/hyprland.conf` (float, pin, never take focus, bottom centre).
  Rule syntax changes between Hyprland versions, so adjust a line if yours is rejected.
- **Other Wayland compositors:** use a window rule for the window titled "Malva Dock" (floating, always on
  top, never focus). Apps cannot position themselves on Wayland, and Slint has no layer-shell support, so a
  layer-shell back-end is the proper long-term fix.
- **Autostart:** run `malva-dock` from your session start-up, for example `exec-once = malva-dock` on
  Hyprland, or copy `packaging/malva-dock.desktop` to `~/.config/autostart/`.

## Customising

| To change                        | Edit                                                         |
| -------------------------------- | ------------------------------------------------------------ |
| Palette, bar colours, font       | the `Theme` global at the top of `ui/dock.slint`             |
| Built-in icons and tile colours  | the `Icons` global in `ui/dock.slint` (glyphs are SVG paths on a 24x24 grid) |
| How far magnification reaches    | `reach` in `DockWindow`                                      |
| Spacing                          | `GAP` in `src/geometry.rs`                                   |
| Default apps                     | `packaging/dock.conf`                                        |

## How the magnification works

Every resting position `u` is moved away from the pointer by a smooth, monotonic function of its distance.
The point under the pointer stays where it is, so a click can be resolved against the resting layout
(`geometry::hit`) with no need to know where the icons ended up. Icon sizes come from the same function, so
the gaps scale with the icons. All of it is a closed-form expression in `ui/dock.slint`.

## Known limits

- The dock does not reserve screen space, so windows can cover it.
- Slint has no backdrop blur, so the glass bar is translucent but not frosted.
- The transparent area around the dock (about 100 px each side and 85 px above) belongs to the window and
  blocks clicks to whatever is underneath.
- Raising a running app works on Hyprland and on X11 with `wmctrl`. Elsewhere a click starts a new instance.
- No drag-to-reorder, right-click menu or window previews yet.
