//! The dock's runtime state: which apps are running, which are starting, and
//! what happens when an icon is clicked.
//!
//! The state owns the Slint model and copies flags into it; every method that
//! would touch the outside world (process probing, focusing, launching) has an
//! `*_with` variant taking injected functions, which is what the unit tests use.

use std::collections::HashSet;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{Model, VecModel};

use crate::apps::{self, LaunchError};
use crate::config::Entry;
use crate::geometry::{self, Slot};
use crate::{DockItem, DockWindow};

/// How long an icon hops after a click, if the app does not show up as running sooner.
pub const LAUNCH_HOP: Duration = Duration::from_secs(6);

/// The icons the dock ships with: Malva icon theme SVGs in `assets/icons`, mapped to these names
/// by the `Icons` global in `ui/dock.slint`. Keep the two lists in step.
pub const BUILT_IN_ICONS: [&str; 11] = [
    "apps",
    "files",
    "browser",
    "notes",
    "calculator",
    "fonts",
    "terminal",
    "keyboard",
    "settings",
    "downloads",
    "trash",
];

/// Maps a config `icon` value to `(key, image, has_image)`.
///
/// `key` is the built-in icon name for built-ins, or `"custom"` for image files
/// and letter tiles. `home` is injected so the `~` expansion is testable.
pub fn icon_for_with(icon: &str, home: Option<&str>) -> (String, slint::Image, bool) {
    if BUILT_IN_ICONS.contains(&icon) {
        return (icon.to_string(), slint::Image::default(), false);
    }
    if !icon.is_empty() {
        let path = crate::platform::expand_home_with(icon, home);
        match slint::Image::load_from_path(std::path::Path::new(&path)) {
            Ok(image) => return ("custom".to_string(), image, true),
            Err(_) => {
                eprintln!("malva-dock: cannot load icon {icon}, falling back to its initial");
            }
        }
    }
    ("custom".to_string(), slint::Image::default(), false)
}

/// [`icon_for_with`] using `$HOME`.
pub fn icon_for(icon: &str) -> (String, slint::Image, bool) {
    icon_for_with(icon, std::env::var("HOME").ok().as_deref())
}

/// Builds one UI item from a config entry and its resting slot.
pub fn make_item(entry: &Entry, slot: &Slot) -> DockItem {
    let (key, image, has_image) = icon_for(&entry.icon);
    let initial = entry
        .name
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();
    DockItem {
        key: key.as_str().into(),
        name: entry.name.as_str().into(),
        initial: initial.as_str().into(),
        image,
        has_image,
        u: slot.u,
        bw: slot.width,
        separator: entry.separator,
        running: false,
        launching: false,
    }
}

/// Lays out the entries and builds their initial UI items.
/// Returns the slots, the resting content width and the items.
pub fn build_items(entries: &[Entry], size: f32) -> (Vec<Slot>, f32, Vec<DockItem>) {
    let (slots, content_width) = geometry::layout(entries, size);
    let items = entries
        .iter()
        .zip(slots.iter())
        .map(|(entry, slot)| make_item(entry, slot))
        .collect();
    (slots, content_width, items)
}

/// The dock's runtime state.
pub struct State {
    entries: Vec<Entry>,
    slots: Vec<Slot>,
    model: Rc<VecModel<DockItem>>,
    running: Vec<bool>,
    launched_at: Vec<Option<Instant>>,
}

impl State {
    /// Creates the state for a laid-out set of entries.
    pub fn new(entries: Vec<Entry>, slots: Vec<Slot>, model: Rc<VecModel<DockItem>>) -> Self {
        let count = entries.len();
        debug_assert_eq!(slots.len(), count, "one slot per entry");
        State {
            entries,
            slots,
            model,
            running: vec![false; count],
            launched_at: vec![None; count],
        }
    }

    /// The resting slots, used to resolve clicks.
    pub fn slots(&self) -> &[Slot] {
        &self.slots
    }

    /// The entry list, for tests and callers that need it.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Re-reads which apps are running, using the real process probe.
    pub fn detect_running(&mut self) {
        let processes = apps::running_processes();
        self.detect_running_with(&processes);
        self.push();
    }

    /// Updates the running flags from an injected process-name set.
    pub fn detect_running_with(&mut self, processes: &HashSet<String>) {
        for (i, entry) in self.entries.iter().enumerate() {
            if entry.separator {
                continue;
            }
            let running = entry
                .process_names()
                .iter()
                .any(|name| processes.contains(name));
            self.running[i] = running;
            if running {
                self.launched_at[i] = None;
            }
        }
    }

    /// Copies the running / launching flags into the UI model where they changed,
    /// expiring launch hops that have outlived [`LAUNCH_HOP`].
    pub fn push(&mut self) {
        self.push_with_now(Instant::now());
    }

    /// [`push`] with an injected clock.
    pub fn push_with_now(&mut self, now: Instant) {
        for i in 0..self.entries.len() {
            if let Some(launched) = self.launched_at[i] {
                if now.duration_since(launched) > LAUNCH_HOP {
                    self.launched_at[i] = None;
                }
            }
            let launching = self.launched_at[i].is_some();
            let running = self.running[i];
            if let Some(mut row) = self.model.row_data(i) {
                if row.running != running || row.launching != launching {
                    row.running = running;
                    row.launching = launching;
                    self.model.set_row_data(i, row);
                }
            }
        }
    }

    /// True while the entry's icon is still hopping after a launch.
    pub fn is_launching(&self, index: usize) -> bool {
        self.launched_at.get(index).is_some_and(|at| at.is_some())
    }

    /// True when the entry is currently detected as running.
    pub fn is_running(&self, index: usize) -> bool {
        self.running.get(index).copied().unwrap_or(false)
    }

    /// An icon was clicked: raise the app if it is running, otherwise start it.
    pub fn activate(&mut self, index: usize) {
        self.activate_with(index, &apps::focus, &|exec| {
            let result = apps::launch(exec);
            if let Err(err) = &result {
                eprintln!("malva-dock: cannot run `{exec}`: {err}");
            }
            result
        });
    }

    /// [`activate`] with injected focus and launch functions (used by the tests).
    ///
    /// Out-of-range and separator indices are ignored instead of panicking: the
    /// click position comes from the UI and must never be able to crash the dock.
    pub fn activate_with(
        &mut self,
        index: usize,
        focus: &impl Fn(&[String]) -> bool,
        launch: &impl Fn(&str) -> Result<(), LaunchError>,
    ) {
        if index >= self.entries.len() || self.entries[index].separator {
            return;
        }
        let names = self.entries[index].process_names();
        if self.running[index] && focus(&names) {
            return;
        }
        // Only hop when something was actually started; a failed launch has
        // nothing to celebrate.
        if launch(&self.entries[index].exec).is_ok() {
            self.launched_at[index] = Some(Instant::now());
            self.push();
        }
    }
}

/// Wires a [`State`] into a [`DockWindow`]: model, geometry and the click callback.
/// Returns the shared state.
pub fn attach(ui: &DockWindow, state: Rc<std::cell::RefCell<State>>) {
    ui.on_item_clicked({
        let state = state.clone();
        move |p| {
            let mut state = state.borrow_mut();
            if let Some(index) = geometry::hit(state.slots(), p) {
                state.activate(index);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{TempDir, TINY_PNG};

    fn entry(name: &str, exec: &str) -> Entry {
        Entry {
            name: name.to_string(),
            icon: String::new(),
            exec: exec.to_string(),
            matches: None,
            separator: false,
        }
    }

    fn separator_entry() -> Entry {
        Entry {
            name: "separator".to_string(),
            icon: String::new(),
            exec: String::new(),
            matches: None,
            separator: true,
        }
    }

    fn model_of(entries: &[Entry], size: f32) -> (Rc<VecModel<DockItem>>, State, f32) {
        let (slots, content_width, items) = build_items(entries, size);
        let model = Rc::new(VecModel::from(items));
        let state = State::new(entries.to_vec(), slots, model.clone());
        (model, state, content_width)
    }

    fn processes(names: &[&str]) -> HashSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    // ---------------------------------------------------------------- make_item

    #[test]
    fn an_entry_without_an_icon_uses_the_custom_tile() {
        let slot = Slot {
            u: 0.0,
            width: 56.0,
            separator: false,
        };
        let item = make_item(&entry("Files", "nautilus"), &slot);
        assert_eq!(item.key.as_str(), "custom");
        assert!(!item.has_image);
    }

    #[test]
    fn an_entry_with_a_built_in_icon_uses_that_icon() {
        let slot = Slot {
            u: 0.0,
            width: 56.0,
            separator: false,
        };
        let mut e = entry("Files", "nautilus");
        e.icon = "files".to_string();
        let item = make_item(&e, &slot);
        assert_eq!(item.key.as_str(), "files");
        assert!(!item.has_image);
    }

    #[test]
    fn the_initial_is_the_first_letter_upper_cased() {
        let slot = Slot {
            u: 0.0,
            width: 56.0,
            separator: false,
        };
        let mut e = entry("terminal", "kitty");
        e.icon = "terminal".to_string();
        let item = make_item(&e, &slot);
        assert_eq!(item.initial.as_str(), "T");
        assert_eq!(item.key.as_str(), "terminal");
    }

    #[test]
    fn an_empty_name_gives_an_empty_initial() {
        let slot = Slot {
            u: 0.0,
            width: 56.0,
            separator: false,
        };
        let item = make_item(&entry("", "x"), &slot);
        assert_eq!(item.initial.as_str(), "");
    }

    #[test]
    fn a_unicode_initial_is_upper_cased() {
        let slot = Slot {
            u: 0.0,
            width: 56.0,
            separator: false,
        };
        let item = make_item(&entry("éditeur", "x"), &slot);
        assert_eq!(item.initial.as_str(), "É");
    }

    #[test]
    fn the_resting_geometry_is_copied_into_the_item() {
        let slot = Slot {
            u: 132.0,
            width: 56.0,
            separator: false,
        };
        let item = make_item(&entry("X", "x"), &slot);
        assert_eq!(item.u, 132.0);
        assert_eq!(item.bw, 56.0);
    }

    #[test]
    fn a_separator_item_is_flagged_and_not_running() {
        let slot = Slot {
            u: 0.0,
            width: geometry::SEPARATOR_WIDTH,
            separator: true,
        };
        let item = make_item(&separator_entry(), &slot);
        assert!(item.separator);
        assert!(!item.running);
        assert!(!item.launching);
    }

    #[test]
    fn items_start_neither_running_nor_launching() {
        let slot = Slot {
            u: 0.0,
            width: 56.0,
            separator: false,
        };
        let item = make_item(&entry("X", "x"), &slot);
        assert!(!item.running);
        assert!(!item.launching);
    }

    // --------------------------------------------------------------- build_items

    #[test]
    fn build_items_lays_out_and_builds_consistently() {
        let entries = vec![entry("A", "a"), separator_entry(), entry("B", "b")];
        let (slots, width, items) = build_items(&entries, 56.0);
        assert_eq!(slots.len(), 3);
        assert_eq!(items.len(), 3);
        assert!(width > 0.0);
        assert_eq!(items[0].u, slots[0].u);
        assert_eq!(items[2].bw, slots[2].width);
    }

    #[test]
    fn build_items_with_no_entries_is_empty() {
        let (slots, width, items) = build_items(&[], 56.0);
        assert!(slots.is_empty());
        assert_eq!(width, 0.0);
        assert!(items.is_empty());
    }

    // ---------------------------------------------------------------- icon_for_with

    #[test]
    fn every_built_in_icon_name_maps_to_itself() {
        for name in BUILT_IN_ICONS {
            let (key, _, has_image) = icon_for_with(name, Some("/home/z"));
            assert_eq!(key, name, "built-in {name} must map to itself");
            assert!(!has_image);
        }
    }

    #[test]
    fn an_unknown_icon_name_falls_back_to_the_letter_tile() {
        let (key, _, has_image) = icon_for_with("not-a-known-icon", Some("/home/z"));
        assert_eq!(key, "custom");
        assert!(!has_image);
    }

    #[test]
    fn an_empty_icon_falls_back_to_the_letter_tile() {
        let (key, _, has_image) = icon_for_with("", Some("/home/z"));
        assert_eq!(key, "custom");
        assert!(!has_image);
    }

    #[test]
    fn a_png_file_loads_as_an_image() {
        let dir = TempDir::new("icon-png");
        let path = dir.path().join("icon.png");
        std::fs::write(&path, TINY_PNG).unwrap();
        let (key, image, has_image) = icon_for_with(path.to_str().unwrap(), Some("/home/z"));
        assert_eq!(key, "custom");
        assert!(has_image);
        assert!(image.size().width > 0);
    }

    #[test]
    fn a_missing_file_falls_back_without_panicking() {
        let (key, _, has_image) = icon_for_with("/does/not/exist.png", Some("/home/z"));
        assert_eq!(key, "custom");
        assert!(!has_image);
    }

    #[test]
    fn a_tilde_path_is_expanded_against_the_home_argument() {
        let dir = TempDir::new("icon-tilde");
        let path = dir.path().join("icon.png");
        std::fs::write(&path, TINY_PNG).unwrap();
        // Pretend the temp dir is the home directory.
        let home = dir.path().to_str().unwrap().to_string();
        let relative = "~/icon.png".to_string();
        let (key, _, has_image) = icon_for_with(&relative, Some(&home));
        assert_eq!(key, "custom");
        assert!(has_image);
    }

    #[test]
    fn a_tilde_path_without_home_cannot_load() {
        let (key, _, has_image) = icon_for_with("~/icon.png", None);
        assert_eq!(key, "custom");
        assert!(!has_image);
    }

    // ---------------------------------------------------------- detect_running_with

    #[test]
    fn with_no_processes_running_nothing_is_running() {
        let (model, mut state, _) = model_of(&[entry("A", "a")], 56.0);
        state.detect_running_with(&processes(&[]));
        state.push_with_now(Instant::now());
        assert!(!state.is_running(0));
        assert!(!model.row_data(0).unwrap().running);
    }

    #[test]
    fn a_matching_process_marks_the_entry_running() {
        let (model, mut state, _) = model_of(&[entry("Terminal", "kitty")], 56.0);
        state.detect_running_with(&processes(&["kitty"]));
        state.push_with_now(Instant::now());
        assert!(state.is_running(0));
        assert!(model.row_data(0).unwrap().running);
    }

    #[test]
    fn a_stopped_process_clears_the_running_flag() {
        let (_, mut state, _) = model_of(&[entry("Terminal", "kitty")], 56.0);
        state.detect_running_with(&processes(&["kitty"]));
        assert!(state.is_running(0));
        state.detect_running_with(&processes(&["vim"]));
        assert!(!state.is_running(0));
    }

    #[test]
    fn separators_are_never_running() {
        let (_, mut state, _) = model_of(&[separator_entry()], 56.0);
        state.detect_running_with(&processes(&["separator"]));
        assert!(!state.is_running(0));
    }

    #[test]
    fn a_running_app_stops_the_launch_hop() {
        let (_, mut state, _) = model_of(&[entry("Terminal", "kitty")], 56.0);
        state.activate_with(0, &|_| false, &|_| Ok(()));
        assert!(state.is_launching(0));
        state.detect_running_with(&processes(&["kitty"]));
        assert!(
            !state.is_launching(0),
            "the hop must stop once the app is running"
        );
    }

    #[test]
    fn an_entry_with_no_process_names_is_never_running() {
        let mut e = entry("Downloads", "xdg-open ~/Downloads");
        e.matches = Some(Vec::new());
        let (_, mut state, _) = model_of(&[e], 56.0);
        state.detect_running_with(&processes(&["xdg-open"]));
        assert!(!state.is_running(0));
    }

    #[test]
    fn only_the_matching_entry_is_marked() {
        let entries = vec![entry("A", "aaa"), entry("B", "bbb")];
        let (_, mut state, _) = model_of(&entries, 56.0);
        state.detect_running_with(&processes(&["bbb"]));
        assert!(!state.is_running(0));
        assert!(state.is_running(1));
    }

    // ------------------------------------------------------------- push_with_now

    #[test]
    fn a_fresh_launch_is_in_the_launching_state() {
        let (model, mut state, _) = model_of(&[entry("A", "a")], 56.0);
        let start = Instant::now();
        state.activate_with(0, &|_| false, &|_| Ok(()));
        state.push_with_now(start + Duration::from_secs(1));
        assert!(state.is_launching(0));
        assert!(model.row_data(0).unwrap().launching);
    }

    #[test]
    fn the_launch_hop_expires_after_six_seconds() {
        let (_, mut state, _) = model_of(&[entry("A", "a")], 56.0);
        let start = Instant::now();
        state.activate_with(0, &|_| false, &|_| Ok(()));
        state.push_with_now(start + LAUNCH_HOP + Duration::from_secs(1));
        assert!(!state.is_launching(0));
    }

    #[test]
    fn the_launch_hop_is_still_alive_exactly_at_the_boundary() {
        let (_, mut state, _) = model_of(&[entry("A", "a")], 56.0);
        // `start` is taken *before* the launch, so `start + LAUNCH_HOP` is at or
        // after the true six-second mark by only a few nanoseconds: still within
        // the hop, which lasts strictly longer than six seconds.
        let start = Instant::now();
        state.activate_with(0, &|_| false, &|_| Ok(()));
        state.push_with_now(start + LAUNCH_HOP);
        assert!(
            state.is_launching(0),
            "the hop lasts *longer than* six seconds"
        );
    }

    #[test]
    fn the_model_reflects_running_changes() {
        let (model, mut state, _) = model_of(&[entry("A", "a")], 56.0);
        assert!(!model.row_data(0).unwrap().running);
        state.detect_running_with(&processes(&["a"]));
        state.push_with_now(Instant::now());
        assert!(model.row_data(0).unwrap().running);
    }

    #[test]
    fn the_model_reflects_launching_expiry() {
        let (model, mut state, _) = model_of(&[entry("A", "a")], 56.0);
        let start = Instant::now();
        state.activate_with(0, &|_| false, &|_| Ok(()));
        assert!(model.row_data(0).unwrap().launching);
        state.push_with_now(start + LAUNCH_HOP + Duration::from_secs(1));
        assert!(!model.row_data(0).unwrap().launching);
    }

    #[test]
    fn push_leaves_other_rows_alone() {
        let entries = vec![entry("A", "a"), entry("B", "b")];
        let (model, mut state, _) = model_of(&entries, 56.0);
        state.detect_running_with(&processes(&["b"]));
        state.push_with_now(Instant::now());
        assert!(!model.row_data(0).unwrap().running);
        assert!(model.row_data(1).unwrap().running);
    }

    // --------------------------------------------------------------- activate_with

    #[test]
    fn a_click_on_a_stopped_app_launches_it_and_hops() {
        let launched = std::cell::RefCell::new(Vec::<String>::new());
        let (model, mut state, _) = model_of(&[entry("Terminal", "kitty")], 56.0);
        state.activate_with(
            0,
            &|_| panic!("focus must not run for a stopped app"),
            &|exec| {
                launched.borrow_mut().push(exec.to_string());
                Ok(())
            },
        );
        assert_eq!(*launched.borrow(), vec!["kitty".to_string()]);
        assert!(state.is_launching(0));
        assert!(model.row_data(0).unwrap().launching);
    }

    #[test]
    fn a_click_on_a_running_app_focuses_instead_of_launching() {
        let launched = std::cell::RefCell::new(0);
        let focused = std::cell::RefCell::new(Vec::<Vec<String>>::new());
        let (_, mut state, _) = model_of(&[entry("Terminal", "kitty")], 56.0);
        state.detect_running_with(&processes(&["kitty"]));
        state.activate_with(
            0,
            &|names| {
                focused.borrow_mut().push(names.to_vec());
                true
            },
            &|_| {
                *launched.borrow_mut() += 1;
                Ok(())
            },
        );
        assert_eq!(*launched.borrow(), 0, "no new instance may be started");
        assert_eq!(*focused.borrow(), vec![vec!["kitty".to_string()]]);
        assert!(!state.is_launching(0));
    }

    #[test]
    fn a_failed_focus_starts_a_new_instance() {
        let launched = std::cell::RefCell::new(0);
        let (_, mut state, _) = model_of(&[entry("Terminal", "kitty")], 56.0);
        state.detect_running_with(&processes(&["kitty"]));
        state.activate_with(0, &|_| false, &|_| {
            *launched.borrow_mut() += 1;
            Ok(())
        });
        assert_eq!(*launched.borrow(), 1);
    }

    #[test]
    fn a_click_on_a_separator_does_nothing() {
        let launched = std::cell::RefCell::new(0);
        let (_, mut state, _) = model_of(&[separator_entry()], 56.0);
        state.activate_with(0, &|_| panic!("separators are not clickable"), &|_| {
            *launched.borrow_mut() += 1;
            Ok(())
        });
        assert_eq!(*launched.borrow(), 0);
        assert!(!state.is_launching(0));
    }

    #[test]
    fn an_out_of_range_click_is_ignored_instead_of_panicking() {
        let (_, mut state, _) = model_of(&[entry("A", "a")], 56.0);
        // The original code indexed the entry vector directly and would panic.
        state.activate_with(999, &|_| false, &|_| Ok(()));
        assert!(!state.is_launching(0));
    }

    #[test]
    fn a_failed_launch_does_not_hop() {
        let (model, mut state, _) = model_of(&[entry("Broken", "")], 56.0);
        state.activate_with(0, &|_| false, &|_| Err(LaunchError::EmptyCommand));
        assert!(
            !state.is_launching(0),
            "nothing started, nothing to hop for"
        );
        assert!(!model.row_data(0).unwrap().launching);
    }

    #[test]
    fn focus_receives_the_entrys_process_names() {
        let seen = std::cell::RefCell::new(Vec::<Vec<String>>::new());
        let mut e = entry("Apps", "wofi --show drun");
        e.matches = Some(vec!["wofi".to_string(), "rofi".to_string()]);
        let (_, mut state, _) = model_of(&[e], 56.0);
        state.detect_running_with(&processes(&["wofi"]));
        state.activate_with(
            0,
            &|names| {
                seen.borrow_mut().push(names.to_vec());
                true
            },
            &|_| Ok(()),
        );
        assert_eq!(
            *seen.borrow(),
            vec![vec!["wofi".to_string(), "rofi".to_string()]]
        );
    }

    // --------------------------------------------------------- detect_running (real)

    #[test]
    fn detect_running_probes_the_real_processes_without_panicking() {
        let (_, mut state, _) = model_of(&[entry("Ghost", "definitely-not-running-xyz")], 56.0);
        state.detect_running();
        assert!(!state.is_running(0));
    }

    #[test]
    fn detect_running_finds_the_test_process_itself() {
        // The test binary is a running process; an entry watching it must light up.
        let exe = std::env::current_exe().unwrap();
        let own_name = exe
            .file_name()
            .unwrap()
            .to_string_lossy()
            .rsplit('/')
            .next()
            .unwrap()
            .to_lowercase();
        let (_, mut state, _) = model_of(&[entry("Self", &own_name)], 56.0);
        state.detect_running();
        assert!(
            state.is_running(0),
            "expected `{own_name}` among running processes"
        );
    }

    #[test]
    fn detect_running_updates_the_model() {
        let (model, mut state, _) = model_of(&[entry("Ghost", "definitely-not-running-xyz")], 56.0);
        state.detect_running();
        assert!(!model.row_data(0).unwrap().running);
    }

    // --------------------------------------------------------------- build_items

    #[test]
    fn build_items_matches_layout_for_separators() {
        let entries = vec![entry("A", "a"), separator_entry(), entry("B", "b")];
        let (slots, _, items) = build_items(&entries, 56.0);
        assert!(items[1].separator);
        assert_eq!(items[1].bw, geometry::SEPARATOR_WIDTH);
        assert_eq!(items[1].u, slots[1].u);
        assert!(!items[1].has_image);
    }

    #[test]
    fn build_items_content_width_matches_the_layout_total() {
        let entries = vec![entry("A", "a"), entry("B", "b"), separator_entry()];
        let (slots, content_width, _) = build_items(&entries, 56.0);
        let last = slots.last().unwrap();
        assert!((content_width - (last.u + last.width)).abs() < 1e-4);
    }

    #[test]
    fn build_items_sanitises_a_hostile_icon_size() {
        let entries = vec![entry("A", "a")];
        let (slots, _, items) = build_items(&entries, f32::NAN);
        assert!(slots[0].width.is_finite());
        assert!(items[0].bw.is_finite());
        assert_eq!(items[0].bw, crate::config::DEFAULT_ICON_SIZE);
    }

    #[test]
    fn build_items_carries_names_into_the_model() {
        let entries = vec![entry("Terminal", "kitty"), separator_entry()];
        let (_, _, items) = build_items(&entries, 56.0);
        assert_eq!(items[0].name.as_str(), "Terminal");
        assert_eq!(items[1].name.as_str(), "separator");
    }

    // ------------------------------------------------------------- icon_for (env)

    #[test]
    fn icon_for_uses_home_from_the_environment() {
        let _env = crate::testutil::EnvScope::new(&["HOME"]);
        let dir = TempDir::new("icon-for-env");
        std::fs::write(dir.path().join("icon.png"), TINY_PNG).unwrap();
        std::env::set_var("HOME", dir.path());
        let (key, _, has_image) = icon_for("~/icon.png");
        assert_eq!(key, "custom");
        assert!(has_image);
    }

    #[test]
    fn icon_for_falls_back_without_a_home() {
        let _env = crate::testutil::EnvScope::new(&["HOME"]);
        std::env::remove_var("HOME");
        let (key, _, has_image) = icon_for("~/icon.png");
        assert_eq!(key, "custom");
        assert!(!has_image);
    }

    #[test]
    fn icon_for_maps_every_built_in_name_from_the_environment() {
        for name in BUILT_IN_ICONS {
            let (key, _, has_image) = icon_for(name);
            assert_eq!(key, name);
            assert!(!has_image);
        }
    }

    #[test]
    fn icon_for_falls_back_for_a_missing_file_from_the_environment() {
        let (key, _, has_image) = icon_for("/definitely/not/here.png");
        assert_eq!(key, "custom");
        assert!(!has_image);
    }

    #[test]
    fn icon_for_treats_an_absolute_path_normally() {
        let dir = TempDir::new("icon-absolute");
        std::fs::write(dir.path().join("icon.png"), TINY_PNG).unwrap();
        let (key, _, has_image) = icon_for(dir.path().join("icon.png").to_str().unwrap());
        assert_eq!(key, "custom");
        assert!(has_image);
    }

    // --------------------------------------------------- State::push (real clock)

    #[test]
    fn push_publishes_running_flags_to_the_model() {
        let (model, mut state, _) = model_of(&[entry("A", "aaa")], 56.0);
        state.detect_running_with(&processes(&["aaa"]));
        state.push();
        assert!(model.row_data(0).unwrap().running);
    }

    #[test]
    fn push_publishes_launching_flags_to_the_model() {
        let (model, mut state, _) = model_of(&[entry("A", "a")], 56.0);
        state.activate_with(0, &|_| false, &|_| Ok(()));
        state.push();
        assert!(model.row_data(0).unwrap().launching);
    }

    #[test]
    fn push_is_idempotent() {
        let (model, mut state, _) = model_of(&[entry("A", "aaa")], 56.0);
        state.detect_running_with(&processes(&["aaa"]));
        state.push();
        let once = model.row_data(0).unwrap();
        state.push();
        let twice = model.row_data(0).unwrap();
        assert_eq!(once.running, twice.running);
        assert_eq!(once.launching, twice.launching);
    }

    #[test]
    fn push_on_an_empty_dock_does_nothing() {
        let model = Rc::new(VecModel::from(Vec::<DockItem>::new()));
        let (slots, _, _) = build_items(&[], 56.0);
        let mut state = State::new(Vec::new(), slots, model);
        state.push(); // must not panic
    }

    #[test]
    fn push_skips_separators_safely() {
        let (model, mut state, _) = model_of(&[separator_entry()], 56.0);
        state.detect_running_with(&processes(&[]));
        state.push();
        assert!(model.row_data(0).unwrap().separator);
        assert!(!model.row_data(0).unwrap().running);
    }

    // ---------------------------------------------- State::activate (real world)

    #[test]
    fn activate_really_launches_a_command_and_hops() {
        let (model, mut state, _) = model_of(&[entry("True", "true")], 56.0);
        state.activate(0);
        assert!(state.is_launching(0), "`true` must have started");
        assert!(model.row_data(0).unwrap().launching);
    }

    #[test]
    fn activate_with_an_empty_exec_does_not_hop() {
        let (model, mut state, _) = model_of(&[entry("Broken", "")], 56.0);
        state.activate(0);
        assert!(!state.is_launching(0));
        assert!(!model.row_data(0).unwrap().launching);
    }

    #[test]
    fn activate_with_a_nul_exec_does_not_panic() {
        let (_, mut state, _) = model_of(&[entry("Nully", "true\0false")], 56.0);
        state.activate(0); // used to be reachable as a Command::arg panic
        assert!(!state.is_launching(0));
    }

    #[test]
    fn activate_with_a_failing_command_still_spawned_and_hops() {
        let (_, mut state, _) = model_of(&[entry("False", "false")], 56.0);
        state.activate(0);
        assert!(state.is_launching(0), "`sh -c false` started fine");
    }

    #[test]
    fn activate_on_a_running_app_without_a_way_to_focus_relaunches() {
        // Off Hyprland and without wmctrl, focus() fails, so a new instance starts.
        let (_, mut state, _) = model_of(&[entry("True", "true")], 56.0);
        state.detect_running_with(&processes(&["true"]));
        assert!(state.is_running(0));
        state.activate(0);
        assert!(state.is_launching(0));
    }

    #[test]
    fn activate_out_of_range_and_on_separators_is_ignored() {
        let (_, mut state, _) = model_of(&[separator_entry()], 56.0);
        state.activate(0);
        state.activate(7);
        assert!(!state.is_launching(0));
    }

    // --------------------------------------------------------- trivial accessors

    #[test]
    fn slots_exposes_the_resting_layout() {
        let entries = vec![entry("A", "a"), entry("B", "b")];
        let (slots, _, _) = build_items(&entries, 56.0);
        let model = Rc::new(VecModel::from(Vec::<DockItem>::new()));
        let state = State::new(entries.clone(), slots.clone(), model);
        assert_eq!(state.slots(), slots.as_slice());
    }

    #[test]
    fn entries_exposes_the_config_entries() {
        let entries = vec![entry("A", "a")];
        let (slots, _, _) = build_items(&entries, 56.0);
        let model = Rc::new(VecModel::from(Vec::<DockItem>::new()));
        let state = State::new(entries.clone(), slots, model);
        assert_eq!(state.entries(), entries.as_slice());
    }

    #[test]
    fn out_of_range_accessors_are_safe() {
        let entries = vec![entry("A", "a")];
        let (slots, _, _) = build_items(&entries, 56.0);
        let model = Rc::new(VecModel::from(Vec::<DockItem>::new()));
        let state = State::new(entries, slots, model);
        assert!(!state.is_running(999));
        assert!(!state.is_launching(999));
        assert!(!state.is_running(usize::MAX));
    }

    #[test]
    fn a_fresh_state_has_nothing_running_or_launching() {
        let entries = vec![entry("A", "a"), separator_entry(), entry("B", "b")];
        let (slots, _, _) = build_items(&entries, 56.0);
        let model = Rc::new(VecModel::from(Vec::<DockItem>::new()));
        let state = State::new(entries, slots, model);
        for i in 0..3 {
            assert!(!state.is_running(i));
            assert!(!state.is_launching(i));
        }
    }
}
