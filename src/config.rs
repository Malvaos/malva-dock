//! Dock settings and pinned apps, read from a small INI-style file.
//!
//! The parser is deliberately forgiving (a typo never stops the dock from starting)
//! but everything it has to ignore is reported back as a warning, so
//! [`load_detailed`] gives the caller a precise list of problems in the user's file.
//!
//! Comment rules follow `git config` semantics: `#` starts a comment only at the
//! beginning of a line (after trimming). Trailing `# comment` after a value is
//! *not* stripped, because `exec` is a shell command in which `#` is a legal
//! character (for example `grep '#include' file`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Used when the user has no config. Also what `--print-config` prints.
pub const DEFAULT_CONFIG: &str = include_str!("../packaging/dock.conf");

/// Smallest accepted icon size, in logical pixels.
pub const MIN_ICON_SIZE: f32 = 32.0;
/// Largest accepted icon size, in logical pixels.
pub const MAX_ICON_SIZE: f32 = 128.0;
/// Icon size used when the config says nothing usable.
pub const DEFAULT_ICON_SIZE: f32 = 56.0;
/// Smallest accepted magnification (1.0 disables magnification).
pub const MIN_MAGNIFICATION: f32 = 1.0;
/// Largest accepted magnification.
pub const MAX_MAGNIFICATION: f32 = 2.5;
/// Magnification used when the config says nothing usable.
pub const DEFAULT_MAGNIFICATION: f32 = 1.7;

/// One pinned entry (an icon, or a separator).
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// Section name, used as the label above the icon.
    pub name: String,
    /// A built-in icon name, or a path to an SVG / PNG.
    pub icon: String,
    /// Shell command that starts the app.
    pub exec: String,
    /// Process names or window classes that mean "running" (lower case).
    /// `None` derives them from `exec`; `Some(vec![])` never shows a running dot.
    pub matches: Option<Vec<String>>,
    /// True when this entry draws a divider instead of an icon.
    pub separator: bool,
}

impl Entry {
    /// Lower-case names that identify this app when it is running.
    pub fn process_names(&self) -> Vec<String> {
        if let Some(explicit) = &self.matches {
            return explicit.clone();
        }
        self.exec
            .split("||")
            .filter_map(|alternative| alternative.split_whitespace().next())
            .map(|program| program.rsplit('/').next().unwrap_or(program).to_lowercase())
            .collect()
    }

    /// True when clicking this entry can do something.
    pub fn is_clickable(&self) -> bool {
        !self.separator
    }
}

/// The whole dock configuration.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// Icon size in logical pixels (always finite, within [`MIN_ICON_SIZE`]..[`MAX_ICON_SIZE`]).
    pub size: f32,
    /// Size of the icon under the pointer relative to normal
    /// (always finite, within [`MIN_MAGNIFICATION`]..[`MAX_MAGNIFICATION`]).
    pub magnification: f32,
    /// True for the dark theme.
    pub dark: bool,
    /// The pinned entries, in order.
    pub entries: Vec<Entry>,
}

/// Parses `text` into a [`Config`], ignoring everything invalid.
///
/// See [`parse_detailed`] if you also want the list of things that were ignored.
pub fn parse(text: &str) -> Config {
    parse_detailed(text).0
}

/// Parses `text` into a [`Config`] plus one warning string per problem found:
/// values that are not numbers, unknown settings, malformed section headers,
/// settings that were clamped into range, and entries that cannot do anything.
///
/// The parse never fails: unusable lines are skipped and reported here.
pub fn parse_detailed(text: &str) -> (Config, Vec<String>) {
    let mut config = Config {
        size: DEFAULT_ICON_SIZE,
        magnification: DEFAULT_MAGNIFICATION,
        dark: true,
        entries: Vec::new(),
    };
    let mut warnings: Vec<String> = Vec::new();
    let mut current: Option<Entry> = None;

    for (index, raw) in strip_bom(text).lines().enumerate() {
        let line_no = index + 1;
        let line = raw.trim();

        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(name) = parse_header(line) {
            if let Some(entry) = current.take() {
                config.entries.push(entry);
            }
            current = Some(Entry {
                name: name.to_string(),
                icon: String::new(),
                exec: String::new(),
                matches: None,
                separator: name.eq_ignore_ascii_case("separator"),
            });
            continue;
        }

        if line.starts_with('[') {
            warnings.push(format!(
                "line {line_no}: `{line}` is not a valid section header; expected `[Name]`; ignored"
            ));
            continue;
        }

        let (key, value) = match line.split_once('=') {
            Some((key, value)) => (key.trim().to_lowercase(), value.trim()),
            None => {
                warnings.push(format!(
                    "line {line_no}: `{line}` is not `key = value`; ignored"
                ));
                continue;
            }
        };

        match current.as_mut() {
            Some(entry) => match key.as_str() {
                "icon" => entry.icon = value.to_string(),
                "exec" => entry.exec = value.to_string(),
                "match" => {
                    entry.matches = Some(
                        value
                            .split(',')
                            .map(|name| name.trim().to_lowercase())
                            .filter(|name| !name.is_empty())
                            .collect(),
                    );
                }
                known => warnings.push(format!(
                    "line {line_no}: unknown setting `{known}` in [{}]; ignored",
                    entry.name
                )),
            },
            None => match key.as_str() {
                "size" => match value.parse::<f32>() {
                    Ok(number) if number.is_finite() => {
                        let clamped = sanitize_icon_size(number, number);
                        if clamped != number {
                            warnings.push(format!(
                                "line {line_no}: size {number} is outside {}..{}; using {}",
                                MIN_ICON_SIZE, MAX_ICON_SIZE, clamped
                            ));
                        }
                        config.size = clamped;
                    }
                    _ => warnings.push(format!(
                        "line {line_no}: size `{value}` is not a number; using {}",
                        config.size
                    )),
                },
                "magnification" => match value.parse::<f32>() {
                    Ok(number) if number.is_finite() => {
                        let clamped = sanitize_magnification(number, number);
                        if clamped != number {
                            warnings.push(format!(
                                "line {line_no}: magnification {number} is outside {}..{}; using {}",
                                MIN_MAGNIFICATION, MAX_MAGNIFICATION, clamped
                            ));
                        }
                        config.magnification = clamped;
                    }
                    _ => warnings.push(format!(
                        "line {line_no}: magnification `{value}` is not a number; using {}",
                        config.magnification
                    )),
                },
                "theme" => match parse_theme(value) {
                    Some(dark) => config.dark = dark,
                    None => warnings.push(format!(
                        "line {line_no}: theme `{value}` is neither `dark` nor `light`; using {}",
                        if config.dark { "dark" } else { "light" }
                    )),
                },
                known => warnings.push(format!(
                    "line {line_no}: unknown setting `{known}`; ignored"
                )),
            },
        }
    }

    if let Some(entry) = current.take() {
        config.entries.push(entry);
    }

    for entry in &config.entries {
        if entry.separator {
            if !entry.icon.is_empty() || !entry.exec.is_empty() || entry.matches.is_some() {
                warnings.push(format!(
                    "[{}] is a separator, but it also defines icon/exec/match; those are ignored",
                    entry.name
                ));
            }
        } else {
            if entry.name.is_empty() {
                warnings.push("a section has an empty name; its label will be blank".to_string());
            }
            if entry.exec.is_empty() && entry.icon.is_empty() && entry.name.is_empty() {
                warnings.push(format!(
                    "[{}] has no exec, no icon and no name; clicking it does nothing",
                    entry.name
                ));
            }
        }
    }

    (config, warnings)
}

/// Strips a UTF-8 byte-order mark from the start of `text`.
pub fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

/// Parses `[Name]` into `Some("Name")`.
///
/// Returns `None` when the line is not a section header at all, or when it is
/// malformed (brackets in the name, missing closing bracket). The name may be
/// empty (`[]`), which the caller may warn about.
pub fn parse_header(line: &str) -> Option<&str> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    if inner.contains('[') || inner.contains(']') {
        return None;
    }
    Some(inner.trim())
}

/// Clamps an icon size into [`MIN_ICON_SIZE`]..[`MAX_ICON_SIZE`].
///
/// Non-finite input falls back to `fallback` (this is the only way a NaN or
/// infinity can leak into a geometry calculation, which would make icons vanish).
pub fn sanitize_icon_size(value: f32, fallback: f32) -> f32 {
    if !value.is_finite() {
        return fallback;
    }
    value.clamp(MIN_ICON_SIZE, MAX_ICON_SIZE)
}

/// Clamps a magnification into [`MIN_MAGNIFICATION`]..[`MAX_MAGNIFICATION`].
///
/// Non-finite input falls back to `fallback`.
pub fn sanitize_magnification(value: f32, fallback: f32) -> f32 {
    if !value.is_finite() {
        return fallback;
    }
    value.clamp(MIN_MAGNIFICATION, MAX_MAGNIFICATION)
}

/// Parses a `theme` value: `dark` → `Some(true)`, `light` → `Some(false)`,
/// anything else (including the empty string) → `None`.
pub fn parse_theme(value: &str) -> Option<bool> {
    if value.eq_ignore_ascii_case("dark") {
        Some(true)
    } else if value.eq_ignore_ascii_case("light") {
        Some(false)
    } else {
        None
    }
}

/// Pure core of [`default_path`]: `$XDG_CONFIG_HOME/malva/dock.conf`, falling back
/// to `$HOME/.config/malva/dock.conf`.
///
/// Following the XDG basedir specification, a relative `XDG_CONFIG_HOME` is
/// ignored (the original code accepted it, which let a relative value point the
/// dock at whatever directory it happened to start in).
pub fn default_path_with(xdg: Option<OsString>, home: Option<OsString>) -> Option<PathBuf> {
    let base = match xdg {
        Some(dir) if !dir.is_empty() && Path::new(&dir).is_absolute() => PathBuf::from(dir),
        _ => PathBuf::from(home?).join(".config"),
    };
    Some(base.join("malva").join("dock.conf"))
}

/// The default config path, or `None` when neither usable `XDG_CONFIG_HOME` nor
/// `HOME` is set.
pub fn default_path() -> Option<PathBuf> {
    default_path_with(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

/// Checks whether `path` is world-writable, in which case any other local user
/// can edit the dock's `exec` lines and run commands as you.
///
/// This is the one real privilege-escalation surface of a dock that reads a
/// config file full of shell commands, so it deserves a loud warning.
#[cfg(unix)]
pub fn world_writable_warning(path: &Path) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let mode = metadata.permissions().mode();
    if mode & 0o002 != 0 {
        return Some(format!(
            "{} is world-writable: any local user can edit it and run commands as you. \
             Fix it with: chmod o-w {}",
            path.display(),
            path.display()
        ));
    }
    None
}

/// Non-Unix systems have no permission bits to check.
#[cfg(not(unix))]
pub fn world_writable_warning(_path: &Path) -> Option<String> {
    None
}

/// Loads the config at `explicit` (or the default path), falling back to the
/// built-in one. Problems are printed by the caller; see [`load_detailed`].
pub fn load(explicit: Option<PathBuf>) -> Config {
    load_detailed(explicit).0
}

/// Loads the config like [`load`], but returns the warnings gathered along the
/// way: unreadable file, unsafe permissions, parse problems, empty file.
pub fn load_detailed(explicit: Option<PathBuf>) -> (Config, Vec<String>) {
    let mut warnings = Vec::new();
    let from_flag = explicit.is_some();
    if let Some(path) = explicit.or_else(default_path) {
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                if let Some(warning) = world_writable_warning(&path) {
                    warnings.push(warning);
                }
                let (config, parse_warnings) = parse_detailed(&text);
                warnings.extend(parse_warnings);
                if !config.entries.is_empty() {
                    return (config, warnings);
                }
                warnings.push(format!(
                    "{} defines no icons; using the built-in dock",
                    path.display()
                ));
            }
            Err(err) => {
                if from_flag || err.kind() != std::io::ErrorKind::NotFound {
                    warnings.push(format!("cannot read {}: {err}", path.display()));
                }
            }
        }
    }
    (parse(DEFAULT_CONFIG), warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn entry_named(name: &str) -> Entry {
        Entry {
            name: name.to_string(),
            icon: String::new(),
            exec: String::new(),
            matches: None,
            separator: false,
        }
    }

    // --------------------------------------------------------- process_names

    #[test]
    fn process_names_are_derived_from_each_exec_alternative() {
        let mut entry = entry_named("Terminal");
        entry.exec = "kitty || alacritty || foot".to_string();
        assert_eq!(entry.process_names(), vec!["kitty", "alacritty", "foot"]);
    }

    #[test]
    fn explicit_matches_replace_the_derived_names() {
        let mut entry = entry_named("Apps");
        entry.exec = "wofi --show drun".to_string();
        entry.matches = Some(vec!["malva-launcher".to_string(), "wofi".to_string()]);
        assert_eq!(
            entry.process_names(),
            vec!["malva-launcher".to_string(), "wofi".to_string()]
        );
    }

    #[test]
    fn an_empty_match_list_means_never_running() {
        let mut entry = entry_named("Downloads");
        entry.exec = "xdg-open ~/Downloads".to_string();
        entry.matches = Some(Vec::new());
        assert!(entry.process_names().is_empty());
    }

    #[test]
    fn exec_paths_are_reduced_to_their_base_name() {
        let mut entry = entry_named("Editor");
        entry.exec = "/usr/bin/kitty || ./local/bin/foot".to_string();
        assert_eq!(entry.process_names(), vec!["kitty", "foot"]);
    }

    #[test]
    fn derived_process_names_are_lower_cased() {
        let mut entry = entry_named("Browser");
        entry.exec = "Firefox || CHROME".to_string();
        assert_eq!(entry.process_names(), vec!["firefox", "chrome"]);
    }

    #[test]
    fn a_whitespace_only_exec_derives_no_names() {
        let mut entry = entry_named("Empty");
        entry.exec = "   ".to_string();
        assert!(entry.process_names().is_empty());
    }

    #[test]
    fn a_single_exec_without_alternatives_still_works() {
        let mut entry = entry_named("Solo");
        entry.exec = "nautilus".to_string();
        assert_eq!(entry.process_names(), vec!["nautilus"]);
    }

    // ------------------------------------------------------------------ parse

    #[test]
    fn the_built_in_config_parses() {
        let config = parse(DEFAULT_CONFIG);
        assert_eq!(config.size, 56.0);
        assert_eq!(config.magnification, 1.7);
        assert!(config.dark);
        assert_eq!(config.entries.len(), 12);
        assert_eq!(config.entries[0].name, "Apps");
        assert!(config.entries[9].separator);
        assert_eq!(config.entries[11].name, "Trash");
    }

    #[test]
    fn empty_text_yields_the_defaults() {
        let config = parse("");
        assert_eq!(config.size, DEFAULT_ICON_SIZE);
        assert_eq!(config.magnification, DEFAULT_MAGNIFICATION);
        assert!(config.dark);
        assert!(config.entries.is_empty());
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let config = parse("# a comment\n\n   \n  # indented comment\n[X]\nexec = x\n");
        assert_eq!(config.entries.len(), 1);
        assert_eq!(config.entries[0].exec, "x");
    }

    #[test]
    fn values_keep_their_hash_characters() {
        // `#` mid-value is legal shell syntax, so it must NOT be treated as a comment.
        let config = parse("[Grep]\nexec = grep '#include' file\n");
        assert_eq!(config.entries[0].exec, "grep '#include' file");
    }

    #[test]
    fn a_byte_order_mark_does_not_break_the_first_line() {
        let config = parse("\u{feff}size = 64\n[X]\nexec = x\n");
        assert_eq!(config.size, 64.0);
        assert_eq!(config.entries.len(), 1);
    }

    #[test]
    fn windows_line_endings_are_handled() {
        let config = parse("size = 64\r\n[X]\r\nexec = x\r\n");
        assert_eq!(config.size, 64.0);
        assert_eq!(config.entries[0].exec, "x");
    }

    #[test]
    fn settings_are_clamped() {
        let config = parse("size = 500\nmagnification = 9\ntheme = light\n[X]\nexec = x\n");
        assert_eq!(config.size, 128.0);
        assert_eq!(config.magnification, 2.5);
        assert!(!config.dark);
        assert_eq!(config.entries.len(), 1);
    }

    #[test]
    fn theme_dark_and_light_are_recognised_case_insensitively() {
        assert!(parse("theme = dark\n").dark);
        assert!(!parse("theme = light\n").dark);
        assert!(!parse("theme = LIGHT\n").dark);
        assert!(parse("theme = Dark\n").dark);
    }

    #[test]
    fn an_unknown_theme_keeps_the_current_one() {
        // The default is dark, so garbage keeps dark (same outcome as before, but now warned).
        assert!(parse("theme = solarized\n").dark);
        let (_, warnings) = parse_detailed("theme = solarized\n");
        assert!(warnings.iter().any(|w| w.contains("solarized")));
    }

    #[test]
    fn section_keys_do_not_leak_into_global_settings() {
        let config = parse("[X]\nsize = 500\nexec = x\n");
        // `size` inside a section is not a global setting; it is ignored (with a warning).
        assert_eq!(config.size, DEFAULT_ICON_SIZE);
        assert_eq!(config.entries[0].exec, "x");
    }

    #[test]
    fn duplicate_keys_last_wins() {
        let config = parse("[X]\nexec = a\nexec = b\n");
        assert_eq!(config.entries[0].exec, "b");
    }

    #[test]
    fn keys_are_case_insensitive_and_values_are_trimmed() {
        let config = parse("[X]\n  EXEC =   spaced  \nMATCH = A , b\n");
        assert_eq!(config.entries[0].exec, "spaced");
        assert_eq!(
            config.entries[0].matches,
            Some(vec!["a".to_string(), "b".to_string()])
        );
    }

    #[test]
    fn a_section_needs_no_settings() {
        let config = parse("[separator]\n");
        assert_eq!(config.entries.len(), 1);
        assert!(config.entries[0].separator);
    }

    #[test]
    fn running_names_come_from_exec_unless_given() {
        let config = parse(DEFAULT_CONFIG);
        let apps = &config.entries[0];
        assert_eq!(apps.process_names()[0], "malva-launcher");
        let files = &config.entries[1];
        assert!(files.process_names().contains(&"nautilus".to_string()));
        let downloads = &config.entries[10];
        assert!(downloads.process_names().is_empty());
    }

    // ------------------------------------------------------ Entry::is_clickable

    #[test]
    fn a_normal_entry_is_clickable() {
        assert!(entry_named("Terminal").is_clickable());
    }

    #[test]
    fn an_entry_without_exec_is_still_clickable() {
        // Clicking it just does nothing (with a warning); it is not a separator.
        assert!(entry_named("Empty").is_clickable());
    }

    #[test]
    fn a_separator_is_not_clickable() {
        let mut entry = entry_named("separator");
        entry.separator = true;
        assert!(!entry.is_clickable());
    }

    #[test]
    fn a_separator_named_differently_is_still_not_clickable() {
        // The separator flag, not the name, decides.
        let mut entry = entry_named("Divider");
        entry.separator = true;
        assert!(!entry.is_clickable());
    }

    #[test]
    fn parsed_separators_are_not_clickable_and_icons_are() {
        let config = parse("[A]\nexec = x\n[separator]\n[B]\nexec = y\n");
        assert!(config.entries[0].is_clickable());
        assert!(!config.entries[1].is_clickable());
        assert!(config.entries[2].is_clickable());
    }

    // -------------------------------------------------------- parse_detailed

    #[test]
    fn the_built_in_config_parses_without_warnings() {
        let (_, warnings) = parse_detailed(DEFAULT_CONFIG);
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
    }

    #[test]
    fn a_line_without_equals_sign_warns() {
        let (_, warnings) = parse_detailed("hello world\n");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("hello world"));
        assert!(warnings[0].contains("line 1"));
    }

    #[test]
    fn a_malformed_header_warns() {
        let (_, warnings) = parse_detailed("[Apps] junk\n");
        assert!(warnings.iter().any(|w| w.contains("[Apps] junk")));
    }

    #[test]
    fn an_unknown_setting_in_a_section_warns() {
        let (_, warnings) = parse_detailed("[X]\nfoo = bar\n");
        assert!(warnings
            .iter()
            .any(|w| w.contains("foo") && w.contains("[X]")));
    }

    #[test]
    fn an_unknown_global_setting_warns() {
        let (_, warnings) = parse_detailed("foo = bar\n");
        assert!(warnings.iter().any(|w| w.contains("foo")));
    }

    #[test]
    fn a_non_numeric_size_warns_and_keeps_the_default() {
        let (config, warnings) = parse_detailed("size = big\n");
        assert_eq!(config.size, DEFAULT_ICON_SIZE);
        assert!(warnings
            .iter()
            .any(|w| w.contains("size") && w.contains("big")));
    }

    #[test]
    fn an_out_of_range_size_warns_about_the_clamp() {
        let (config, warnings) = parse_detailed("size = 500\n");
        assert_eq!(config.size, MAX_ICON_SIZE);
        assert!(warnings
            .iter()
            .any(|w| w.contains("clamped") || w.contains("outside")));
    }

    #[test]
    fn a_non_numeric_magnification_warns() {
        let (config, warnings) = parse_detailed("magnification = huge\n");
        assert_eq!(config.magnification, DEFAULT_MAGNIFICATION);
        assert!(warnings.iter().any(|w| w.contains("magnification")));
    }

    #[test]
    fn a_separator_with_extra_settings_warns() {
        let (_, warnings) = parse_detailed("[separator]\nexec = x\n");
        assert!(warnings.iter().any(|w| w.contains("separator")));
    }

    #[test]
    fn an_empty_section_name_warns() {
        let (_, warnings) = parse_detailed("[]\n");
        assert!(warnings.iter().any(|w| w.contains("empty name")));
    }

    // -------------------------------------------------------- parse_header

    #[test]
    fn parse_header_accepts_a_simple_header() {
        assert_eq!(parse_header("[Apps]"), Some("Apps"));
    }

    #[test]
    fn parse_header_trims_the_name() {
        assert_eq!(parse_header("[ Apps ]"), Some("Apps"));
    }

    #[test]
    fn parse_header_accepts_an_empty_name() {
        assert_eq!(parse_header("[]"), Some(""));
    }

    #[test]
    fn parse_header_rejects_nested_brackets() {
        assert_eq!(parse_header("[App]s]"), None);
        assert_eq!(parse_header("[[App]]"), None);
    }

    #[test]
    fn parse_header_rejects_a_missing_bracket() {
        assert_eq!(parse_header("[Apps"), None);
        assert_eq!(parse_header("Apps]"), None);
    }

    #[test]
    fn parse_header_rejects_plain_text() {
        assert_eq!(parse_header("Apps"), None);
        assert_eq!(parse_header(""), None);
    }

    #[test]
    fn parse_header_keeps_unicode_names() {
        assert_eq!(parse_header("[文件]"), Some("文件"));
    }

    // ------------------------------------------------------ sanitize helpers

    #[test]
    fn sanitize_icon_size_keeps_in_range_values() {
        assert_eq!(sanitize_icon_size(56.0, 32.0), 56.0);
        assert_eq!(sanitize_icon_size(32.0, 56.0), 32.0);
        assert_eq!(sanitize_icon_size(128.0, 56.0), 128.0);
    }

    #[test]
    fn sanitize_icon_size_clamps_out_of_range_values() {
        assert_eq!(sanitize_icon_size(1.0, 56.0), MIN_ICON_SIZE);
        assert_eq!(sanitize_icon_size(999.0, 56.0), MAX_ICON_SIZE);
        assert_eq!(sanitize_icon_size(-40.0, 56.0), MIN_ICON_SIZE);
    }

    #[test]
    fn sanitize_icon_size_replaces_nan_with_the_fallback() {
        assert_eq!(sanitize_icon_size(f32::NAN, 56.0), 56.0);
    }

    #[test]
    fn sanitize_icon_size_replaces_infinities_with_the_fallback() {
        assert_eq!(sanitize_icon_size(f32::INFINITY, 44.0), 44.0);
        assert_eq!(sanitize_icon_size(f32::NEG_INFINITY, 44.0), 44.0);
    }

    #[test]
    fn sanitize_icon_size_accepts_just_outside_the_bounds_by_clamping() {
        assert_eq!(sanitize_icon_size(31.999, 56.0), MIN_ICON_SIZE);
        assert_eq!(sanitize_icon_size(128.001, 56.0), MAX_ICON_SIZE);
        assert_eq!(sanitize_icon_size(0.0, 56.0), MIN_ICON_SIZE);
    }

    #[test]
    fn sanitize_magnification_keeps_in_range_values() {
        assert_eq!(sanitize_magnification(1.0, 1.7), 1.0);
        assert_eq!(sanitize_magnification(1.7, 1.0), 1.7);
        assert_eq!(sanitize_magnification(2.5, 1.7), 2.5);
    }

    #[test]
    fn sanitize_magnification_clamps_out_of_range_values() {
        assert_eq!(sanitize_magnification(0.5, 1.7), MIN_MAGNIFICATION);
        assert_eq!(sanitize_magnification(9.0, 1.7), MAX_MAGNIFICATION);
        assert_eq!(sanitize_magnification(-3.0, 1.7), MIN_MAGNIFICATION);
    }

    #[test]
    fn sanitize_magnification_replaces_non_finite_values() {
        assert_eq!(sanitize_magnification(f32::NAN, 1.7), 1.7);
        assert_eq!(sanitize_magnification(f32::INFINITY, 1.2), 1.2);
        assert_eq!(sanitize_magnification(f32::NEG_INFINITY, 1.2), 1.2);
    }

    #[test]
    fn sanitize_magnification_accepts_just_outside_the_bounds_by_clamping() {
        assert_eq!(sanitize_magnification(0.999, 1.7), MIN_MAGNIFICATION);
        assert_eq!(sanitize_magnification(2.501, 1.7), MAX_MAGNIFICATION);
    }

    #[test]
    fn sanitize_magnification_of_exactly_one_disables_magnification() {
        // 1.0 is the documented off switch, not a fallback case.
        assert_eq!(sanitize_magnification(1.0, 1.7), 1.0);
        assert_eq!(sanitize_magnification(1.0, 2.0), 1.0);
    }

    // ------------------------------------------------------------ parse_theme

    #[test]
    fn parse_theme_maps_dark_to_true() {
        assert_eq!(parse_theme("dark"), Some(true));
        assert_eq!(parse_theme("Dark"), Some(true));
        assert_eq!(parse_theme("  dark  "), None); // caller trims first
    }

    #[test]
    fn parse_theme_maps_light_to_false() {
        assert_eq!(parse_theme("light"), Some(false));
        assert_eq!(parse_theme("LIGHT"), Some(false));
        assert_eq!(parse_theme("Light"), Some(false));
    }

    #[test]
    fn parse_theme_rejects_garbage() {
        assert_eq!(parse_theme("solarized"), None);
        assert_eq!(parse_theme("blue"), None);
        assert_eq!(parse_theme("darkk"), None);
    }

    #[test]
    fn parse_theme_rejects_the_empty_string() {
        assert_eq!(parse_theme(""), None);
    }

    #[test]
    fn parse_theme_is_ascii_only_case_insensitive() {
        assert_eq!(parse_theme("DARK"), Some(true));
        assert_eq!(parse_theme("LiGhT"), Some(false));
    }

    // ------------------------------------------------------------ strip_bom

    #[test]
    fn strip_bom_removes_a_leading_bom() {
        assert_eq!(strip_bom("\u{feff}size = 1"), "size = 1");
    }

    #[test]
    fn strip_bom_keeps_text_without_a_bom() {
        assert_eq!(strip_bom("size = 1"), "size = 1");
    }

    #[test]
    fn strip_bom_only_strips_at_the_start() {
        assert_eq!(strip_bom("a\u{feff}b"), "a\u{feff}b");
    }

    #[test]
    fn strip_bom_handles_an_empty_string() {
        assert_eq!(strip_bom(""), "");
    }

    #[test]
    fn strip_bom_handles_a_bom_only_string() {
        assert_eq!(strip_bom("\u{feff}"), "");
    }

    #[test]
    fn strip_bom_accepts_two_boms_but_strips_only_the_first() {
        assert_eq!(strip_bom("\u{feff}\u{feff}size = 1"), "\u{feff}size = 1");
    }

    #[test]
    fn strip_bom_does_not_touch_a_regular_character() {
        assert_eq!(strip_bom("ÿz"), "ÿz");
    }

    // -------------------------------------------------- default_path_with

    #[test]
    fn default_path_uses_home_dot_config_by_default() {
        let path = default_path_with(None, Some(OsString::from("/home/z")));
        assert_eq!(path, Some(PathBuf::from("/home/z/.config/malva/dock.conf")));
    }

    #[test]
    fn default_path_prefers_an_absolute_xdg_config_home() {
        let path = default_path_with(
            Some(OsString::from("/etc/xdg")),
            Some(OsString::from("/home/z")),
        );
        assert_eq!(path, Some(PathBuf::from("/etc/xdg/malva/dock.conf")));
    }

    #[test]
    fn default_path_ignores_a_relative_xdg_config_home() {
        // XDG basedir spec: only an absolute path counts.
        let path = default_path_with(
            Some(OsString::from("relative/xdg")),
            Some(OsString::from("/home/z")),
        );
        assert_eq!(path, Some(PathBuf::from("/home/z/.config/malva/dock.conf")));
    }

    #[test]
    fn default_path_ignores_an_empty_xdg_config_home() {
        let path = default_path_with(Some(OsString::new()), Some(OsString::from("/home/z")));
        assert_eq!(path, Some(PathBuf::from("/home/z/.config/malva/dock.conf")));
    }

    #[test]
    fn default_path_is_none_without_home() {
        assert_eq!(default_path_with(None, None), None);
        assert!(default_path_with(Some(OsString::from("/etc/xdg")), None).is_some());
    }

    #[test]
    fn default_path_reads_the_environment() {
        let _env = crate::testutil::EnvScope::new(&["XDG_CONFIG_HOME", "HOME"]);
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::set_var("HOME", "/home/testuser");
        assert_eq!(
            default_path(),
            Some(PathBuf::from("/home/testuser/.config/malva/dock.conf"))
        );
        std::env::set_var("XDG_CONFIG_HOME", "/tmp/xdg");
        assert_eq!(
            default_path(),
            Some(PathBuf::from("/tmp/xdg/malva/dock.conf"))
        );
        std::env::set_var("XDG_CONFIG_HOME", "relative");
        assert_eq!(
            default_path(),
            Some(PathBuf::from("/home/testuser/.config/malva/dock.conf"))
        );
    }

    #[test]
    fn default_path_falls_back_to_home_when_xdg_is_empty() {
        let _env = crate::testutil::EnvScope::new(&["XDG_CONFIG_HOME", "HOME"]);
        std::env::set_var("XDG_CONFIG_HOME", "");
        std::env::set_var("HOME", "/home/empty-xdg");
        assert_eq!(
            default_path(),
            Some(PathBuf::from("/home/empty-xdg/.config/malva/dock.conf"))
        );
    }

    #[test]
    fn default_path_is_none_when_both_are_unset() {
        let _env = crate::testutil::EnvScope::new(&["XDG_CONFIG_HOME", "HOME"]);
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::remove_var("HOME");
        assert_eq!(default_path(), None);
    }

    #[test]
    fn default_path_uses_an_absolute_xdg_even_without_home() {
        let _env = crate::testutil::EnvScope::new(&["XDG_CONFIG_HOME", "HOME"]);
        std::env::set_var("XDG_CONFIG_HOME", "/etc/xdg");
        std::env::remove_var("HOME");
        assert_eq!(
            default_path(),
            Some(PathBuf::from("/etc/xdg/malva/dock.conf"))
        );
    }

    #[test]
    fn default_path_appends_the_same_suffix_everywhere() {
        for (xdg, home) in [
            (Some("/a"), Some("/b")),
            (Some("/very/long/path"), Some("/h")),
            (None, Some("/home/z")),
        ] {
            let path =
                default_path_with(xdg.map(OsString::from), home.map(OsString::from)).unwrap();
            assert!(path.ends_with("malva/dock.conf"), "{path:?}");
        }
    }

    // ------------------------------------------------ world_writable_warning

    #[cfg(unix)]
    mod permissions {
        use super::*;
        use crate::testutil::TempDir;

        #[test]
        fn a_normal_file_is_fine() {
            let dir = TempDir::new("perm-normal");
            let file = dir.file("dock.conf", "[X]\nexec = x\n");
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(world_writable_warning(&file).is_none());
        }

        #[test]
        fn a_world_writable_file_is_flagged() {
            let dir = TempDir::new("perm-ww");
            let file = dir.file("dock.conf", "[X]\nexec = x\n");
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o666)).unwrap();
            let warning = world_writable_warning(&file);
            assert!(warning.is_some(), "world-writable file must be flagged");
            assert!(warning.unwrap().contains("chmod o-w"));
        }

        #[test]
        fn an_owner_only_file_is_fine() {
            let dir = TempDir::new("perm-600");
            let file = dir.file("dock.conf", "[X]\nexec = x\n");
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
            assert!(world_writable_warning(&file).is_none());
        }

        #[test]
        fn a_group_writable_file_is_not_flagged() {
            let dir = TempDir::new("perm-664");
            let file = dir.file("dock.conf", "[X]\nexec = x\n");
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o664)).unwrap();
            assert!(world_writable_warning(&file).is_none());
        }

        #[test]
        fn a_missing_file_is_not_flagged() {
            assert!(world_writable_warning(Path::new("/does/not/exist")).is_none());
        }

        #[test]
        fn a_directory_is_not_flagged() {
            let dir = TempDir::new("perm-dir");
            assert!(world_writable_warning(dir.path()).is_none());
        }
    }

    // ------------------------------------------------------------------ load

    #[test]
    fn load_reads_an_explicit_file() {
        let dir = TempDir::new("load-explicit");
        let file = dir.file("dock.conf", "size = 64\n[Terminal]\nexec = kitty\n");
        let config = load(Some(file));
        assert_eq!(config.size, 64.0);
        assert_eq!(config.entries.len(), 1);
        assert_eq!(config.entries[0].exec, "kitty");
    }

    #[test]
    fn load_falls_back_to_the_builtin_config_when_the_file_is_missing() {
        let config = load(Some(PathBuf::from("/does/not/exist/dock.conf")));
        assert_eq!(config.entries.len(), parse(DEFAULT_CONFIG).entries.len());
    }

    #[test]
    fn load_falls_back_when_the_file_defines_no_icons() {
        let dir = TempDir::new("load-empty");
        let file = dir.file("dock.conf", "# only comments\nsize = 64\n");
        let (config, warnings) = load_detailed(Some(file));
        assert!(
            !config.entries.is_empty(),
            "must fall back to the built-in dock"
        );
        assert!(warnings.iter().any(|w| w.contains("defines no icons")));
    }

    #[test]
    fn load_warns_when_an_explicit_file_cannot_be_read() {
        let dir = TempDir::new("load-dir");
        let (_, warnings) = load_detailed(Some(dir.path().to_path_buf()));
        assert!(warnings.iter().any(|w| w.contains("cannot read")));
    }

    #[test]
    fn load_does_not_warn_about_a_missing_default_file() {
        let _env = crate::testutil::EnvScope::new(&["XDG_CONFIG_HOME", "HOME"]);
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::set_var("HOME", "/nonexistent-malva-home");
        let (_, warnings) = load_detailed(None);
        assert!(
            !warnings.iter().any(|w| w.contains("cannot read")),
            "a missing default config must be silent: {warnings:?}"
        );
    }

    #[test]
    fn load_reads_the_default_path() {
        let _env = crate::testutil::EnvScope::new(&["XDG_CONFIG_HOME", "HOME"]);
        let dir = TempDir::new("load-default");
        std::fs::create_dir_all(dir.path().join(".config/malva")).unwrap();
        std::fs::write(
            dir.path().join(".config/malva/dock.conf"),
            "[Custom]\nexec = x\n",
        )
        .unwrap();
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::set_var("HOME", dir.path());
        let config = load(None);
        assert_eq!(config.entries.len(), 1);
        assert_eq!(config.entries[0].name, "Custom");
    }

    #[test]
    fn load_collects_parse_warnings() {
        let dir = TempDir::new("load-warn");
        let file = dir.file("dock.conf", "size = lots\n[X]\nfoo = bar\nexec = x\n");
        let (config, warnings) = load_detailed(Some(file));
        assert_eq!(config.entries.len(), 1);
        assert!(warnings.iter().any(|w| w.contains("size")));
        assert!(warnings.iter().any(|w| w.contains("foo")));
    }
}
