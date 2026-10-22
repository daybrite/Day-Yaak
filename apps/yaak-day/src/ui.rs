//! The window's pages: the request list pane, the request editor with its response, and
//! Settings. `lib.rs` arranges them in the nav host; `model.rs` holds what they show.

use crate::engine::{ResponseSummary, Sent, TimelineRow, size_text};
use crate::model::{
    AUTH_KINDS, AUTH_LOCATIONS, BODY_KINDS, HTTP_VERSIONS, METHODS, PairRow, PairRowFields,
    RequestRow, Rows, Scene, TABS, TRI_STATE, auth, body,
};
use crate::res;
use day::prelude::*;
use day_piece_settings::prelude::*;
use day_piece_texteditor::highlight::{Language, Palette, highlight_with_templates, highlighter};
use day_piece_texteditor::text_editor_text;

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
        toolbar_button(day::id!(tb_new), res::str::cmd_new_request())
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
    .deletable(
        Delete::new(move |index| {
            if let Some(row) = scene.requests.get_untracked().get(index) {
                scene.delete_request(&row.id);
            }
        })
        .label(res::str::cmd_delete()),
    )
    .id(day::id!(request_list))
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
    .spacing(Space::S)
    .align(VAlign::Center)
    .padding(Insets::symmetric(12.0, 6.0))
    .context_menu([menu_item(res::str::cmd_delete()).action(move || {
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

/// A picker's options over one of the model's tables, by position: the model keeps each choice
/// as an index into its table (`body::JSON`, `auth::BEARER`, …), and each entry is the generated
/// accessor for that choice's title.
fn indexed(titles: &'static [fn() -> LocalizedText]) -> Vec<(usize, LocalizedText)> {
    titles.iter().enumerate().map(|(i, title)| (i, title())).collect()
}

/// As [`indexed`], for a table whose entries are their own titles (the HTTP methods).
fn indexed_titles(titles: &'static [&'static str]) -> Vec<(usize, &'static str)> {
    titles.iter().copied().enumerate().collect()
}

// ---- the editor and the response ----------------------------------------------------------------

/// The detail: the open request's editor and its response, or the empty state. A wide window
/// shows the two side by side; a compact one shows one at a time behind a segmented picker.
pub(crate) fn request_page() -> impl Piece {
    let scene = Scene::ambient();
    column((
        when(move || scene.selected.get().is_none(), move || empty_state(scene)),
        // Keyed by the open request, so the editor is rebuilt per request with its own scope
        // and bindings.
        keyed(move || scene.selected.get(), move |_| editor(scene)),
    ))
    .grow()
    // Derived, not fixed: the layout button's glyph shows the arrangement a press gives, so
    // the items are rebuilt whenever `stacked` flips.
    .toolbar(move || {
        let stacked = scene.stacked.get();
        vec![
            // Stack the response under the request, or put it back beside it. Only the wide
            // editor has the split; a phone shows one pane at a time behind its picker.
            toolbar_button(day::id!(tb_layout), res::str::cmd_layout())
                .image(if stacked {
                    res::vectors::layout_beside
                } else {
                    res::vectors::layout_stack
                })
                .tooltip(if stacked {
                    res::str::cmd_layout_beside()
                } else {
                    res::str::cmd_layout_stack()
                })
                .enabled(move || scene.selected.get().is_some())
                .action(move || scene.stacked.update(|s| *s = !*s)),
            toolbar_button(day::id!(tb_send), res::str::cmd_send())
                .image(res::vectors::send)
                .tooltip(res::str::cmd_send())
                .placement(ToolbarPlacement::Primary)
                .enabled(move || scene.selected.get().is_some() && !scene.sending.get())
                .action(move || scene.send()),
        ]
    })
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
        .id(day::id!(empty_state)),
        spacer(),
    ))
    .align(HAlign::Center)
    .grow()
    .padding(Space::XL)
    .grow()
}

fn editor(scene: Scene) -> impl Piece {
    column((
        url_bar(scene),
        // One pane at a time on a phone, both beside each other where there is room. The read
        // is tracked, so a window crossing a breakpoint re-arranges.
        when(
            || day::size_class().is_some_and(|c| c.width >= WidthClass::Expanded),
            move || {
                // The toolkit's own splitter where there is one: the request form and the
                // response, side by side or stacked (the toolbar's layout button), the
                // divider's position kept per window.
                split(request_pane(scene), response_pane(scene))
                    .axis(move || {
                        if scene.stacked.get() {
                            SplitAxis::Vertical
                        } else {
                            SplitAxis::Horizontal
                        }
                    })
                    .fraction(scene.split)
                    .id(day::id!(editor_split))
                    .grow()
            },
        )
        .otherwise(move || {
            column((
                picker(
                    scene.pane,
                    [
                        (0, res::str::pane_request()),
                        (1, res::str::pane_response()),
                    ],
                )
                .segmented()
                .id(day::id!(pane_picker))
                .padding(Insets {
                    top: 0.0,
                    leading: 12.0,
                    bottom: 4.0,
                    trailing: 12.0,
                }),
                switch(
                    move || scene.pane.get(),
                    move |pane| match pane {
                        0 => request_pane(scene).grow().any(),
                        _ => response_pane(scene).grow().any(),
                    },
                ),
            ))
            .grow()
        }),
    ))
    .grow()
}

/// Method, URL and Send, on one line. Return in the URL field sends too, and while a send is
/// in flight the button cancels it.
fn url_bar(scene: Scene) -> impl Piece {
    row((
        picker(scene.method, indexed_titles(&METHODS)).menu().id(day::id!(method)),
        // A single-line styled editor rather than a text field, so a template tag reads as one
        // in the URL; Return sends.
        text_editor_text(scene.url)
            .single_line()
            .placeholder(res::str::url_hint())
            .on_submit(move || scene.send())
            .highlight(highlighter(Language::Plain, Palette::default(), true))
            .id(day::id!(url))
            .grow(),
        button(move || {
            if scene.sending.get() {
                res::str::cmd_cancel().format()
            } else {
                res::str::cmd_send().format()
            }
        })
        .prominent()
        .action(move || if scene.sending.get_untracked() { scene.cancel() } else { scene.send() })
        .id(day::id!(send)),
    ))
    .spacing(Space::S)
    .align(VAlign::Center)
    .padding(Insets::symmetric(12.0, 8.0))
}

/// The request's own settings: its name and environment, then one tab at a time of its body,
/// URL parameters, headers, authentication and per-request settings.
fn request_pane(scene: Scene) -> impl Piece {
    scroll(
        column((
            labeled(
                res::str::field_name(),
                text_field(scene.name).placeholder(res::str::field_name_hint()).id(day::id!(name)),
            ),
            when(
                move || !scene.environments.get().is_empty(),
                move || {
                    labeled(
                        res::str::field_environment(),
                        picker(scene.environment, move || {
                            let mut names = vec![res::str::environment_base().format()];
                            names.extend(scene.environments.get().iter().map(|e| e.name.clone()));
                            names.into_iter().enumerate().collect::<Vec<_>>()
                        })
                        .menu()
                        .id(day::id!(environment)),
                    )
                },
            ),
            // Five tabs are wider than a request pane dragged narrow, so the strip scrolls
            // sideways there instead of losing its last tab.
            row((picker(scene.tab, indexed(&TABS)).segmented().id(day::id!(request_tab)),))
                .fit(RowFit::Scroll),
            switch(
                move || scene.tab.get(),
                move |tab| match tab {
                    0 => body_tab(scene).any(),
                    1 => params_tab(scene).any(),
                    2 => pairs_editor(scene, scene.headers, &HEADER_IDS).any(),
                    3 => auth_tab(scene).any(),
                    _ => settings_tab(scene).any(),
                },
            ),
        ))
        .spacing(10.0)
        .align(HAlign::Leading)
        .padding(Space::M),
    )
}

/// The element ids of one name-and-value editor, so a walkthrough can tell the headers' rows
/// from the parameters' and the form's.
struct PairIds {
    on: fn(String) -> day::ElementId,
    name: fn(String) -> day::ElementId,
    value: fn(String) -> day::ElementId,
    file: fn(String) -> day::ElementId,
    remove: fn(String) -> day::ElementId,
    add: fn() -> day::ElementId,
    /// Which editor this is, so the multipart form's rows can be told from the others.
    multipart: bool,
}

const HEADER_IDS: PairIds = PairIds {
    on: |k| day::id!(header_on, k),
    name: |k| day::id!(header_name, k),
    value: |k| day::id!(header_value, k),
    file: |k| day::id!(header_file, k),
    remove: |k| day::id!(header_remove, k),
    add: || day::id!(add_header),
    multipart: false,
};
const PARAM_IDS: PairIds = PairIds {
    on: |k| day::id!(param_on, k),
    name: |k| day::id!(param_name, k),
    value: |k| day::id!(param_value, k),
    file: |k| day::id!(param_file, k),
    remove: |k| day::id!(param_remove, k),
    add: || day::id!(add_param),
    multipart: false,
};
const FORM_IDS: PairIds = PairIds {
    on: |k| day::id!(form_on, k),
    name: |k| day::id!(form_name, k),
    value: |k| day::id!(form_value, k),
    file: |k| day::id!(form_file, k),
    remove: |k| day::id!(form_remove, k),
    add: || day::id!(add_field),
    multipart: true,
};

/// One row per pair: enabled, name, value, remove, and Add under the last. Each field binds
/// through the store, so typing patches one field and the save watches the store's version.
/// A multipart form's rows also take a file in place of a value.
fn pairs_editor(scene: Scene, rows: Rows, ids: &'static PairIds) -> impl Piece {
    column((
        each(rows.rows(move || rows.keys()), move |slot: ModelSlot<PairRow>| {
            let key = slot.key();
            let has_file = move || slot.file().with(|f| f.is_some_and(|f| !f.is_empty()));
            let takes_file = move || ids.multipart && scene.body_kind.get() == body::MULTIPART;
            // Two halves, side by side where there is room and one over the other on a
            // phone: the switch with the name, then the value with the row's buttons.
            let name = row((
                toggle(slot.enabled()).id((ids.on)(key.to_string())),
                text_field(slot.name())
                    .placeholder(res::str::pair_name_hint())
                    .id((ids.name)(key.to_string()))
                    .grow(),
            ))
            .spacing(6.0)
            .align(VAlign::Center);
            let value = row((
                // The value binds through the store like the name, in a single-line styled
                // editor so a `${[ … ]}` tag shows as one. A row uploading a file names it here.
                when(
                    move || !(takes_file() && has_file()),
                    move || {
                        text_editor_text(slot.value())
                            .single_line()
                            .placeholder(res::str::pair_value_hint())
                            .highlight(highlighter(Language::Plain, Palette::default(), true))
                            .id((ids.value)(key.to_string()))
                    },
                )
                .otherwise(move || {
                    label(move || slot.file().with(|f| file_name(f.map_or("", |f| f.as_str()))))
                        .secondary()
                        .id((ids.value)(key.to_string()))
                })
                .grow(),
                when(takes_file, move || {
                    // An icon each way, so the row keeps its width for the name and the value;
                    // the title is what assistive technology and the tooltip read.
                    button(move || {
                        if has_file() {
                            res::str::cmd_remove_file().format()
                        } else {
                            res::str::cmd_choose_file().format()
                        }
                    })
                    .icon(move || if has_file() { Symbol::Close } else { Symbol::Document })
                    .icon_only()
                    .compact()
                    .action(move || {
                        if has_file() {
                            slot.file().write_commit(String::new());
                        } else {
                            scene.pick_form_file(key);
                        }
                    })
                    .id((ids.file)(key.to_string()))
                }),
                button(res::str::cmd_remove_row())
                    .icon(Symbol::Delete)
                    .icon_only()
                    .compact()
                    .action(move || scene.remove_row(rows, key))
                    .id((ids.remove)(key.to_string())),
            ))
            .spacing(6.0)
            .align(VAlign::Center);
            adaptive((name.grow(), value.grow())).spacing(6.0)
        }),
        button(res::str::cmd_add()).compact().action(move || scene.add_row(rows)).id((ids.add)()),
    ))
    .spacing(Space::S)
    .align(HAlign::Leading)
}

/// A path's last component, as a row names the file it uploads.
fn file_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// The body: its kind, then that kind's editor.
fn body_tab(scene: Scene) -> impl Piece {
    let kind = move || scene.body_kind.get();
    column((
        picker(scene.body_kind, indexed(&BODY_KINDS)).menu().id(day::id!(body_kind)),
        when(
            move || matches!(kind(), body::JSON | body::TEXT | body::XML),
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
                            body::JSON => Language::Json,
                            body::XML => Language::Xml,
                            _ => Language::Plain,
                        };
                        highlight_with_templates(language, text, &palette)
                    })
                    .id(day::id!(body))
            },
        ),
        when(
            move || matches!(kind(), body::FORM | body::MULTIPART),
            move || pairs_editor(scene, scene.form, &FORM_IDS),
        ),
        when(
            move || kind() == body::GRAPHQL,
            move || {
                let (palette, variables_palette) = (Palette::default(), Palette::default());
                column((
                    label(res::str::graphql_query()).font(Font::Subheadline).secondary(),
                    text_editor_text(scene.graphql_query)
                        .code()
                        .placeholder(res::str::graphql_query_hint())
                        .min_lines(6)
                        .highlight(move |text| {
                            highlight_with_templates(Language::GraphQl, text, &palette)
                        })
                        .id(day::id!(graphql_query)),
                    label(res::str::graphql_variables()).font(Font::Subheadline).secondary(),
                    text_editor_text(scene.graphql_variables)
                        .code()
                        .placeholder(res::str::graphql_variables_hint())
                        .min_lines(3)
                        .highlight(move |text| {
                            highlight_with_templates(Language::Json, text, &variables_palette)
                        })
                        .id(day::id!(graphql_variables)),
                ))
                .spacing(6.0)
                .align(HAlign::Leading)
            },
        ),
        when(
            move || kind() == body::BINARY,
            move || {
                row((
                    text_field(scene.binary_path)
                        .placeholder(res::str::binary_hint())
                        .id(day::id!(binary_path))
                        .grow(),
                    button(res::str::cmd_choose_file())
                        .action(move || scene.pick_binary())
                        .id(day::id!(binary_choose)),
                ))
                .spacing(Space::S)
                .align(VAlign::Center)
            },
        ),
    ))
    .spacing(10.0)
    .align(HAlign::Leading)
}

/// The URL's parameters: query pairs, and values for the path's `:name` placeholders.
fn params_tab(scene: Scene) -> impl Piece {
    column((
        pairs_editor(scene, scene.params, &PARAM_IDS),
        label(res::str::params_hint()).font(Font::Footnote).secondary(),
    ))
    .spacing(10.0)
    .align(HAlign::Leading)
}

/// The picker's choices, by position: the fixed ones, and a stored plugin type under its own
/// name at [`auth::FOREIGN`].
fn auth_choices(scene: Scene) -> Vec<(usize, String)> {
    let mut choices: Vec<String> = AUTH_KINDS.iter().map(|title| title().format()).collect();
    let foreign = scene.auth_foreign.get();
    if !foreign.is_empty() {
        choices.push(foreign);
    }
    choices.into_iter().enumerate().collect()
}

/// Authentication: inherited, none, or one of the types this client applies itself.
fn auth_tab(scene: Scene) -> impl Piece {
    let kind = move || scene.auth_kind.get();
    // Secrets stay hidden until asked for; a template tag in one is worth reading.
    let shown = Signal::new(false);
    let secret = move |text: Signal<String>, id: day::ElementId| {
        secure_field(text).secure(move || !shown.get()).id(id).grow()
    };
    column((
        // Derived, so the stored plugin type's choice exists from the first frame and follows
        // the request.
        picker(scene.auth_kind, move || auth_choices(scene)).menu().id(day::id!(auth_kind)),
        when(
            move || kind() == auth::INHERIT,
            || label(res::str::auth_inherit_hint()).font(Font::Footnote).secondary(),
        ),
        when(
            move || kind() == auth::FOREIGN,
            move || {
                label(move || res::str::auth_foreign_hint(scene.auth_foreign.get()).format())
                    .font(Font::Footnote)
                    .secondary()
                    .id(day::id!(auth_foreign))
            },
        ),
        when(
            move || kind() == auth::BASIC,
            move || {
                form((
                    labeled(
                        res::str::auth_username(),
                        text_field(scene.auth_username).id(day::id!(auth_username)).grow(),
                    ),
                    labeled(
                        res::str::auth_password(),
                        secret(scene.auth_password, day::id!(auth_password)),
                    ),
                ))
            },
        ),
        when(
            move || kind() == auth::BEARER,
            move || {
                form((
                    labeled(res::str::auth_token(), secret(scene.auth_token, day::id!(auth_token))),
                    labeled(
                        res::str::auth_prefix(),
                        text_field(scene.auth_prefix).id(day::id!(auth_prefix)).grow(),
                    ),
                ))
            },
        ),
        when(
            move || kind() == auth::API_KEY,
            move || {
                form((
                    labeled(
                        res::str::auth_location(),
                        picker(scene.auth_location, indexed(&AUTH_LOCATIONS))
                            .menu()
                            .id(day::id!(auth_location)),
                    ),
                    labeled(
                        move || {
                            if scene.auth_location.get() == 1 {
                                res::str::auth_key_query().format()
                            } else {
                                res::str::auth_key_header().format()
                            }
                        },
                        text_field(scene.auth_key).id(day::id!(auth_key)).grow(),
                    ),
                    labeled(res::str::auth_value(), secret(scene.auth_value, day::id!(auth_value))),
                ))
            },
        ),
        when(
            move || matches!(kind(), auth::BASIC | auth::BEARER | auth::API_KEY),
            move || {
                form((
                    labeled(res::str::auth_show(), toggle(shown).id(day::id!(auth_show))),
                    labeled(
                        res::str::auth_enabled(),
                        toggle(scene.auth_enabled).id(day::id!(auth_on)),
                    ),
                ))
            },
        ),
    ))
    .spacing(10.0)
    .align(HAlign::Leading)
}

/// One setting the request may leave to its folder and workspace, or turn on or off itself.
fn inherited_choice<M>(
    title: impl IntoText<M>,
    value: Signal<usize>,
    id: day::ElementId,
) -> impl Piece {
    labeled(title, picker(value, indexed(&TRI_STATE)).menu().id(id))
}

/// What this request overrides of its folder's and workspace's send settings.
fn settings_tab(scene: Scene) -> impl Piece {
    column((
        form((
            inherited_choice(
                res::str::setting_follow_redirects(),
                scene.follow_redirects,
                day::id!(set_redirects),
            ),
            inherited_choice(
                res::str::setting_validate_certificates(),
                scene.validate_certificates,
                day::id!(set_certificates),
            ),
            inherited_choice(
                res::str::setting_send_cookies(),
                scene.send_cookies,
                day::id!(set_send_cookies),
            ),
            inherited_choice(
                res::str::setting_store_cookies(),
                scene.store_cookies,
                day::id!(set_store_cookies),
            ),
            labeled(
                res::str::setting_http_version(),
                picker(scene.http_version, indexed(&HTTP_VERSIONS))
                    .menu()
                    .id(day::id!(set_http_version)),
            ),
            labeled(
                res::str::setting_timeout(),
                text_field(scene.timeout)
                    .placeholder(res::str::setting_timeout_hint())
                    .input_purpose(InputPurpose::Number)
                    .id(day::id!(set_timeout))
                    .grow(),
            ),
        )),
        label(res::str::settings_hint()).font(Font::Footnote).secondary(),
    ))
    .spacing(10.0)
    .align(HAlign::Leading)
}

/// The response on show: the send in flight, or a stored response with the request's history.
fn response_pane(scene: Scene) -> impl Piece {
    column((
        when(
            move || scene.sending.get(),
            move || {
                row((
                    label(res::str::response_sending()).secondary(),
                    button(res::str::cmd_cancel())
                        .compact()
                        .action(move || scene.cancel())
                        .id(day::id!(cancel)),
                ))
                .spacing(10.0)
                .align(VAlign::Center)
                .padding(Space::M)
            },
        ),
        when(
            move || !scene.sending.get() && scene.response.get().is_none(),
            || {
                label(res::str::response_none())
                    .secondary()
                    .padding(Space::M)
                    .id(day::id!(response_none))
            },
        ),
        when(
            move || !scene.sending.get() && !scene.history.get().is_empty(),
            move || history_row(scene),
        ),
        // The key is the whole response: a new one rebuilds the pane.
        keyed(
            move || (!scene.sending.get()).then(|| scene.response.get()).flatten(),
            move |sent: &Sent| response_view(scene, sent.clone()),
        ),
    ))
    .align(HAlign::Leading)
}

/// One history entry as the picker names it: when it was sent, how it ended, how long it took.
fn history_label(entry: &ResponseSummary) -> String {
    if entry.failed {
        format!("{} · {}", entry.sent_at, res::str::response_failed().format())
    } else {
        format!(
            "{} · {} {} · {} ms",
            entry.sent_at, entry.status, entry.status_reason, entry.elapsed_ms
        )
    }
}

/// The request's stored responses, newest first, and the way to forget them.
fn history_row(scene: Scene) -> impl Piece {
    let labels =
        move || scene.history.get().iter().map(history_label).enumerate().collect::<Vec<_>>();
    row((
        picker(scene.history_index, labels).menu().id(day::id!(history)).grow(),
        button(res::str::cmd_clear_history())
            .compact()
            .action(move || scene.clear_history())
            .id(day::id!(clear_history)),
    ))
    .spacing(Space::S)
    .align(VAlign::Center)
    .padding(Insets { top: 12.0, leading: 12.0, bottom: 0.0, trailing: 12.0 })
}

fn response_view(scene: Scene, sent: Sent) -> impl Piece {
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
    let (for_body, for_headers, for_timeline) = (sent.clone(), sent.clone(), sent.timeline.clone());
    scroll(
        column((
            row((
                label(status_line).font(Font::Headline).color(status_color).id(day::id!(status)),
                label(meta).font(Font::Caption).secondary().id(day::id!(response_meta)),
            ))
            .spacing(10.0)
            .align(VAlign::Center),
            when(
                move || !error.is_empty(),
                move || {
                    label(error_shown.clone())
                        .color(Color::hex(0xEF4444))
                        .selectable()
                        .id(day::id!(error))
                },
            ),
            picker(
                scene.response_tab,
                [
                    (0, res::str::response_tab_body()),
                    (1, res::str::response_tab_headers()),
                    (2, res::str::response_tab_timeline()),
                ],
            )
            .segmented()
            .id(day::id!(response_tab)),
            switch(
                move || scene.response_tab.get(),
                move |tab| match tab {
                    0 => response_body(for_body.clone()).any(),
                    1 => response_headers(for_headers.clone()).any(),
                    _ => response_timeline(for_timeline.clone()).any(),
                },
            ),
        ))
        .spacing(Space::S)
        .align(HAlign::Leading)
        .padding(Space::M),
    )
}

/// Read-only, highlighted by the content type: the same editor the request body uses, so a
/// JSON response reads the way the request that produced it does.
fn response_body(sent: Sent) -> impl Piece {
    let text = if sent.body_truncated {
        format!("{}\n\n{}", sent.body, res::str::body_truncated().format())
    } else if sent.body.is_empty() && sent.body_len > 0 {
        res::str::body_binary().format()
    } else {
        sent.body
    };
    text_editor_text(Signal::new(text))
        .editable(false)
        .no_spellcheck()
        .min_lines(3)
        .highlight(highlighter(
            sent.content_type.as_deref().map(Language::for_mime).unwrap_or(Language::Plain),
            Palette::default(),
            false,
        ))
        .id(day::id!(response_body))
}

/// One `name: value` line per pair, selectable.
fn pair_lines(pairs: Vec<(String, String)>) -> impl Piece {
    column(
        pairs
            .into_iter()
            .map(|(name, value)| {
                label(format!("{name}: {value}")).font(Font::Footnote).monospace().selectable()
            })
            .collect::<Vec<_>>(),
    )
    .spacing(2.0)
    .align(HAlign::Leading)
}

/// What came back, what went out (with authentication and cookies applied), and where to.
fn response_headers(sent: Sent) -> impl Piece {
    let mut details = vec![(res::str::detail_url().format(), sent.url.clone())];
    if !sent.version.is_empty() {
        details.push((res::str::detail_version().format(), sent.version.clone()));
    }
    if !sent.remote_addr.is_empty() {
        details.push((res::str::detail_remote_address().format(), sent.remote_addr.clone()));
    }
    details.push((
        res::str::detail_time_to_headers().format(),
        format!("{} ms", sent.elapsed_headers_ms),
    ));
    details.push((res::str::detail_dns().format(), format!("{} ms", sent.elapsed_dns_ms)));
    column((
        label(res::str::section_response_headers()).font(Font::Headline),
        pair_lines(sent.headers).id(day::id!(response_headers)),
        label(res::str::section_request_headers()).font(Font::Headline),
        pair_lines(sent.request_headers).id(day::id!(request_headers)),
        label(res::str::section_details()).font(Font::Headline),
        pair_lines(details),
    ))
    .spacing(6.0)
    .align(HAlign::Leading)
}

/// The send as it happened, one event a line: `>` sent, `<` received, `*` a note.
fn response_timeline(rows: Vec<TimelineRow>) -> impl Piece {
    let empty = rows.is_empty();
    column((
        when(move || empty, || label(res::str::timeline_none()).font(Font::Footnote).secondary()),
        column(
            rows.into_iter()
                .map(|row| {
                    // Notes recede; what crossed the wire reads at full strength.
                    label(format!("{} {}", row.mark, row.text))
                        .font(Font::Footnote)
                        .monospace()
                        .apply_if(row.mark == '*', |line| line.secondary())
                        .selectable()
                })
                .collect::<Vec<_>>(),
        )
        .spacing(2.0)
        .align(HAlign::Leading),
    ))
    .align(HAlign::Leading)
    .id(day::id!(timeline))
}

// ---- settings ---------------------------------------------------------------------------------

/// The Data group of Settings (https://daybrite.dev/docs/settings): which store this client
/// opened, the desktop app's or its own, and where it lives. Appearance and language are Day's
/// own General group (lib.rs).
pub(crate) fn data_group() -> Group {
    let engine = crate::engine::engine();
    let store =
        if engine.shared_with_desktop { res::str::store_shared() } else { res::str::store_own() };
    Group::new("data", res::str::settings_data())
        .icon(Symbol::Folder)
        .row(rows::info(res::str::store(), store).id(day::id!(store)))
        .row(
            rows::info(res::str::data_dir(), engine.data_dir.display().to_string())
                .copyable()
                .id(day::id!(data_dir)),
        )
}
