//! Malva Dock: a macOS-style dock written in Rust with Slint.
//!
//! This binary only parses arguments, wires the [`malva_dock`] library into the
//! Slint window and runs the event loop; everything else lives in the library,
//! where it is unit-tested.

#![deny(unsafe_code)]

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use slint::{ComponentHandle, ModelRc, VecModel};

use malva_dock::{config, geometry, platform, state, DockItem, DockWindow, Theme};

const HELP: &str = "\
Malva Dock

Usage: malva-dock [options]

Options:
  --config PATH   read this config file instead of ~/.config/malva/dock.conf
  --light         use the light theme
  --dark          use the dark theme
  --print-config  print the built-in config, which you can save and edit
  --version       print the version
  -h, --help      show this help
";

/// What the command line asks the dock to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Start the dock with an optional config path and theme override.
    Run {
        /// Explicit `--config PATH`, if given.
        config: Option<PathBuf>,
        /// `Some(true)` for `--dark`, `Some(false)` for `--light`.
        theme: Option<bool>,
    },
    /// `--print-config`: print the built-in config and exit.
    PrintConfig,
    /// `-h` / `--help`: print the help and exit.
    Help,
    /// `--version`: print the version and exit.
    Version,
}

/// Parses a list of command-line arguments (without the program name).
///
/// Unknown options and a missing `--config` value are errors; the caller turns
/// them into a usage message with exit code 2 (the original silently ignored
/// unknown options and silently dropped a valueless `--config`).
pub fn parse_args_from(args: Vec<String>) -> Result<Action, String> {
    let mut config = None;
    let mut theme = None;
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--config" => match iter.next() {
                Some(path) => config = Some(PathBuf::from(path)),
                None => return Err("--config needs a path".to_string()),
            },
            "--light" => theme = Some(false),
            "--dark" => theme = Some(true),
            "--print-config" => return Ok(Action::PrintConfig),
            "--version" | "-V" => return Ok(Action::Version),
            "-h" | "--help" => return Ok(Action::Help),
            other => {
                // Also accept the `--config=PATH` spelling.
                if let Some(path) = other.strip_prefix("--config=") {
                    if !path.is_empty() {
                        config = Some(PathBuf::from(path));
                        continue;
                    }
                }
                return Err(format!("unknown option {other} (try --help)"));
            }
        }
    }
    Ok(Action::Run { config, theme })
}

fn main() -> Result<(), slint::PlatformError> {
    let action = match parse_args_from(std::env::args().skip(1).collect()) {
        Ok(action) => action,
        Err(message) => {
            eprintln!("malva-dock: {message}");
            eprint!("{HELP}");
            std::process::exit(2);
        }
    };

    let (run_config, theme_override) = match action {
        Action::PrintConfig => {
            print!("{}", config::DEFAULT_CONFIG);
            return Ok(());
        }
        Action::Help => {
            print!("{HELP}");
            return Ok(());
        }
        Action::Version => {
            println!("malva-dock {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Action::Run { config, theme } => (config, theme),
    };

    let (config, warnings) = config::load_detailed(run_config);
    for warning in &warnings {
        eprintln!("malva-dock: {warning}");
    }
    let dark = theme_override.unwrap_or(config.dark);

    let (slots, content_width, items) = state::build_items(&config.entries, config.size);
    let model: Rc<VecModel<DockItem>> = Rc::new(VecModel::from(items));

    let ui = DockWindow::new()?;
    ui.set_items(ModelRc::from(model.clone()));
    ui.set_content_width(content_width);
    ui.set_icon_size(config.size);
    ui.set_gap(geometry::GAP);
    ui.set_max_scale(config.magnification);
    ui.global::<Theme>().set_dark(dark);

    let state = Rc::new(RefCell::new(state::State::new(
        config.entries.clone(),
        slots,
        model,
    )));
    state::attach(&ui, state.clone());

    // Keep the running dots up to date.
    state.borrow_mut().detect_running();
    let poll = slint::Timer::default();
    poll.start(slint::TimerMode::Repeated, Duration::from_millis(1500), {
        let state = state.clone();
        move || state.borrow_mut().detect_running()
    });

    // On X11, sit at the bottom centre of the screen. The window size is computed
    // from the config (mirroring the formulas in ui/dock.slint) instead of being
    // read back from a window that may not have been laid out yet, and scaled by
    // the window scale factor for HiDPI.
    if let Some((screen_width, screen_height)) = platform::x11_screen_size() {
        let weak = ui.as_weak();
        let (logical_w, logical_h) = platform::window_logical_size(
            content_width,
            config.size,
            geometry::GAP,
            config.magnification,
        );
        slint::Timer::single_shot(Duration::from_millis(250), move || {
            if let Some(ui) = weak.upgrade() {
                let scale = ui.window().scale_factor();
                let (x, y) = platform::bottom_centre(
                    (screen_width as i32, screen_height as i32),
                    (logical_w * scale) as i32,
                    (logical_h * scale) as i32,
                );
                ui.window().set_position(slint::PhysicalPosition::new(x, y));
            }
        });
    }

    ui.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_arguments_runs_with_defaults() {
        assert_eq!(
            parse_args_from(args(&[])),
            Ok(Action::Run {
                config: None,
                theme: None
            })
        );
    }

    #[test]
    fn config_takes_a_path() {
        assert_eq!(
            parse_args_from(args(&["--config", "/tmp/dock.conf"])),
            Ok(Action::Run {
                config: Some(PathBuf::from("/tmp/dock.conf")),
                theme: None
            })
        );
    }

    #[test]
    fn config_accepts_the_equals_spelling() {
        assert_eq!(
            parse_args_from(args(&["--config=/tmp/dock.conf"])),
            Ok(Action::Run {
                config: Some(PathBuf::from("/tmp/dock.conf")),
                theme: None
            })
        );
    }

    #[test]
    fn config_without_a_path_is_an_error() {
        assert_eq!(
            parse_args_from(args(&["--config"])),
            Err("--config needs a path".to_string())
        );
    }

    #[test]
    fn light_selects_the_light_theme() {
        assert_eq!(
            parse_args_from(args(&["--light"])),
            Ok(Action::Run {
                config: None,
                theme: Some(false)
            })
        );
    }

    #[test]
    fn dark_selects_the_dark_theme() {
        assert_eq!(
            parse_args_from(args(&["--dark"])),
            Ok(Action::Run {
                config: None,
                theme: Some(true)
            })
        );
    }

    #[test]
    fn the_last_theme_flag_wins() {
        assert_eq!(
            parse_args_from(args(&["--dark", "--light"])),
            Ok(Action::Run {
                config: None,
                theme: Some(false)
            })
        );
        assert_eq!(
            parse_args_from(args(&["--light", "--dark"])),
            Ok(Action::Run {
                config: None,
                theme: Some(true)
            })
        );
    }

    #[test]
    fn unknown_options_are_errors() {
        assert_eq!(
            parse_args_from(args(&["--frobnicate"])),
            Err("unknown option --frobnicate (try --help)".to_string())
        );
    }

    #[test]
    fn help_and_version_are_actions() {
        assert_eq!(parse_args_from(args(&["-h"])), Ok(Action::Help));
        assert_eq!(parse_args_from(args(&["--help"])), Ok(Action::Help));
        assert_eq!(parse_args_from(args(&["--version"])), Ok(Action::Version));
        assert_eq!(parse_args_from(args(&["-V"])), Ok(Action::Version));
    }

    #[test]
    fn print_config_is_an_action() {
        assert_eq!(
            parse_args_from(args(&["--print-config"])),
            Ok(Action::PrintConfig)
        );
    }

    #[test]
    fn config_and_theme_combine() {
        assert_eq!(
            parse_args_from(args(&["--config", "/a/b", "--light"])),
            Ok(Action::Run {
                config: Some(PathBuf::from("/a/b")),
                theme: Some(false)
            })
        );
    }

    #[test]
    fn an_empty_config_equals_path_is_an_error() {
        assert_eq!(
            parse_args_from(args(&["--config="])),
            Err("unknown option --config= (try --help)".to_string())
        );
    }

    #[test]
    fn the_help_text_lists_every_option() {
        for option in ["--config", "--light", "--dark", "--print-config", "--help"] {
            assert!(HELP.contains(option), "HELP must mention {option}");
        }
    }
}
