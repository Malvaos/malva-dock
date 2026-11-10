# Dock icons

The dock's built-in icons are the Malva icon theme (`Malva`, one scalable SVG per icon, 1024 x 1024
viewBox). Slint embeds these files in the binary at build time, so nothing has to be installed next to
the dock. `ui/dock.slint` (the `Icons` global) maps each name used in `dock.conf` to one of them.

| `icon =` name | File | Theme folder / name | Notes |
|---|---|---|---|
| `apps` | `launcher.svg` | `scalable/apps/launcher` | Malva Launcher |
| `files` | `files.svg` | `scalable/apps/files` | Malva Files |
| `browser` | `applications-internet.svg` | `scalable/categories/applications-internet` | the theme has no browser app icon yet; the Internet tile stands in |
| `notes` | `text-editor.svg` | `scalable/apps/text-editor` | the theme has no Notes icon yet; Text Editor stands in |
| `calculator` | `calculator.svg` | generated, see below | the theme has no Calculator tile |
| `fonts` | `preferences-desktop-font.svg` | `scalable/categories/preferences-desktop-font` | |
| `terminal` | `terminal.svg` | `scalable/apps/terminal` | Malva Terminal |
| `keyboard` | `preferences-desktop-keyboard.svg` | `scalable/categories/preferences-desktop-keyboard` | on-screen keyboard |
| `settings` | `settings.svg` | `scalable/apps/settings` | Malva Settings |
| `downloads` | `folder-download.svg` | `scalable/places/folder-download` | a folder object, not a tile, like macOS's Downloads |
| `trash` | `user-trash.svg` | `scalable/places/user-trash` | the empty bin; the dock does not show a full state |
| (custom entry, no image) | `tile.svg` | generated, see below | the plain tile behind the letter initial |

Everything except `calculator.svg` and `tile.svg` is copied unchanged from the theme.

## Generated files

`calculator.svg` and `tile.svg` were made with the theme's own `tools/icons/appfmt.py` and `tiles.py`,
so they follow the same recipe as the shipped tiles (squircle, two-stop gradient, highlight, lilac
symbol from the kit's `accessories-calculator` glyph on the 24 grid, stroke 2.3).

* **Check that the tools were run correctly:** regenerating the theme's `preferences-desktop-keyboard`
  tile with the same setup reproduces the shipped file byte for byte. (The tools read `CORNER_EXPONENT`
  from `crates/malva-tokens/src/balance.rs` of the main Malva repository, which was not at hand; 4.0 was
  used because it is the value that reproduces the shipped tiles exactly, and 3.0, 3.5, 4.5 and 5.0 do not.)
* **Calculator:** hue 28 (the theme's red; no other dock icon uses it), symbol sized to 0.52 of the tile
  by the theme's rule (scale 1.092), shift (0, 0.8) from the theme's centring rule; the symbol's box sits
  within 0.2 % of the middle of the tile.
* **Tile:** the Malva brand plum gradient (`#A24E8C` to `#4E1D42`) with no symbol.

If the theme gains a Calculator, Notes or Browser icon, drop the new file in here and point the matching
line of the `Icons` global at it.
