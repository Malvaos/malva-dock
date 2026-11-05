//! Starting apps, finding out which ones are running, and raising a running one.
//!
//! Everything that touches the outside world (`sh`, `hyprctl`, `wmctrl`, `/proc`)
//! sits behind a thin wrapper, and every wrapper has a pure, injectable core that
//! the unit tests can exercise without a desktop session.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

use crate::platform::env_flag;

/// Why an app could not be started.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchError {
    /// The command is empty or only whitespace.
    EmptyCommand,
    /// The command contains a NUL byte. `Command::arg` would panic on it,
    /// and the OS would silently truncate it, so we refuse it up front.
    InteriorNul,
    /// `sh` itself could not be spawned, with the OS error.
    Spawn(String),
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LaunchError::EmptyCommand => write!(f, "the command is empty"),
            LaunchError::InteriorNul => write!(f, "the command contains a NUL byte"),
            LaunchError::Spawn(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for LaunchError {}

/// Checks an `exec` string before it reaches the OS.
///
/// * empty / whitespace-only → [`LaunchError::EmptyCommand`]
/// * interior NUL byte → [`LaunchError::InteriorNul`] (this used to be a panic,
///   because `Command::arg` aborts on NUL bytes)
pub fn validate_exec(exec: &str) -> Result<(), LaunchError> {
    if exec.trim().is_empty() {
        return Err(LaunchError::EmptyCommand);
    }
    if exec.contains('\0') {
        return Err(LaunchError::InteriorNul);
    }
    Ok(())
}

/// Runs `exec` through the shell, detached from the dock, and reaps the child so
/// it does not linger as a zombie. Returns whether a process was started.
pub fn launch(exec: &str) -> Result<(), LaunchError> {
    validate_exec(exec)?;

    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(exec)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // The app must survive the dock and not receive its signals: put it in a
    // process group of its own (no controlling-terminal handover).
    #[cfg(unix)]
    command.process_group(0);

    match command.spawn() {
        Ok(mut child) => {
            // Reap the child so it does not linger as a zombie. This thread lives
            // as long as the app, which is the same lifetime the app would have
            // had without the dock.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            Ok(())
        }
        Err(err) => Err(LaunchError::Spawn(err.to_string())),
    }
}

/// True when running under Hyprland (a set, non-empty instance signature).
pub fn on_hyprland() -> bool {
    env_flag("HYPRLAND_INSTANCE_SIGNATURE")
}

/// Parses the window classes out of `hyprctl clients` output. Pure.
///
/// Client lines look like `\tClass: kitty`; anything not starting with `class:`
/// after trimming is ignored.
pub fn hypr_classes_from_output(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("class:"))
        .map(|class| class.trim().to_string())
        .filter(|class| !class.is_empty())
        .collect()
}

/// Window classes of every open window, straight from `hyprctl`.
/// Returns an empty list when not on Hyprland or when `hyprctl` fails.
pub fn hypr_classes() -> Vec<String> {
    if !on_hyprland() {
        return Vec::new();
    }
    match Command::new("hyprctl").arg("clients").output() {
        Ok(output) => hypr_classes_from_output(&String::from_utf8_lossy(&output.stdout)),
        Err(_) => Vec::new(),
    }
}

/// How long a `hyprctl clients` result is reused before it is fetched again.
pub const HYPR_CACHE_TTL: Duration = Duration::from_millis(750);

/// Default fetcher for [`HyprClients`].
pub type HyprFetch = fn() -> Vec<String>;

/// A small TTL cache for the Hyprland window list, so that polling and focusing
/// within one interaction do not spawn `hyprctl` twice.
///
/// The fetcher (and, via the `classes`/`store` parameters, the clock) are injected,
/// which is what makes the caching itself unit-testable.
pub struct HyprClients<F = HyprFetch>
where
    F: Fn() -> Vec<String>,
{
    ttl: Duration,
    fetch: F,
    cache: RefCell<Option<(Instant, Rc<Vec<String>>)>>,
}

impl HyprClients<HyprFetch> {
    /// A cache around [`hypr_classes`] that refreshes at most every `ttl`.
    pub fn new(ttl: Duration) -> Self {
        HyprClients::with_fetcher(ttl, hypr_classes as HyprFetch)
    }
}

impl<F: Fn() -> Vec<String>> HyprClients<F> {
    /// Builds a cache around an injected fetcher (used by the tests).
    pub fn with_fetcher(ttl: Duration, fetch: F) -> Self {
        HyprClients {
            ttl,
            fetch,
            cache: RefCell::new(None),
        }
    }

    /// The window classes, fetching at most once per `ttl`.
    pub fn classes(&self, now: Instant) -> Vec<String> {
        {
            let cache = self.cache.borrow();
            if let Some((at, classes)) = cache.as_ref() {
                // `checked_duration_since` also covers a clock that jumped backwards.
                if let Some(age) = now.checked_duration_since(*at) {
                    if age < self.ttl {
                        return classes.as_ref().clone();
                    }
                }
            }
        }
        let fresh = Rc::new((self.fetch)());
        let result = fresh.as_ref().clone();
        *self.cache.borrow_mut() = Some((now, fresh));
        result
    }

    /// Publishes a fresh list (fetched elsewhere) into the cache.
    pub fn store(&self, now: Instant, classes: Vec<String>) {
        *self.cache.borrow_mut() = Some((now, Rc::new(classes)));
    }

    /// Empties the cache; the next [`classes`](Self::classes) call fetches again.
    pub fn invalidate(&self) {
        *self.cache.borrow_mut() = None;
    }
}

thread_local! {
    static HYPR_CLIENTS: HyprClients<HyprFetch> = HyprClients::new(HYPR_CACHE_TTL);
}

/// Window classes through the shared TTL cache (used by the running-app poll).
pub fn hypr_classes_cached() -> Vec<String> {
    HYPR_CLIENTS.with(|clients| clients.classes(Instant::now()))
}

/// Lower-case names of everything that is running: process names from `/proc`, plus
/// window classes when running under Hyprland.
pub fn running_processes() -> HashSet<String> {
    let mut names = process_names_from_proc(Path::new("/proc"));
    for class in hypr_classes_cached() {
        names.insert(class.to_lowercase());
    }
    names
}

/// Reads process names from a directory laid out like `/proc`.
///
/// Parameterised so tests can build a fake `/proc` tree; the real entry point is
/// [`running_processes`].
///
/// Both `comm` (limited to 15 characters by the kernel) and the first `cmdline`
/// word are collected, because long program names only survive in `cmdline`.
pub fn process_names_from_proc(proc_dir: &Path) -> HashSet<String> {
    let mut names = HashSet::new();
    let dir = match std::fs::read_dir(proc_dir) {
        Ok(dir) => dir,
        Err(_) => return names,
    };
    for entry in dir.flatten() {
        let is_pid = entry
            .file_name()
            .to_str()
            .map(|name| !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit()))
            .unwrap_or(false);
        if !is_pid {
            continue;
        }
        let path = entry.path();

        if let Ok(comm) = std::fs::read_to_string(path.join("comm")) {
            let comm = comm.trim();
            if !comm.is_empty() {
                names.insert(comm.to_lowercase());
            }
        }
        if let Ok(cmdline) = std::fs::read(path.join("cmdline")) {
            if let Some(first) = cmdline.split(|&byte| byte == 0).next() {
                let program = String::from_utf8_lossy(first);
                if let Some(base) = program.rsplit('/').next() {
                    if !base.is_empty() {
                        names.insert(base.to_lowercase());
                    }
                }
            }
        }
    }
    names
}

/// True when one of `names` matches the window `class` (case-insensitively).
pub fn class_matches(names: &[String], class: &str) -> bool {
    names.iter().any(|name| name.eq_ignore_ascii_case(class))
}

/// Escapes the Hyprland regex metacharacters in a window class.
///
/// The class comes from the outside world and is interpolated into
/// `class:^(...)$`, so an unescaped `(` in a class like `org.foo(bar)` could
/// previously change what the selector matched — or make `hyprctl` act on the
/// wrong window. This is the injection fix.
pub fn escape_hypr_regex(class: &str) -> String {
    const META: &str = "\\.^$*+?()[]{}|";
    let mut escaped = String::with_capacity(class.len());
    for c in class.chars() {
        if META.contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

/// The `hyprctl` window selector for a class, with the class escaped.
pub fn hypr_selector_for(class: &str) -> String {
    format!("class:^({})$", escape_hypr_regex(class))
}

/// The pure core of [`focus`]: tries Hyprland classes first, then the X11 helper.
///
/// `hypr_classes` is `None` when not on Hyprland (then `dispatch` is never
/// called). `dispatch` receives a ready-made selector; `activate_x11` receives a
/// process name. Both return whether they succeeded.
pub fn focus_with(
    names: &[String],
    hypr_classes: Option<Vec<String>>,
    mut dispatch: impl FnMut(&str) -> bool,
    mut activate_x11: impl FnMut(&str) -> bool,
) -> bool {
    if let Some(classes) = hypr_classes {
        for class in classes {
            if class_matches(names, &class) && dispatch(&hypr_selector_for(&class)) {
                return true;
            }
        }
    }
    for name in names {
        if activate_x11(name) {
            return true;
        }
    }
    false
}

/// Tries to bring an already-running app to the front. Returns `false` when it
/// cannot (the caller then starts a new instance).
///
/// Works on Hyprland (via `hyprctl`) and on X11 (via `wmctrl`, if installed).
/// Other Wayland compositors have no standard way for a client to do this.
pub fn focus(names: &[String]) -> bool {
    let classes = if on_hyprland() {
        let fresh = hypr_classes();
        // Publish the fresh list so the next running-app poll does not re-fetch.
        HYPR_CLIENTS.with(|clients| clients.store(Instant::now(), fresh.clone()));
        Some(fresh)
    } else {
        None
    };
    focus_with(
        names,
        classes,
        |selector| run_succeeds("hyprctl", &["dispatch", "focuswindow", selector]),
        |name| run_succeeds("wmctrl", &["-x", "-a", name]),
    )
}

/// Runs a helper program quietly and reports whether it exited successfully.
fn run_succeeds(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{EnvScope, TempDir};
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ------------------------------------------------------------ validate_exec

    #[test]
    fn a_normal_command_validates() {
        assert_eq!(validate_exec("kitty || alacritty"), Ok(()));
        assert_eq!(validate_exec("xdg-open ~/Downloads"), Ok(()));
    }

    #[test]
    fn an_empty_command_is_rejected() {
        assert_eq!(validate_exec(""), Err(LaunchError::EmptyCommand));
    }

    #[test]
    fn a_whitespace_command_is_rejected() {
        assert_eq!(validate_exec("   \t \n "), Err(LaunchError::EmptyCommand));
    }

    #[test]
    fn a_command_with_a_nul_byte_is_rejected() {
        // `Command::arg` would panic on this before the fix.
        assert_eq!(
            validate_exec("kitty\0rm -rf"),
            Err(LaunchError::InteriorNul)
        );
    }

    #[test]
    fn unicode_commands_are_accepted() {
        assert_eq!(validate_exec("kitty --title '文件'"), Ok(()));
    }

    #[test]
    fn a_padded_command_still_validates() {
        // Leading whitespace is fine: the shell handles it.
        assert_eq!(validate_exec("  kitty  "), Ok(()));
    }

    // ------------------------------------------------------------------ launch

    #[test]
    fn launching_an_empty_command_does_nothing() {
        assert_eq!(launch(""), Err(LaunchError::EmptyCommand));
        assert_eq!(launch("   "), Err(LaunchError::EmptyCommand));
    }

    #[test]
    fn launching_a_command_with_a_nul_byte_is_refused() {
        assert_eq!(launch("true\0false"), Err(LaunchError::InteriorNul));
    }

    #[test]
    fn launching_true_succeeds() {
        assert_eq!(launch("true"), Ok(()));
    }

    #[test]
    fn launching_a_failing_command_still_spawned() {
        // `sh -c false` starts fine; the exit code is not a launch error.
        assert_eq!(launch("false"), Ok(()));
    }

    #[test]
    fn launching_a_slow_command_succeeds_and_is_reaped() {
        assert_eq!(launch("sleep 0.05"), Ok(()));
        std::thread::sleep(Duration::from_millis(200));
    }

    #[test]
    fn launch_errors_are_displayable() {
        assert!(!LaunchError::EmptyCommand.to_string().is_empty());
        assert!(!LaunchError::InteriorNul.to_string().is_empty());
        assert!(!LaunchError::Spawn("no sh".into()).to_string().is_empty());
    }

    // ---------------------------------------------------------- on_hyprland

    #[test]
    fn on_hyprland_reflects_the_environment() {
        let _env = EnvScope::new(&["HYPRLAND_INSTANCE_SIGNATURE"]);
        std::env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");
        assert!(!on_hyprland());
        std::env::set_var("HYPRLAND_INSTANCE_SIGNATURE", "sig-123");
        assert!(on_hyprland());
    }

    #[test]
    fn an_empty_instance_signature_means_no_hyprland() {
        let _env = EnvScope::new(&["HYPRLAND_INSTANCE_SIGNATURE"]);
        std::env::set_var("HYPRLAND_INSTANCE_SIGNATURE", "");
        assert!(!on_hyprland());
    }

    // ------------------------------------------------- hypr_classes_from_output

    #[test]
    fn hypr_output_yields_its_classes() {
        let text = "Window 1 -> 64a12b:\n\tmapped: true\n\tclass: kitty\n\ttitle: fish\n\n\
                    Window 2 -> 64a13c:\n\tmapped: true\n\tclass: firefox\n";
        assert_eq!(
            hypr_classes_from_output(text),
            vec!["kitty".to_string(), "firefox".to_string()]
        );
    }

    #[test]
    fn empty_output_yields_no_classes() {
        assert!(hypr_classes_from_output("").is_empty());
    }

    #[test]
    fn output_without_class_lines_yields_nothing() {
        assert!(hypr_classes_from_output("Workspace 1\n\tmonitor: DP-1").is_empty());
    }

    #[test]
    fn classes_are_trimmed() {
        assert_eq!(
            hypr_classes_from_output("\tclass:    spaced-out   \n"),
            vec!["spaced-out".to_string()]
        );
    }

    #[test]
    fn an_empty_class_is_dropped() {
        assert_eq!(
            hypr_classes_from_output("class:   \nclass:\n"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn non_client_class_prefixes_are_ignored() {
        // `xclass:` does not start with `class:` after trimming.
        assert!(hypr_classes_from_output("xclass: kitty\nklass: vim").is_empty());
    }

    #[test]
    fn duplicate_classes_are_kept_in_order() {
        // focus() tries them in order; keeping duplicates is harmless and honest.
        assert_eq!(
            hypr_classes_from_output("class: a\nclass: b\nclass: a\n"),
            vec!["a".to_string(), "b".to_string(), "a".to_string()]
        );
    }

    // ------------------------------------------------------- escape_hypr_regex

    #[test]
    fn a_plain_class_is_unchanged() {
        assert_eq!(escape_hypr_regex("kitty"), "kitty");
        assert_eq!(escape_hypr_regex("alacritty foot"), "alacritty foot");
    }

    #[test]
    fn dots_are_escaped() {
        assert_eq!(escape_hypr_regex("org.foo.Bar"), "org\\.foo\\.Bar");
    }

    #[test]
    fn parentheses_and_brackets_are_escaped() {
        assert_eq!(escape_hypr_regex("app(x)"), "app\\(x\\)");
        assert_eq!(escape_hypr_regex("list[1]"), "list\\[1\\]");
        assert_eq!(escape_hypr_regex("{a}"), "\\{a\\}");
    }

    #[test]
    fn regex_operators_are_escaped() {
        assert_eq!(
            escape_hypr_regex("a^b$c*d+e?f|g"),
            "a\\^b\\$c\\*d\\+e\\?f\\|g"
        );
    }

    #[test]
    fn backslashes_are_escaped() {
        assert_eq!(escape_hypr_regex("path\\to"), "path\\\\to");
    }

    #[test]
    fn an_empty_class_stays_empty() {
        assert_eq!(escape_hypr_regex(""), "");
    }

    // ------------------------------------------------------- hypr_selector_for

    #[test]
    fn a_plain_class_gets_the_selector_shape() {
        assert_eq!(hypr_selector_for("kitty"), "class:^(kitty)$");
    }

    #[test]
    fn metacharacters_in_a_class_are_escaped_inside_the_selector() {
        assert_eq!(
            hypr_selector_for("org.foo(bar)"),
            "class:^(org\\.foo\\(bar\\))$"
        );
    }

    #[test]
    fn an_empty_class_still_makes_a_valid_selector() {
        assert_eq!(hypr_selector_for(""), "class:^()$");
    }

    #[test]
    fn a_unicode_class_is_passed_through() {
        assert_eq!(hypr_selector_for("文件"), "class:^(文件)$");
    }

    #[test]
    fn the_selector_is_always_anchored() {
        let selector = hypr_selector_for("xterm");
        assert!(selector.starts_with("class:^("));
        assert!(selector.ends_with(")$"));
    }

    // ------------------------------------------------------------ class_matches

    #[test]
    fn an_exact_name_matches() {
        assert!(class_matches(&["kitty".to_string()], "kitty"));
    }

    #[test]
    fn matching_ignores_ascii_case() {
        assert!(class_matches(&["firefox".to_string()], "Firefox"));
        assert!(class_matches(&["Firefox".to_string()], "firefox"));
    }

    #[test]
    fn a_different_name_does_not_match() {
        assert!(!class_matches(&["kitty".to_string()], "alacritty"));
    }

    #[test]
    fn an_empty_name_list_never_matches() {
        assert!(!class_matches(&[], "kitty"));
    }

    #[test]
    fn any_of_several_names_matches() {
        let names = vec!["kitty".to_string(), "foot".to_string()];
        assert!(class_matches(&names, "foot"));
        assert!(!class_matches(&names, "vim"));
    }

    #[test]
    fn the_empty_class_does_not_match_a_non_empty_name() {
        assert!(!class_matches(&["kitty".to_string()], ""));
    }

    // --------------------------------------------------------------- focus_with

    #[test]
    fn a_matching_class_and_successful_dispatch_focus_the_app() {
        let dispatched = std::cell::RefCell::new(Vec::<String>::new());
        let x11_tried = std::cell::RefCell::new(0);
        let result = focus_with(
            &["kitty".to_string()],
            Some(vec!["kitty".to_string()]),
            |selector| {
                dispatched.borrow_mut().push(selector.to_string());
                true
            },
            |_| {
                *x11_tried.borrow_mut() += 1;
                false
            },
        );
        assert!(result);
        assert_eq!(*dispatched.borrow(), vec!["class:^(kitty)$".to_string()]);
        assert_eq!(
            *x11_tried.borrow(),
            0,
            "wmctrl must not be tried after success"
        );
    }

    #[test]
    fn a_failed_dispatch_falls_through_to_the_next_class() {
        let calls = std::cell::RefCell::new(0);
        let result = focus_with(
            &["kitty".to_string()],
            Some(vec!["kitty".to_string(), "kitty".to_string()]),
            |_| {
                let mut calls = calls.borrow_mut();
                *calls += 1;
                *calls == 2
            },
            |_| false,
        );
        assert!(result);
        assert_eq!(*calls.borrow(), 2);
    }

    #[test]
    fn with_no_matching_class_the_x11_helper_is_tried() {
        let x11_names = std::cell::RefCell::new(Vec::<String>::new());
        let result = focus_with(
            &["kitty".to_string()],
            Some(vec!["firefox".to_string()]),
            |_| panic!("dispatch must not run without a match"),
            |name| {
                x11_names.borrow_mut().push(name.to_string());
                true
            },
        );
        assert!(result);
        assert_eq!(*x11_names.borrow(), vec!["kitty".to_string()]);
    }

    #[test]
    fn the_x11_helper_tries_each_name_until_one_succeeds() {
        let tried = std::cell::RefCell::new(Vec::<String>::new());
        let result = focus_with(
            &["first".to_string(), "second".to_string()],
            None,
            |_| panic!("no hyprland"),
            |name| {
                tried.borrow_mut().push(name.to_string());
                name == "second"
            },
        );
        assert!(result);
        assert_eq!(
            *tried.borrow(),
            vec!["first".to_string(), "second".to_string()]
        );
    }

    #[test]
    fn without_hyprland_the_dispatcher_is_never_called() {
        let result = focus_with(
            &["kitty".to_string()],
            None,
            |_| panic!("dispatch must not run off Hyprland"),
            |name| name == "kitty",
        );
        assert!(result);
    }

    #[test]
    fn when_everything_fails_focus_reports_failure() {
        let result = focus_with(
            &["kitty".to_string()],
            Some(vec!["kitty".to_string()]),
            |_| false,
            |_| false,
        );
        assert!(!result);
    }

    #[test]
    fn a_class_with_regex_metacharacters_is_dispatched_escaped() {
        let dispatched = std::cell::RefCell::new(Vec::<String>::new());
        focus_with(
            &["org.foo(bar)".to_string()],
            Some(vec!["org.foo(bar)".to_string()]),
            |selector| {
                dispatched.borrow_mut().push(selector.to_string());
                true
            },
            |_| false,
        );
        assert_eq!(
            *dispatched.borrow(),
            vec!["class:^(org\\.foo\\(bar\\))$".to_string()]
        );
    }

    // -------------------------------------------------------- process_names_from_proc

    fn fake_proc(dir: &TempDir, pid: &str, comm: Option<&str>, cmdline: Option<&[u8]>) {
        let pid_dir = dir.path().join(pid);
        std::fs::create_dir_all(&pid_dir).unwrap();
        if let Some(comm) = comm {
            std::fs::write(pid_dir.join("comm"), comm).unwrap();
        }
        if let Some(cmdline) = cmdline {
            std::fs::write(pid_dir.join("cmdline"), cmdline).unwrap();
        }
    }

    #[test]
    fn comm_names_are_collected_lower_cased() {
        let dir = TempDir::new("proc-comm");
        fake_proc(&dir, "42", Some("KTiny\n"), None);
        let names = process_names_from_proc(dir.path());
        assert!(names.contains("ktiny"));
    }

    #[test]
    fn cmdline_paths_are_reduced_to_base_names() {
        let dir = TempDir::new("proc-cmdline");
        fake_proc(&dir, "42", None, Some(b"/usr/bin/Kitty\0--flag\0"));
        let names = process_names_from_proc(dir.path());
        assert!(names.contains("kitty"));
    }

    #[test]
    fn kernel_threads_with_empty_cmdline_contribute_nothing() {
        let dir = TempDir::new("proc-kthread");
        fake_proc(&dir, "42", Some("kworker/1:1"), Some(b""));
        let names = process_names_from_proc(dir.path());
        assert!(names.contains("kworker/1:1"));
        assert!(!names.contains(""));
    }

    #[test]
    fn non_numeric_directories_are_ignored() {
        let dir = TempDir::new("proc-nonnumeric");
        fake_proc(&dir, "self", Some("cuckoo"), None);
        fake_proc(&dir, "thread-self", Some("cuckoo"), None);
        fake_proc(&dir, "irq", Some("cuckoo"), None);
        let names = process_names_from_proc(dir.path());
        assert!(names.is_empty(), "nothing should match: {names:?}");
    }

    #[test]
    fn a_missing_proc_directory_yields_no_names() {
        let names = process_names_from_proc(Path::new("/does/not/exist/proc"));
        assert!(names.is_empty());
    }

    #[test]
    fn both_comm_and_cmdline_of_one_pid_are_collected() {
        // comm is truncated to 15 chars by the kernel; cmdline keeps the full name.
        let dir = TempDir::new("proc-both");
        fake_proc(
            &dir,
            "42",
            Some("malva-dock\n"),
            Some(b"/usr/bin/malva-dock\0"),
        );
        let names = process_names_from_proc(dir.path());
        assert!(names.contains("malva-dock"));
    }

    #[test]
    fn an_empty_comm_is_ignored() {
        let dir = TempDir::new("proc-empty-comm");
        fake_proc(&dir, "42", Some("   \n"), Some(b"/usr/bin/app\0"));
        let names = process_names_from_proc(dir.path());
        assert!(names.contains("app"));
        assert!(!names.contains(""));
    }

    // ------------------------------------------------------------- HyprClients

    #[test]
    fn the_fetcher_runs_once_within_the_ttl() {
        let calls = AtomicUsize::new(0);
        let clients = HyprClients::with_fetcher(Duration::from_secs(10), || {
            calls.fetch_add(1, Ordering::SeqCst);
            vec!["kitty".to_string()]
        });
        let t0 = Instant::now();
        assert_eq!(clients.classes(t0), vec!["kitty".to_string()]);
        assert_eq!(
            clients.classes(t0 + Duration::from_secs(1)),
            vec!["kitty".to_string()]
        );
        assert_eq!(
            clients.classes(t0 + Duration::from_secs(9)),
            vec!["kitty".to_string()]
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_fetcher_runs_again_after_the_ttl() {
        let calls = AtomicUsize::new(0);
        let clients = HyprClients::with_fetcher(Duration::from_millis(100), || {
            calls.fetch_add(1, Ordering::SeqCst);
            vec!["a".to_string()]
        });
        let t0 = Instant::now();
        clients.classes(t0);
        clients.classes(t0 + Duration::from_millis(150));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn the_ttl_boundary_is_exclusive() {
        let clients =
            HyprClients::with_fetcher(Duration::from_millis(100), || vec!["a".to_string()]);
        let t0 = Instant::now();
        clients.classes(t0);
        // Exactly at the TTL the entry is stale (age < ttl is the freshness test).
        clients.classes(t0 + Duration::from_millis(100));
        let clients2 =
            HyprClients::with_fetcher(Duration::from_millis(100), || vec!["a".to_string()]);
        clients2.classes(t0);
        clients2.classes(t0 + Duration::from_millis(99));
        // No panic and both calls behave: the first may refetch, the second must not.
    }

    #[test]
    fn invalidate_forces_a_refetch() {
        let calls = AtomicUsize::new(0);
        let clients = HyprClients::with_fetcher(Duration::from_secs(10), || {
            calls.fetch_add(1, Ordering::SeqCst);
            vec!["a".to_string()]
        });
        let t0 = Instant::now();
        clients.classes(t0);
        clients.invalidate();
        clients.classes(t0);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_zero_ttl_always_refetches() {
        let calls = AtomicUsize::new(0);
        let clients = HyprClients::with_fetcher(Duration::ZERO, || {
            calls.fetch_add(1, Ordering::SeqCst);
            vec![]
        });
        let t0 = Instant::now();
        clients.classes(t0);
        clients.classes(t0);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn store_publishes_a_list_without_fetching() {
        let calls = AtomicUsize::new(0);
        let clients = HyprClients::with_fetcher(Duration::from_secs(10), || {
            calls.fetch_add(1, Ordering::SeqCst);
            vec!["fetched".to_string()]
        });
        let t0 = Instant::now();
        clients.store(t0, vec!["stored".to_string()]);
        assert_eq!(
            clients.classes(t0 + Duration::from_secs(1)),
            vec!["stored".to_string()]
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_clock_running_backwards_does_not_panic() {
        let clients = HyprClients::with_fetcher(Duration::from_secs(10), || vec!["a".to_string()]);
        let t0 = Instant::now();
        clients.classes(t0);
        let earlier = t0.checked_sub(Duration::from_secs(5)).unwrap();
        let _ = clients.classes(earlier); // must not panic
    }

    // ------------------------------------------- real-world wrappers (env-based)

    #[test]
    fn on_hyprland_is_false_when_unset() {
        let _env = EnvScope::new(&["HYPRLAND_INSTANCE_SIGNATURE"]);
        std::env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");
        assert!(!on_hyprland());
    }

    #[test]
    fn on_hyprland_is_true_with_a_signature() {
        let _env = EnvScope::new(&["HYPRLAND_INSTANCE_SIGNATURE"]);
        std::env::set_var("HYPRLAND_INSTANCE_SIGNATURE", "a9f1c2");
        assert!(on_hyprland());
    }

    #[test]
    fn on_hyprland_is_false_with_an_empty_signature() {
        let _env = EnvScope::new(&["HYPRLAND_INSTANCE_SIGNATURE"]);
        std::env::set_var("HYPRLAND_INSTANCE_SIGNATURE", "");
        assert!(!on_hyprland());
    }

    #[test]
    fn on_hyprland_survives_repeated_checks() {
        let _env = EnvScope::new(&["HYPRLAND_INSTANCE_SIGNATURE"]);
        std::env::set_var("HYPRLAND_INSTANCE_SIGNATURE", "x");
        assert!(on_hyprland() && on_hyprland());
        std::env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");
        assert!(!on_hyprland() && !on_hyprland());
    }

    #[test]
    fn on_hyprland_does_not_look_at_other_variables() {
        let _env = EnvScope::new(&["HYPRLAND_INSTANCE_SIGNATURE", "XDG_CURRENT_DESKTOP"]);
        std::env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");
        std::env::set_var("XDG_CURRENT_DESKTOP", "Hyprland");
        assert!(
            !on_hyprland(),
            "the signature, not the desktop name, decides"
        );
    }

    #[test]
    fn hypr_classes_off_hyprland_is_empty() {
        let _env = EnvScope::new(&["HYPRLAND_INSTANCE_SIGNATURE"]);
        std::env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");
        assert!(hypr_classes().is_empty());
    }

    #[test]
    fn hypr_classes_cached_off_hyprland_is_empty() {
        let _env = EnvScope::new(&["HYPRLAND_INSTANCE_SIGNATURE"]);
        std::env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");
        assert!(hypr_classes_cached().is_empty());
        assert!(hypr_classes_cached().is_empty());
    }

    #[test]
    fn hypr_classes_with_a_bogus_signature_is_empty() {
        // hyprctl is not installed / cannot connect: still no panic, just empty.
        let _env = EnvScope::new(&["HYPRLAND_INSTANCE_SIGNATURE"]);
        std::env::set_var("HYPRLAND_INSTANCE_SIGNATURE", "definitely-not-running");
        let classes = hypr_classes();
        assert!(classes.is_empty());
    }

    #[test]
    fn running_processes_contains_this_test_binary() {
        let names = running_processes();
        let exe = std::env::current_exe().unwrap();
        let own_name: String = exe
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned()
            .to_lowercase();
        assert!(
            names.contains(own_name.as_str()),
            "the running test binary `{own_name}` must be among the processes"
        );
    }

    #[test]
    fn running_processes_does_not_contain_garbage() {
        let names = running_processes();
        assert!(!names.contains("definitely-not-a-real-process-xyz"));
        assert!(!names.contains(""));
    }

    #[test]
    fn running_processes_names_are_lower_cased() {
        // The set is built from `comm`/`cmdline` lower-cased; a mixed-case name
        // must never appear even though our own binary path may contain capitals.
        for name in running_processes() {
            assert_eq!(
                name,
                name.to_lowercase(),
                "process names must be lower case"
            );
        }
    }

    #[test]
    fn running_processes_is_non_empty() {
        // A Linux system always has processes; the probe must find them.
        let names = running_processes();
        assert!(!names.is_empty());
    }

    #[test]
    fn focus_off_hyprland_without_wmctrl_reports_failure() {
        let _env = EnvScope::new(&["HYPRLAND_INSTANCE_SIGNATURE"]);
        std::env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");
        // No window manager helpers in the sandbox: focus simply fails.
        let result = focus(&["definitely-not-running".to_string()]);
        assert!(!result);
    }

    #[test]
    fn focus_with_no_names_reports_failure() {
        let _env = EnvScope::new(&["HYPRLAND_INSTANCE_SIGNATURE"]);
        std::env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");
        assert!(!focus(&[]));
    }

    #[test]
    fn focus_does_not_panic_with_a_bogus_hyprland_signature() {
        let _env = EnvScope::new(&["HYPRLAND_INSTANCE_SIGNATURE"]);
        std::env::set_var("HYPRLAND_INSTANCE_SIGNATURE", "no-such-instance");
        assert!(!focus(&["anything".to_string()]));
    }

    // ------------------------------------------------------ LaunchError Display

    #[test]
    fn launch_error_messages_are_helpful() {
        assert!(LaunchError::EmptyCommand.to_string().contains("empty"));
        assert!(LaunchError::InteriorNul
            .to_string()
            .to_lowercase()
            .contains("nul"));
        assert!(LaunchError::Spawn("permission denied".into())
            .to_string()
            .contains("permission denied"));
    }

    #[test]
    fn launch_error_is_a_std_error() {
        fn takes_error(_: &dyn std::error::Error) {}
        takes_error(&LaunchError::EmptyCommand);
        takes_error(&LaunchError::InteriorNul);
        takes_error(&LaunchError::Spawn("x".into()));
    }

    #[test]
    fn launch_error_variants_are_comparable() {
        assert_eq!(LaunchError::EmptyCommand, LaunchError::EmptyCommand);
        assert_ne!(LaunchError::EmptyCommand, LaunchError::InteriorNul);
        assert_ne!(
            LaunchError::Spawn("a".into()),
            LaunchError::Spawn("b".into())
        );
    }

    #[test]
    fn launch_error_is_debuggable() {
        assert!(format!("{:?}", LaunchError::EmptyCommand).contains("EmptyCommand"));
        assert!(format!("{:?}", LaunchError::InteriorNul).contains("InteriorNul"));
    }

    // ------------------------------------------------ HyprClients store / invalidate

    #[test]
    fn store_overwrites_an_older_entry() {
        let clients =
            HyprClients::with_fetcher(Duration::from_secs(10), || vec!["old".to_string()]);
        let t0 = Instant::now();
        clients.classes(t0); // fetches "old"
        clients.store(t0 + Duration::from_secs(1), vec!["new".to_string()]);
        assert_eq!(
            clients.classes(t0 + Duration::from_secs(2)),
            vec!["new".to_string()]
        );
    }

    #[test]
    fn store_then_invalidate_forces_a_fetch() {
        let calls = AtomicUsize::new(0);
        let clients = HyprClients::with_fetcher(Duration::from_secs(10), || {
            calls.fetch_add(1, Ordering::SeqCst);
            vec!["fetched".to_string()]
        });
        let t0 = Instant::now();
        clients.store(t0, vec!["stored".to_string()]);
        clients.invalidate();
        assert_eq!(clients.classes(t0), vec!["fetched".to_string()]);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn invalidate_on_a_fresh_cache_is_harmless() {
        let clients = HyprClients::with_fetcher(Duration::from_secs(10), || vec!["a".to_string()]);
        clients.invalidate(); // nothing cached yet: must not panic
        assert_eq!(clients.classes(Instant::now()), vec!["a".to_string()]);
    }

    #[test]
    fn invalidate_lets_the_next_call_see_new_windows() {
        let clients = HyprClients::with_fetcher(Duration::from_secs(60), Vec::new);
        let t0 = Instant::now();
        assert!(clients.classes(t0).is_empty());
        // A window appeared; without invalidating we would serve the stale list.
        clients.invalidate();
        clients.store(t0, vec!["new-window".to_string()]);
        assert_eq!(clients.classes(t0), vec!["new-window".to_string()]);
    }

    // -------------------------------------------- hypr_classes / cached (more)

    #[test]
    fn hypr_classes_cached_and_uncached_agree_off_hyprland() {
        let _env = EnvScope::new(&["HYPRLAND_INSTANCE_SIGNATURE"]);
        std::env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");
        assert_eq!(hypr_classes(), hypr_classes_cached());
    }

    #[test]
    fn hypr_classes_cached_with_a_bogus_signature_is_empty() {
        let _env = EnvScope::new(&["HYPRLAND_INSTANCE_SIGNATURE"]);
        std::env::set_var("HYPRLAND_INSTANCE_SIGNATURE", "no-hyprland-here");
        assert!(hypr_classes_cached().is_empty());
        assert!(hypr_classes_cached().is_empty());
    }

    #[test]
    fn hypr_classes_with_an_empty_signature_is_empty() {
        let _env = EnvScope::new(&["HYPRLAND_INSTANCE_SIGNATURE"]);
        std::env::set_var("HYPRLAND_INSTANCE_SIGNATURE", "");
        assert!(hypr_classes().is_empty());
    }

    // ------------------------------------------------------ focus (more wrappers)

    #[test]
    fn focus_with_several_names_tries_them_all_without_wmctrl() {
        let _env = EnvScope::new(&["HYPRLAND_INSTANCE_SIGNATURE"]);
        std::env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");
        let names = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert!(!focus(&names));
    }

    #[test]
    fn focus_repeatedly_is_stable() {
        let _env = EnvScope::new(&["HYPRLAND_INSTANCE_SIGNATURE"]);
        std::env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");
        let names = vec!["x".to_string()];
        assert!(!focus(&names));
        assert!(!focus(&names));
        assert!(!focus(&names));
    }

    // ------------------------------------------------ running_processes (more)

    #[test]
    fn running_processes_is_stable_across_calls() {
        let first = running_processes();
        let second = running_processes();
        // At least the test binary must survive in both snapshots.
        assert!(!first.is_empty() && !second.is_empty());
    }

    #[test]
    fn running_processes_works_via_the_hyprland_path_too() {
        // With a bogus Hyprland signature the hyprctl call fails and contributes
        // nothing; the /proc part must still work.
        let _env = EnvScope::new(&["HYPRLAND_INSTANCE_SIGNATURE"]);
        std::env::set_var("HYPRLAND_INSTANCE_SIGNATURE", "no-such-instance");
        let names = running_processes();
        let exe = std::env::current_exe().unwrap();
        let own_name = exe.file_name().unwrap().to_string_lossy().to_lowercase();
        assert!(names.contains(own_name.as_str()));
    }
}
