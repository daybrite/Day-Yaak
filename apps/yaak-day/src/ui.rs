//! The window's pages: the request list pane, the request editor with its response, and
//! Settings. `lib.rs` arranges them in the nav host; `model.rs` holds what they show.

use crate::engine::Sent;
use crate::model::{BODY_KINDS, HeaderRowFields, METHODS, RequestRow, Scene};
use crate::res;
use day::prelude::*;
use day_piece_texteditor::highlight::{Language, Palette, highlight_with_templates, highlighter};
use day_piece_texteditor::{TextEditorBuilder, text_editor_text};

/// The pushed editor's bar title: the open request's name, or the workspace's while none is.
pub(crate) fn detail_title(scene: Scene) -> String {
    match scene.selected_row() {
        Some(row) => row.name,
        None => scene.workspace_name(),
    }
}

// ---- the list ---------------------------------------------------------------------------------

/// The content-list pane: the workspace's requests, with New Request on its chrome.
pub(crate) fn request_list_pane() -> impl Piece {
    let scene = Scene::ambient();
    // Closing the editor drops the selection with it, so the row un-highlights.
    watch(
        move || scene.detail_open.get(),
        move |open, _| {
            if !open {
                scene.clear_selection();
            }
        },
    );
    request_list(scene).grow().toolbar(
        toolbar_button("tb-new", res::str::cmd_new_request())
            .icon(Symbol::Add)
            .tooltip(res::str::cmd_new_request())
            .placement(ToolbarPlacement::Primary)
            .action(move || scene.new_request()),
    )
}

fn request_list(scene: Scene) -> impl Piece {
    list(
        items(move || scene.requests.get(), |r: &RequestRow| r.id.clone()),
        move |slot: ItemSlot<RequestRow, String>| request_row(scene, slot),
    )
    .row_height(RowHeight::Uniform(52.0))
    .on_selection(move |rows: Vec<String>| match rows.first() {
        Some(id) => scene.open(id),
        None => scene.clear_selection(),
    })
    .selected_rows(move || scene.selected_index().into_iter().collect())
    .scroll_to_row(scene.scroll_to)
    .deletable(true)
    .delete_label(res::str::cmd_delete().format())
    .on_delete(move |index| {
        if let Some(row) = scene.requests.get_untracked().get(index) {
            scene.delete_request(&row.id);
        }
    })
    .id("request-list")
}

/// One row: the method in its color, the name, and the folder as a caption.
fn request_row(scene: Scene, slot: ItemSlot<RequestRow, String>) -> impl Piece {
    row((
        label(move || slot.get().method)
            .font(Font::Caption)
            .bold()
            .color(move || method_color(&slot.get().method))
            .width(56.0),
        column((
            label(move || slot.get().name),
            label(move || {
                let r = slot.get();
                if r.folder.is_empty() { r.url } else { r.folder }
            })
            .font(Font::Caption)
            .secondary(),
        ))
        .spacing(1.0)
        .align(HAlign::Leading)
        .grow(),
    ))
    .spacing(8.0)
    .align(VAlign::Center)
    .padding(Insets { top: 6.0, leading: 12.0, bottom: 6.0, trailing: 12.0 })
    .context_menu(vec![menu_item(res::str::cmd_delete().format()).action(move || {
        scene.delete_request(&slot.get().id);
    })])
}

/// Yaak's method colors, so a row reads at a glance.
pub(crate) fn method_color(method: &str) -> Color {
    match method {
        "GET" => Color::hex(0x3B82F6),
        "POST" => Color::hex(0x10B981),
        "PUT" => Color::hex(0xF59E0B),
        "PATCH" => Color::hex(0xA855F7),
        "DELETE" => Color::hex(0xEF4444),
        _ => Color::hex(0x6B7280),
    }
}

// ---- the editor and the response ----------------------------------------------------------------

/// The detail: the open request's editor and its response, or the empty state. A wide window
/// shows the two side by side; a compact one shows one at a time behind a segmented picker.
pub(crate) fn request_page() -> impl Piece {
    let scene = Scene::ambient();
    column((
        when(move || scene.selected.get().is_none(), move || empty_state(scene)),
        // `each` over a zero-or-one list, so the editor is rebuilt per request with its own
        // scope and bindings.
        each(
            items(
                move || scene.selected.get().into_iter().collect::<Vec<String>>(),
                |id: &String| id.clone(),
            ),
            move |_slot: ItemSlot<String, String>| editor(scene),
        ),
    ))
    .grow()
    .toolbar(
        toolbar_button("tb-send", res::str::cmd_send())
            .icon(Symbol::Play)
            .tooltip(res::str::cmd_send())
            .placement(ToolbarPlacement::Primary)
            .enabled_when(move || scene.selected.get().is_some() && !scene.sending.get())
            .action(move || scene.send()),
    )
}

fn empty_state(scene: Scene) -> impl Piece {
    column((
        spacer(),
        label(move || {
            if scene.requests.get().is_empty() {
                res::str::empty_workspace().format()
            } else {
                res::str::empty_selection().format()
            }
        })
        .font(Font::Title3)
        .secondary()
        .align(TextAlign::Center)
        .id("empty-state"),
        spacer(),
    ))
    .align(HAlign::Center)
    .grow()
    .padding(24.0)
    .grow()
}

fn editor(scene: Scene) -> impl Piece {
    column((
        url_bar(scene),
        // One pane at a time on a phone, both beside each other where there is room. The read
        // is tracked, so a window crossing a breakpoint re-arranges.
        each(
            items(
                move || vec![day::size_class().is_some_and(|c| c.width >= WidthClass::Expanded)],
                |wide: &bool| *wide,
            ),
            move |slot: ItemSlot<bool, bool>| {
                if slot.key() {
                    Either::Left(
                        row((
                            request_pane(scene).grow(),
                            divider().vertical(),
                            response_pane(scene).grow(),
                        ))
                        .spacing(8.0)
                        .grow(),
                    )
                } else {
                    Either::Right(
                        column((
                            picker(
                                [
                                    res::str::pane_request().format(),
                                    res::str::pane_response().format(),
                                ],
                                scene.pane,
                            )
                            .segmented()
                            .padding(Insets {
                                top: 0.0,
                                leading: 12.0,
                                bottom: 4.0,
                                trailing: 12.0,
                            })
                            .id("pane-picker"),
                            each(
                                items(move || vec![scene.pane.get()], |p: &usize| *p),
                                move |slot: ItemSlot<usize, usize>| {
                                    if slot.key() == 0 {
                                        Either::Left(request_pane(scene).grow())
                                    } else {
                                        Either::Right(response_pane(scene).grow())
                                    }
                                },
                            ),
                        ))
                        .grow(),
                    )
                }
            },
        ),
    ))
    .grow()
}

/// Method, URL and Send, on one line. Return in the URL field sends too.
fn url_bar(scene: Scene) -> impl Piece {
    row((
        picker(METHODS, scene.method).menu().id("method"),
        // A single-line styled editor rather than a text field, so a template tag reads as one
        // in the URL; Return sends.
        text_editor_text(scene.url)
            .single_line()
            .placeholder(res::str::url_hint())
            .on_submit(move || scene.send())
            .highlight(highlighter(Language::Plain, Palette::default(), true))
            .id("url")
            .grow(),
        button(move || {
            if scene.sending.get() {
                res::str::cmd_sending().format()
            } else {
                res::str::cmd_send().format()
            }
        })
        .prominent()
        .enabled(move || !scene.sending.get())
        .action(move || scene.send())
        .id("send"),
    ))
    .spacing(8.0)
    .align(VAlign::Center)
    .padding(Insets { top: 8.0, leading: 12.0, bottom: 8.0, trailing: 12.0 })
}

/// The request's own settings: its name, environment, headers and body.
fn request_pane(scene: Scene) -> impl Piece {
    scroll(
        column((
            labeled(
                res::str::field_name(),
                text_field(scene.name).placeholder(res::str::field_name_hint()).id("name"),
            ),
            when(
                move || !scene.environments.get().is_empty(),
                move || {
                    labeled(
                        res::str::field_environment(),
                        picker([res::str::environment_base().format()], scene.environment)
                            .options_reactive(move || {
                                let mut names = vec![res::str::environment_base().format()];
                                names.extend(
                                    scene.environments.get().iter().map(|e| e.name.clone()),
                                );
                                names
                            })
                            .menu()
                            .id("environment"),
                    )
                },
            ),
            heading_row(res::str::section_headers(), scene),
            headers_editor(scene),
            label(res::str::section_body()).font(Font::Headline),
            picker(BODY_KINDS.iter().map(|k| tr(k).format()), scene.body_kind)
                .segmented()
                .id("body-kind"),
            when(
                move || scene.body_kind.get() != 0,
                move || {
                    // The styled editor over the body string, highlighted as whatever the body
                    // kind says it is; the kind is read inside the highlighter, so switching it
                    // restyles the same text in place.
                    let palette = Palette::default();
                    text_editor_text(scene.body)
                        .code()
                        .placeholder(res::str::body_hint())
                        .min_lines(8)
                        .highlight(move |text| {
                            let language = match scene.body_kind.get() {
                                1 => Language::Json,
                                _ => Language::Plain,
                            };
                            highlight_with_templates(language, text, &palette)
                        })
                        .id("body")
                },
            ),
        ))
        .spacing(10.0)
        .align(HAlign::Leading)
        .padding(12.0),
    )
}

/// A section heading with its own Add button beside it.
fn heading_row<M>(title: impl IntoText<M>, scene: Scene) -> impl Piece {
    row((
        label(title).font(Font::Headline),
        spacer(),
        button(res::str::cmd_add_header())
            .compact()
            .action(move || scene.add_header())
            .id("add-header"),
    ))
    .align(VAlign::Center)
}

/// One row per header: enabled, name, value, remove. Each field binds through the store, so
/// typing patches one field and the save watches the store's version.
fn headers_editor(scene: Scene) -> impl Piece {
    let store = scene.headers;
    each(store.rows(move || store.keys()), move |slot: ModelSlot<crate::model::HeaderRow>| {
        row((
            toggle(slot.enabled()).id_keyed("header-on", slot.key()),
            text_field(slot.name())
                .placeholder(res::str::header_name_hint())
                .id_keyed("header-name", slot.key())
                .grow(),
            // The value binds through the store like the name, in a single-line styled editor
            // so a `${[ … ]}` tag shows as one.
            text_editor_text(slot.value())
                .single_line()
                .placeholder(res::str::header_value_hint())
                .highlight(highlighter(Language::Plain, Palette::default(), true))
                .id_keyed("header-value", slot.key())
                .grow(),
            button("×")
                .compact()
                .action(move || scene.remove_header(slot.key()))
                .id_keyed("header-remove", slot.key()),
        ))
        .spacing(6.0)
        .align(VAlign::Center)
    })
}

/// The last response: status line, headers, body.
fn response_pane(scene: Scene) -> impl Piece {
    column((
        when(
            move || scene.sending.get(),
            || label(res::str::response_sending()).secondary().padding(12.0),
        ),
        when(
            move || !scene.sending.get() && scene.response.get().is_none(),
            || label(res::str::response_none()).secondary().padding(12.0).id("response-none"),
        ),
        each(
            items(
                move || {
                    if scene.sending.get() {
                        Vec::new()
                    } else {
                        scene.response.get().into_iter().collect::<Vec<Sent>>()
                    }
                },
                // The key is the whole response: a new one rebuilds the pane.
                |s: &Sent| format!("{}:{}:{}", s.status, s.elapsed_ms, s.body_len),
            ),
            |slot: ItemSlot<Sent, String>| response_view(slot.get()),
        ),
    ))
    .align(HAlign::Leading)
}

fn response_view(sent: Sent) -> impl Piece {
    let status_color = match sent.status {
        200..=299 => Color::hex(0x10B981),
        300..=399 => Color::hex(0xF59E0B),
        400..=599 => Color::hex(0xEF4444),
        _ => Color::hex(0x6B7280),
    };
    let status_line = if sent.ok() {
        format!("{} {}", sent.status, sent.status_reason)
    } else {
        res::str::response_failed().format()
    };
    let meta = format!("{} ms · {}", sent.elapsed_ms, size_text(sent.body_len));
    let error = sent.error.clone().unwrap_or_default();
    let error_shown = error.clone();
    let headers = sent.headers.clone();
    let body = if sent.body_truncated {
        format!("{}\n\n{}", sent.body, res::str::body_truncated().format())
    } else if sent.body.is_empty() && sent.body_len > 0 {
        res::str::body_binary().format()
    } else {
        sent.body
    };
    scroll(
        column((
            row((
                label(status_line).font(Font::Headline).color(status_color).id("status"),
                label(meta).font(Font::Caption).secondary().id("response-meta"),
            ))
            .spacing(10.0)
            .align(VAlign::Center),
            when(
                move || !error.is_empty(),
                move || {
                    label(error_shown.clone()).color(Color::hex(0xEF4444)).selectable().id("error")
                },
            ),
            label(res::str::section_response_headers()).font(Font::Headline),
            each(
                items(move || headers.clone(), |h: &(String, String)| h.clone()),
                |slot: ItemSlot<(String, String), (String, String)>| {
                    let (name, value) = slot.key();
                    label(format!("{name}: {value}")).font(Font::Footnote).monospace().selectable()
                },
            ),
            label(res::str::section_response_body()).font(Font::Headline),
            // Read-only, highlighted by the content type: the same editor the request body uses,
            // so a JSON response reads the way the request that produced it does.
            text_editor_text(Signal::new(body))
                .editable(false)
                .spellcheck(false)
                .min_lines(3)
                .highlight(highlighter(
                    sent.content_type.as_deref().map(Language::for_mime).unwrap_or(Language::Plain),
                    Palette::default(),
                    false,
                ))
                .id("response-body"),
        ))
        .spacing(8.0)
        .align(HAlign::Leading)
        .padding(12.0),
    )
}

fn size_text(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

// ---- settings ---------------------------------------------------------------------------------

/// Appearance and language, from `day-piece-settings`, and where the data lives.
pub(crate) fn settings_body() -> impl Piece {
    let engine = crate::engine::engine();
    let store = if engine.shared_with_desktop {
        res::str::store_shared().format()
    } else {
        res::str::store_own().format()
    };
    column((
        form((day_piece_settings::settings_sections(
            crate::THEME_KEY,
            crate::LOCALE_KEY,
            res::locales::ALL,
        ),)),
        label(store).font(Font::Footnote).secondary().padding(12.0).id("store-note"),
        label(engine.data_dir.display().to_string())
            .font(Font::Caption)
            .secondary()
            .selectable()
            .padding(Insets { top: 0.0, leading: 12.0, bottom: 12.0, trailing: 12.0 }),
    ))
    .align(HAlign::Leading)
}

/// The same body as a nav section, for the platforms with no menu bar.
pub(crate) fn settings_page() -> impl Piece {
    column((
        label(res::str::nav_settings()).font(Font::Title).id("settings-title"),
        settings_body(),
    ))
    .spacing(12.0)
    .align(HAlign::Leading)
    .padding(16.0)
}
