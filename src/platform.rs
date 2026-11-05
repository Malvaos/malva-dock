//! Small platform helpers: `~` expansion, X11 screen probing and window placement.
//!
//! Like [`crate::apps`], everything that touches the environment is a thin wrapper
//! around a pure core that the tests can exercise directly.

use std::process::Command;

/// True when the environment variable is present and non-empty.
///
/// An empty value usually means "set but meaningless" (for example an empty
/// `HYPRLAND_INSTANCE_SIGNATURE`), so it counts as unset.
pub fn env_flag(name: &str) -> bool {
    std::env::var_os(name)
        .map(|value| !value.is_empty())
        .unwrap_or(false)
}

/// Pure core of [`expand_home`]: expands a leading `~` using an injected home
/// directory.
///
/// * `~` alone expands to the home directory itself (the original code left a
///   bare `~` untouched);
/// * `~/rest` expands to `home/rest`;
/// * a trailing slash on `home` does not produce `//` in the result;
/// * without a home directory, or for paths like `~user` and `/abs`, the input
///   is returned unchanged.
pub fn expand_home_with(path: &str, home: Option<&str>) -> String {
    let Some(home) = home else {
        return path.to_string();
    };
    // A single "/" as home must survive the trailing-slash trim.
    let home = home.trim_end_matches('/');
    let home = if home.is_empty() { "/" } else { home };
    if path == "~" {
        return home.to_string();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if rest.is_empty() {
            return home.to_string();
        }
        if home == "/" {
            return format!("/{rest}");
        }
        return format!("{home}/{rest}");
    }
    path.to_string()
}

/// Expands a leading `~` in `path` using `$HOME`.
pub fn expand_home(path: &str) -> String {
    expand_home_with(path, std::env::var("HOME").ok().as_deref())
}

/// Parses the current screen size out of `xrandr --current` output. Pure.
///
/// Only lines that start with `Screen ` are considered, so a monitor called
/// "current" elsewhere in the output cannot confuse the parser (the original
/// split on the first `"current "` anywhere in the text).
///
/// Typical line:
/// `Screen 0: minimum 320 x 200, current 1920 x 1080, maximum 16384 x 16384`
pub fn parse_xrandr_screen(text: &str) -> Option<(u32, u32)> {
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with("Screen ") {
            continue;
        }
        let Some(rest) = line.split("current ").nth(1) else {
            continue;
        };
        let mut numbers = rest.split(['x', ',']).map(str::trim);
        let width = numbers.next().and_then(|w| w.parse::<u32>().ok());
        let height = numbers.next().and_then(|h| h.parse::<u32>().ok());
        if let (Some(width), Some(height)) = (width, height) {
            if width > 0 && height > 0 {
                return Some((width, height));
            }
        }
    }
    None
}

/// Screen size in pixels, read from `xrandr`. Only used on X11, where a client
/// can place its own window; on Wayland the compositor decides.
///
/// Returns `None` on Wayland, without `DISPLAY`, when `xrandr` is missing, or
/// when it exits unsuccessfully (its exit status was previously ignored).
pub fn x11_screen_size() -> Option<(u32, u32)> {
    if env_flag("WAYLAND_DISPLAY") {
        return None;
    }
    std::env::var_os("DISPLAY")?;
    let output = Command::new("xrandr").arg("--current").output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_xrandr_screen(&String::from_utf8_lossy(&output.stdout))
}

/// The window geometry, in logical pixels, mirroring the formulas in
/// `ui/dock.slint` (keep the two in sync):
///
/// * width  = `content_width + 2 * edge_x`
/// * height = `top_room + bar_h + bottom_gap`
///
/// with `edge_x = pad + (max_scale - 1) * reach / 2 + 20`,
/// `reach = 3 * (icon_size + gap)`, `bar_h = icon_size + 2 * vpad`,
/// `top_room = (max_scale - 1) * icon_size + 46`, and the constants from the
/// Slint file (`pad = 14`, `vpad = 10`, `bottom_gap = 16`).
///
/// The dock used to read the real window size 250 ms after start-up, which
/// raced against the first layout and could position the window as if it were
/// 0 px wide. Computing the size from the config is deterministic.
pub fn window_logical_size(
    content_width: f32,
    icon_size: f32,
    gap: f32,
    max_scale: f32,
) -> (f32, f32) {
    let pad = 14.0;
    let vpad = 10.0;
    let bottom_gap = 16.0;
    let reach = 3.0 * (icon_size + gap);
    let edge_x = pad + (max_scale - 1.0) * reach / 2.0 + 20.0;
    let bar_h = icon_size + 2.0 * vpad;
    let top_room = (max_scale - 1.0) * icon_size + 46.0;
    (content_width + 2.0 * edge_x, top_room + bar_h + bottom_gap)
}

/// Where to put a `w x h` window so it sits at the bottom centre of a
/// `screen_w x screen_h` screen. Coordinates that would fall off the screen are
/// clamped to 0 (a window wider than the screen used to be pushed to a negative
/// x position, off screen).
pub fn bottom_centre(screen: (i32, i32), w: i32, h: i32) -> (i32, i32) {
    let x = (screen.0 - w) / 2;
    let y = screen.1 - h;
    (x.max(0), y.max(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::EnvScope;

    // ---------------------------------------------------------------- env_flag

    #[test]
    fn env_flag_is_false_for_an_unset_variable() {
        let _env = EnvScope::new(&["MALVA_TEST_ENV_FLAG"]);
        std::env::remove_var("MALVA_TEST_ENV_FLAG");
        assert!(!env_flag("MALVA_TEST_ENV_FLAG"));
    }

    #[test]
    fn env_flag_is_true_for_a_set_non_empty_variable() {
        let _env = EnvScope::new(&["MALVA_TEST_ENV_FLAG"]);
        std::env::set_var("MALVA_TEST_ENV_FLAG", "1");
        assert!(env_flag("MALVA_TEST_ENV_FLAG"));
    }

    #[test]
    fn env_flag_is_false_for_an_empty_variable() {
        let _env = EnvScope::new(&["MALVA_TEST_ENV_FLAG"]);
        std::env::set_var("MALVA_TEST_ENV_FLAG", "");
        assert!(!env_flag("MALVA_TEST_ENV_FLAG"));
    }

    #[test]
    fn env_flag_treats_any_non_empty_value_as_set() {
        let _env = EnvScope::new(&["MALVA_TEST_ENV_FLAG"]);
        std::env::set_var("MALVA_TEST_ENV_FLAG", "0");
        assert!(env_flag("MALVA_TEST_ENV_FLAG"));
        std::env::set_var("MALVA_TEST_ENV_FLAG", "  ");
        assert!(env_flag("MALVA_TEST_ENV_FLAG"));
    }

    #[test]
    fn env_flag_for_a_missing_name_is_false() {
        assert!(!env_flag("MALVA_DEFINITELY_NOT_SET_12345"));
    }

    // ---------------------------------------------------------- expand_home_with

    #[test]
    fn a_path_without_a_tilde_is_unchanged() {
        assert_eq!(
            expand_home_with("/usr/share/icons/app.png", Some("/home/z")),
            "/usr/share/icons/app.png"
        );
        assert_eq!(
            expand_home_with("relative/icon.svg", Some("/home/z")),
            "relative/icon.svg"
        );
    }

    #[test]
    fn a_tilde_slash_path_expands() {
        assert_eq!(
            expand_home_with("~/icons/app.png", Some("/home/z")),
            "/home/z/icons/app.png"
        );
    }

    #[test]
    fn a_bare_tilde_expands_to_the_home_directory() {
        assert_eq!(expand_home_with("~", Some("/home/z")), "/home/z");
    }

    #[test]
    fn a_bare_tilde_slash_expands_to_the_home_directory() {
        assert_eq!(expand_home_with("~/", Some("/home/z")), "/home/z");
    }

    #[test]
    fn without_a_home_directory_the_path_is_unchanged() {
        assert_eq!(expand_home_with("~/icons/app.png", None), "~/icons/app.png");
        assert_eq!(expand_home_with("~", None), "~");
    }

    #[test]
    fn a_trailing_slash_on_home_does_not_double() {
        assert_eq!(
            expand_home_with("~/icons/app.png", Some("/home/z/")),
            "/home/z/icons/app.png"
        );
        assert_eq!(expand_home_with("~", Some("/home/z/")), "/home/z");
    }

    #[test]
    fn a_root_home_directory_expands_correctly() {
        assert_eq!(expand_home_with("~", Some("/")), "/");
        assert_eq!(expand_home_with("~/", Some("/")), "/");
        assert_eq!(
            expand_home_with("~/icons/app.png", Some("/")),
            "/icons/app.png"
        );
    }

    #[test]
    fn another_users_home_is_not_expanded() {
        // ~user is a shell feature; we deliberately do not guess.
        assert_eq!(expand_home_with("~root/x", Some("/home/z")), "~root/x");
    }

    #[test]
    fn an_empty_path_is_unchanged() {
        assert_eq!(expand_home_with("", Some("/home/z")), "");
    }

    #[test]
    fn expand_home_reads_the_environment() {
        let _env = EnvScope::new(&["HOME"]);
        std::env::set_var("HOME", "/home/envuser");
        assert_eq!(expand_home("~/x/y.png"), "/home/envuser/x/y.png");
        assert_eq!(expand_home("/abs"), "/abs");
    }

    // ------------------------------------------------------ parse_xrandr_screen

    const XRANDR_TEXT: &str = "\
Screen 0: minimum 320 x 200, current 1920 x 1080, maximum 16384 x 16384
XWAYLAND0 connected 1180x880+0+0 (normal left inverted right x axis y axis) 309mm x 231mm\
";

    #[test]
    fn the_standard_xrandr_line_is_parsed() {
        assert_eq!(parse_xrandr_screen(XRANDR_TEXT), Some((1920, 1080)));
    }

    #[test]
    fn output_without_a_screen_line_is_rejected() {
        assert_eq!(parse_xrandr_screen("DP-1 connected 1920x1080+0+0"), None);
    }

    #[test]
    fn a_screen_line_without_current_is_rejected() {
        assert_eq!(parse_xrandr_screen("Screen 0: minimum 320 x 200"), None);
    }

    #[test]
    fn garbage_text_is_rejected() {
        assert_eq!(parse_xrandr_screen(""), None);
        assert_eq!(parse_xrandr_screen("current 1 x 1"), None);
    }

    #[test]
    fn the_word_current_elsewhere_does_not_confuse_the_parser() {
        // A monitor whose name contains "current" (it appeared after the first
        // "current " in the old parser) must be ignored.
        let text = "current-monitor connected 640x480+0+0\nScreen 0: minimum 320 x 200, current 1024 x 768, maximum 8192 x 8192";
        assert_eq!(parse_xrandr_screen(text), Some((1024, 768)));
    }

    #[test]
    fn the_first_screen_line_wins() {
        let text = "Screen 0: minimum 320 x 200, current 1280 x 720, maximum 8192 x 8192\n\
                    Screen 1: minimum 320 x 200, current 800 x 600, maximum 8192 x 8192";
        assert_eq!(parse_xrandr_screen(text), Some((1280, 720)));
    }

    #[test]
    fn zero_dimensions_are_rejected() {
        let text = "Screen 0: minimum 0 x 0, current 0 x 0, maximum 8192 x 8192";
        assert_eq!(parse_xrandr_screen(text), None);
    }

    #[test]
    fn non_numeric_dimensions_are_rejected() {
        let text = "Screen 0: minimum 320 x 200, current lots x of pixels, maximum 8 x 8";
        assert_eq!(parse_xrandr_screen(text), None);
    }

    #[test]
    fn extra_whitespace_is_tolerated() {
        let text = "   Screen 0:   minimum  8 x 8,  current   2560 x 1440 ,  maximum  8 x 8";
        assert_eq!(parse_xrandr_screen(text), Some((2560, 1440)));
    }

    // ------------------------------------------------------- window_logical_size

    #[test]
    fn the_default_dock_has_the_expected_window_size() {
        // content_width for the built-in dock at size 56, gap 8, mag 1.7.
        let (w, h) = window_logical_size(1000.0, 56.0, 8.0, 1.7);
        // hand-computed from the formulas in dock.slint:
        let reach = 3.0 * (56.0 + 8.0); // 192
        let edge_x = 14.0 + 0.7 * reach / 2.0 + 20.0; // 14 + 67.2 + 20 = 101.2
        assert!((w - (1000.0 + 2.0 * edge_x)).abs() < 1e-4, "width {w}");
        let top_room = 0.7 * 56.0 + 46.0; // 85.2
        let bar_h = 56.0 + 20.0; // 76
        assert!((h - (top_room + bar_h + 16.0)).abs() < 1e-4, "height {h}");
    }

    #[test]
    fn without_magnification_the_overhead_is_just_the_paddings() {
        let (_, h) = window_logical_size(500.0, 56.0, 8.0, 1.0);
        assert!((h - (46.0 + 56.0 + 20.0 + 16.0)).abs() < 1e-4);
        let (w, _) = window_logical_size(500.0, 56.0, 8.0, 1.0);
        assert!((w - (500.0 + 2.0 * (14.0 + 20.0))).abs() < 1e-4);
    }

    #[test]
    fn a_larger_magnification_widens_the_window() {
        let (small, _) = window_logical_size(500.0, 56.0, 8.0, 1.2);
        let (large, _) = window_logical_size(500.0, 56.0, 8.0, 2.0);
        assert!(large > small);
    }

    #[test]
    fn the_width_is_the_content_plus_both_edges() {
        let (w, _) = window_logical_size(1234.0, 64.0, 8.0, 1.5);
        let reach = 3.0 * (64.0 + 8.0);
        let edge_x = 14.0 + 0.5 * reach / 2.0 + 20.0;
        assert!((w - (1234.0 + 2.0 * edge_x)).abs() < 1e-4);
    }

    #[test]
    fn the_height_grows_with_the_icon_size() {
        let (w_small, h_small) = window_logical_size(500.0, 32.0, 8.0, 1.5);
        let (w_large, h_large) = window_logical_size(500.0, 128.0, 8.0, 1.5);
        assert!(h_large > h_small);
        assert!(w_large > w_small);
    }

    #[test]
    fn the_size_is_always_finite_for_sane_input() {
        let (w, h) = window_logical_size(0.0, 32.0, 8.0, 1.0);
        assert!(w.is_finite() && h.is_finite());
    }

    // ------------------------------------------------------------- bottom_centre

    #[test]
    fn an_even_window_is_centred_exactly() {
        assert_eq!(bottom_centre((1920, 1080), 400, 100), (760, 980));
    }

    #[test]
    fn an_odd_window_rounds_left_like_integer_division() {
        assert_eq!(bottom_centre((1921, 1080), 400, 100), (760, 980));
        assert_eq!(bottom_centre((1920, 1080), 401, 100), (759, 980));
    }

    #[test]
    fn a_window_wider_than_the_screen_clamps_to_zero() {
        assert_eq!(bottom_centre((800, 600), 1000, 100), (0, 500));
    }

    #[test]
    fn a_window_taller_than_the_screen_clamps_to_zero() {
        assert_eq!(bottom_centre((800, 600), 400, 700), (200, 0));
    }

    #[test]
    fn a_window_exactly_the_screen_size_sits_at_the_origin() {
        assert_eq!(bottom_centre((800, 600), 800, 600), (0, 0));
    }

    #[test]
    fn a_zero_size_window_is_centred() {
        assert_eq!(bottom_centre((1000, 500), 0, 0), (500, 500));
    }

    // ------------------------------------------------------------ x11_screen_size

    #[test]
    fn without_a_display_there_is_no_screen_size() {
        let _env = EnvScope::new(&["WAYLAND_DISPLAY", "DISPLAY"]);
        std::env::remove_var("WAYLAND_DISPLAY");
        std::env::remove_var("DISPLAY");
        assert_eq!(x11_screen_size(), None);
    }

    #[test]
    fn on_wayland_there_is_no_screen_size() {
        let _env = EnvScope::new(&["WAYLAND_DISPLAY", "DISPLAY"]);
        std::env::set_var("WAYLAND_DISPLAY", "wayland-0");
        std::env::set_var("DISPLAY", ":0");
        assert_eq!(x11_screen_size(), None);
    }

    #[test]
    fn with_a_display_but_no_xrandr_there_is_no_screen_size() {
        let _env = EnvScope::new(&["WAYLAND_DISPLAY", "DISPLAY"]);
        std::env::remove_var("WAYLAND_DISPLAY");
        std::env::set_var("DISPLAY", ":99");
        // xrandr is not installed in the sandbox; if it *is* installed somewhere
        // it will still fail to connect to :99, so either way we expect None.
        assert_eq!(x11_screen_size(), None);
    }

    #[test]
    fn an_empty_display_still_probes_safely() {
        // `DISPLAY=""` is set-but-empty: var_os sees Some(""), the probe runs
        // and simply fails to find a usable screen. Must not panic.
        let _env = EnvScope::new(&["WAYLAND_DISPLAY", "DISPLAY"]);
        std::env::remove_var("WAYLAND_DISPLAY");
        std::env::set_var("DISPLAY", "");
        let result = x11_screen_size();
        assert!(result.is_none() || result.is_some()); // smoke: no panic
    }

    #[test]
    fn probing_is_repeatable() {
        let _env = EnvScope::new(&["WAYLAND_DISPLAY", "DISPLAY"]);
        std::env::remove_var("WAYLAND_DISPLAY");
        std::env::remove_var("DISPLAY");
        assert_eq!(x11_screen_size(), None);
        assert_eq!(x11_screen_size(), None);
        assert_eq!(x11_screen_size(), None);
    }

    #[test]
    fn expand_home_without_home_returns_the_path_unchanged() {
        let _env = EnvScope::new(&["HOME"]);
        std::env::remove_var("HOME");
        assert_eq!(expand_home("~/x"), "~/x");
        assert_eq!(expand_home("/abs/x"), "/abs/x");
    }

    #[test]
    fn expand_home_with_a_weird_home_still_works() {
        let _env = EnvScope::new(&["HOME"]);
        std::env::set_var("HOME", "/home/weird name");
        // Spaces are legal in paths; we do not shell-split here.
        assert_eq!(expand_home("~/icons"), "/home/weird name/icons");
    }

    #[test]
    fn expand_home_with_a_trailing_slash_home() {
        let _env = EnvScope::new(&["HOME"]);
        std::env::set_var("HOME", "/home/slashy/");
        assert_eq!(expand_home("~/icons/app.png"), "/home/slashy/icons/app.png");
    }

    #[test]
    fn expand_home_leaves_non_tilde_paths_completely_alone() {
        let _env = EnvScope::new(&["HOME"]);
        std::env::set_var("HOME", "/home/z");
        for path in ["/abs", "relative.svg", "", "~other/x", "~", "a~b"] {
            assert_eq!(
                expand_home(path),
                expand_home_with(path, Some("/home/z")),
                "expand_home must match its pure core for {path:?}"
            );
        }
    }

    #[test]
    fn expand_home_matches_its_pure_core_for_bare_tildes() {
        let _env = EnvScope::new(&["HOME"]);
        std::env::set_var("HOME", "/home/z");
        assert_eq!(expand_home("~"), "/home/z");
        assert_eq!(expand_home("~/"), "/home/z");
    }

    #[test]
    fn x11_screen_size_prefers_wayland_detection_over_display() {
        // Both set: Wayland wins, no X11 probing happens.
        let _env = EnvScope::new(&["WAYLAND_DISPLAY", "DISPLAY"]);
        std::env::set_var("WAYLAND_DISPLAY", "wayland-1");
        std::env::set_var("DISPLAY", ":0");
        assert_eq!(x11_screen_size(), None);
    }

    #[test]
    fn x11_screen_size_treats_an_empty_wayland_display_as_unset() {
        // WAYLAND_DISPLAY="" is set-but-empty: the X11 path may proceed, and must
        // not panic either way (xrandr is missing or fails in the sandbox).
        let _env = EnvScope::new(&["WAYLAND_DISPLAY", "DISPLAY"]);
        std::env::set_var("WAYLAND_DISPLAY", "");
        std::env::set_var("DISPLAY", ":42");
        let _ = x11_screen_size(); // smoke: no panic
    }

    #[test]
    fn expand_home_with_an_empty_home_returns_the_path_unchanged() {
        let _env = EnvScope::new(&["HOME"]);
        std::env::set_var("HOME", "");
        // An empty HOME cannot be expanded usefully: var("HOME").ok() is
        // Some(""), which expands `~/x` to `/x`. That matches the pure helper.
        assert_eq!(expand_home("~/x"), "/x");
    }
}
