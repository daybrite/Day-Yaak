//! Yaak for Day: Yaak's API client with its interface built from Day's native pieces, over the
//! same Rust engine the desktop app and the CLI use (`../../crates`). `root()` runs once and
//! opens the first window; `window_shell` builds one window's UI.

use day::prelude::*;
use day_piece_settings::prelude::*;

mod engine;
mod model;
mod ui;
use crate::model::Scene;

// Entry point for the mobile hosts; a desktop build enters through src/main.rs.
day::day_start!(options: window(), root);

/// Options for every window. The catalog and title go to `launch`, which installs them
/// (https://daybrite.dev/docs/localization).
pub fn window() -> day::WindowOptions {
    // The generated options carry the catalog, the title and the version (named in Day's exit
    // line, https://daybrite.dev/docs/lifecycle); the size is desktop only, phones fill the screen.
    res::window_options().size(1100.0, 720.0)
}

// Typed names for everything under `resource/` (https://daybrite.dev/docs/resources).
day::resources!();

/// The nav key of the Settings row, where there is no menu bar to hold Settings.
pub(crate) const SETTINGS: &str = "settings";

/// One-time app setup, then the first window's content.
pub fn root() -> impl Piece {
    info!("Yaak for Day starting");
    // Open the store before the first window, so a failure shows up at launch.
    let _ = engine::engine();
    // Settings and About (https://daybrite.dev/docs/settings): appearance and language from
    // Day's catalog, the Data group (ui.rs), the app's name and version from Day.toml. A
    // desktop gets the App menu's Settings… and About items and their windows; a phone the
    // Settings row of the navigation in `window_shell`.
    let prefs = Preferences::new()
        .about(About::from_manifest().link(res::str::day_about_website(), "https://yaak.app"))
        .group(general_group(res::locales::ALL))
        .group(ui::data_group())
        .install();

    day::register_new_window({
        let prefs = prefs.clone();
        move || window_shell(prefs.clone())
    });
    app_menu(menus());

    window_shell(prefs)
}

/// One window's UI, for the first window and every File ▸ New Window. Each window has its own
/// scene, so two windows can show two workspaces.
fn window_shell(prefs: Prefs) -> impl Piece {
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
            .content_list(
                ContentList::new(ui::request_list_pane)
                    .width(300.0)
                    .only_for(|k: &Option<String>| k.as_deref() != Some(SETTINGS))
                    .detail_visible(scene.detail_open)
                    .detail_title(move || ui::detail_title(scene)),
            )
            .items((
                nav_items(
                    move || scene.workspaces.get(),
                    |w: &yaak_models::models::Workspace| item(w.id.clone(), w.name.clone()),
                ),
                // Use the desktop's App menu Settings… item; without a menu bar, keep Settings
                // reachable as a navigation row (https://daybrite.dev/docs/settings).
                prefs.nav_item(Some(SETTINGS.to_string())),
            ))
            .destination(|_: &Option<String>| ui::request_page())
            .id(day::id!(nav))
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
            res::str::menu_file(),
            vec![
                menu_role(MenuRole::NewWindow),
                menu_item(res::str::cmd_new_request())
                    .shortcut(Shortcut::primary("n").shift())
                    .action(front(|scene| scene.new_request())),
                menu_separator(),
                menu_role(MenuRole::CloseWindow),
            ],
        ),
        sub_menu(
            res::str::menu_edit(),
            vec![
                menu_role(MenuRole::Cut),
                menu_role(MenuRole::Copy),
                menu_role(MenuRole::Paste),
                menu_role(MenuRole::SelectAll),
                menu_separator(),
                menu_item(res::str::cmd_delete())
                    .shortcut(Shortcut::primary("Delete"))
                    .action(front(|scene| scene.delete_selected())),
            ],
        ),
        sub_menu(
            res::str::menu_request(),
            vec![
                menu_item(res::str::cmd_send())
                    .shortcut(Shortcut::primary("Enter"))
                    .action(front(|scene| scene.send())),
                menu_item(res::str::cmd_cancel())
                    .shortcut(Shortcut::primary("."))
                    .action(front(|scene| scene.cancel())),
            ],
        ),
    ]
}
