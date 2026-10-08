//! Yaak for Day: Yaak's API client with its interface built from Day's native pieces, over the
//! same Rust engine the desktop app and the CLI use (`../../crates`). `root()` runs once and
//! opens the first window; `window_shell` builds one window's UI.

use day::prelude::*;

mod engine;
mod model;
mod ui;
use crate::model::Scene;

// Entry point for the mobile hosts; a desktop build enters through src/main.rs.
day::day_start!(options: window(), root);

/// Options for every window. The catalog and title go to `launch`, which installs them
/// (https://daybrite.dev/docs/localization).
pub fn window() -> day::WindowOptions {
    day::WindowOptions {
        locales: Some((res::locales::DEFAULT, res::locales::CATALOG)),
        title_fn: Some(|| res::str::app_title().format()),
        // Desktop only; phones fill the screen.
        size: day::prelude::Size::new(1100.0, 720.0),
        // Named in Day's exit line (https://daybrite.dev/docs/lifecycle).
        version: Some(env!("CARGO_PKG_VERSION").into()),
        ..Default::default()
    }
}

// Typed names for everything under `resource/` (https://daybrite.dev/docs/resources).
day::resources!();

/// The `day::prefs` keys the Settings page writes and startup reads.
const THEME_KEY: &str = "app.theme";
const LOCALE_KEY: &str = "app.locale";
/// The nav key of the Settings row, where there is no menu bar to hold Settings.
pub(crate) const SETTINGS: &str = "settings";

/// True where there is a menu bar, so Settings lives in the App menu instead of the nav.
pub(crate) fn has_menu_bar() -> bool {
    capability(Cap::AppMenu) != Support::Unsupported
}

/// One-time app setup, then the first window's content.
pub fn root() -> impl Piece {
    info!("Yaak for Day starting");
    day_piece_settings::apply_startup(THEME_KEY, LOCALE_KEY);
    // Open the store before the first window, so a failure shows up at launch.
    let _ = engine::engine();

    day::register_preferences(ui::settings_body);
    day::register_new_window(window_shell);
    app_menu(menus());

    window_shell()
}

/// One window's UI, for the first window and every File ▸ New Window. Each window has its own
/// scene, so two windows can show two workspaces.
fn window_shell() -> impl Piece {
    Scene::scoped(move |scene| {
        scene.install();
        day::window_title(move || match scene.selected_row() {
            Some(row) => format!("{} – {}", row.name, scene.workspace_name()),
            None => {
                let name = scene.workspace_name();
                if name.is_empty() { res::str::app_title().format() } else { name }
            }
        });
        // Workspaces in the sidebar, the workspace's requests as the content list, the open
        // request as the detail: a sidebar on a desktop, three pushed layers on a phone
        // (https://daybrite.dev/docs/navigation).
        nav(scene.section)
            .style(NavStyle::Sidebar)
            .title(res::str::app_title())
            .content_list(ui::request_list_pane)
            .content_list_width(300.0)
            .content_list_for(|k: &Option<String>| k.as_deref() != Some(SETTINGS))
            .detail_visible(scene.detail_open)
            .detail_title(move || ui::detail_title(scene))
            .items(
                move || scene.workspaces.get(),
                |w: &yaak_models::models::Workspace| item(w.id.clone(), w.name.clone()),
            )
            .items(
                move || if has_menu_bar() { Vec::new() } else { vec![SETTINGS.to_string()] },
                |k: &String| item(k.clone(), res::str::nav_settings()),
            )
            .destination(|k: &Option<String>| {
                if k.as_deref() == Some(SETTINGS) {
                    Either::Left(ui::settings_page())
                } else {
                    Either::Right(ui::request_page())
                }
            })
            .id("nav")
    })
}

/// Run a command against the focused window. Menu bar items belong to no window, so they
/// look the front one up when they run.
fn front(f: impl Fn(Scene) + 'static) -> impl Fn() + 'static {
    move || {
        if let Some(scene) = Scene::focused() {
            f(scene)
        }
    }
}

/// The desktop menu bar; the mobile toolkits ignore it.
fn menus() -> Vec<MenuEntry> {
    vec![
        sub_menu(
            res::str::menu_file().format(),
            vec![
                menu_role(MenuRole::NewWindow),
                menu_item(res::str::cmd_new_request().format())
                    .shortcut(Shortcut::new("n").shift())
                    .action(front(|scene| scene.new_request())),
                menu_separator(),
                menu_role(MenuRole::CloseWindow),
            ],
        ),
        sub_menu(
            res::str::menu_edit().format(),
            vec![
                menu_role(MenuRole::Cut),
                menu_role(MenuRole::Copy),
                menu_role(MenuRole::Paste),
                menu_role(MenuRole::SelectAll),
                menu_separator(),
                menu_item(res::str::cmd_delete().format())
                    .shortcut(Shortcut::new("Delete"))
                    .action(front(|scene| scene.delete_selected())),
            ],
        ),
        sub_menu(
            res::str::menu_request().format(),
            vec![
                menu_item(res::str::cmd_send().format())
                    .shortcut(Shortcut::new("Enter"))
                    .action(front(|scene| scene.send())),
            ],
        ),
    ]
}
