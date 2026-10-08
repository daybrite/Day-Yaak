//! The per-window `Scene`: which workspace and request a window shows, the editor's bindings
//! over the open request, and the last response.
//!
//! Yaak's store is the document; the scene is a projection of it. Reads go straight to the
//! database through the engine. The editor's fields are plain signals, loaded when a request is
//! opened and written back as the user types, so the store (and the desktop app, when it has
//! the same store open) always holds what the editor shows.

use crate::engine::{self, Sent, engine};
use day::model::Op;
use day::prelude::*;
use yaak_models::models::{Environment, HttpRequestHeader, Workspace};

/// The methods the picker offers, in its order.
pub(crate) const METHODS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

/// The body kinds the picker offers: none, JSON text, plain text. Yaak stores the text under
/// `body.text` and the kind as `body_type`.
pub(crate) const BODY_KINDS: [&str; 3] = ["body_none", "body_json", "body_text"];
const BODY_TYPES: [Option<&str>; 3] = [None, Some("application/json"), Some("other")];

/// One request as the list shows it.
#[derive(Clone, PartialEq)]
pub(crate) struct RequestRow {
    pub id: String,
    pub name: String,
    pub method: String,
    pub url: String,
    /// The folder's name, for the caption; empty at the workspace root.
    pub folder: String,
}

/// One header in the editor. `#[derive(Observable)]` gives each field a two-way binding.
#[derive(Observable, Clone, PartialEq)]
pub(crate) struct HeaderRow {
    #[obs(key)]
    pub id: u64,
    pub enabled: bool,
    pub name: String,
    pub value: String,
}

/// Everything one window owns. `Copy`, because every field is a handle.
#[derive(Clone, Copy)]
pub(crate) struct Scene {
    pub workspaces: Signal<Vec<Workspace>>,
    /// The nav selection: a workspace id, or `settings` where that is a nav row.
    pub section: Signal<Option<String>>,
    pub requests: Signal<Vec<RequestRow>>,
    pub environments: Signal<Vec<Environment>>,
    /// Index into the environment picker: 0 is the base environment alone.
    pub environment: Signal<usize>,
    /// The open request's id.
    pub selected: Signal<Option<String>>,
    /// Whether the editor is showing, where only one pane shows at a time.
    pub detail_open: Signal<bool>,
    /// A row the list should scroll to, cleared once it has.
    pub scroll_to: Signal<Option<usize>>,

    // The editor over the open request.
    pub name: Signal<String>,
    pub method: Signal<usize>,
    pub url: Signal<String>,
    pub body_kind: Signal<usize>,
    pub body: Signal<String>,
    pub headers: Store<Keyed<HeaderRow>>,
    /// Set while a request is being loaded into the editor, so the writes-back stay quiet.
    loading: Signal<bool>,

    pub sending: Signal<bool>,
    pub response: Signal<Option<Sent>>,
    /// Which pane a compact window shows: the request or the response.
    pub pane: Signal<usize>,
}

impl Ambient for Scene {
    fn create() -> Self {
        let workspaces = engine().db().list_workspaces().unwrap_or_default();
        let first = workspaces.first().map(|w| w.id.clone());
        Scene {
            workspaces: Signal::new(workspaces),
            section: Signal::new(first),
            requests: Signal::new(Vec::new()),
            environments: Signal::new(Vec::new()),
            environment: Signal::new(0),
            selected: Signal::new(None),
            detail_open: Signal::new(false),
            scroll_to: Signal::new(None),
            name: Signal::new(String::new()),
            method: Signal::new(0),
            url: Signal::new(String::new()),
            body_kind: Signal::new(0),
            body: Signal::new(String::new()),
            headers: Store::new(Keyed::new(Vec::new())),
            loading: Signal::new(false),
            sending: Signal::new(false),
            response: Signal::new(None),
            pane: Signal::new(0),
        }
    }
}

impl Scene {
    /// Follow the window's state: reload the request list when the workspace changes, and write
    /// the editor back to the store as it changes.
    pub(crate) fn install(self) {
        // `watch` reports changes; the first workspace is already selected, so load it now.
        self.reload_requests();
        self.reload_environments();
        watch(
            move || self.section.get(),
            move |_, _| {
                self.selected.set(None);
                self.detail_open.set(false);
                self.response.set(None);
                self.reload_requests();
                self.reload_environments();
            },
        );
        // One subscription per editor field: a tracked read of each, then one write.
        let save = move || {
            if !self.loading.get_untracked() {
                self.save_editor();
            }
        };
        watch(move || self.name.get(), move |_, _| save());
        watch(move || self.method.get(), move |_, _| save());
        watch(move || self.url.get(), move |_, _| save());
        watch(move || self.body_kind.get(), move |_, _| save());
        watch(move || self.body.get(), move |_, _| save());
        let headers = self.headers;
        watch(
            move || {
                headers.with(|_| {});
                headers.version()
            },
            move |_, _| save(),
        );
    }

    pub(crate) fn workspace_id(self) -> Option<String> {
        self.section.get().filter(|s| s != crate::SETTINGS)
    }

    pub(crate) fn workspace_name(self) -> String {
        let id = self.workspace_id();
        self.workspaces
            .get()
            .iter()
            .find(|w| Some(&w.id) == id.as_ref())
            .map(|w| w.name.clone())
            .unwrap_or_default()
    }

    /// The request list for the open workspace, folders first by name, then by Yaak's own
    /// sort order.
    pub(crate) fn reload_requests(self) {
        let Some(workspace_id) = self.workspace_id() else {
            self.requests.set(Vec::new());
            return;
        };
        let db = engine().db();
        let folders = db.list_folders(&workspace_id).unwrap_or_default();
        let mut rows: Vec<(String, f64, RequestRow)> = db
            .list_http_requests(&workspace_id)
            .unwrap_or_default()
            .into_iter()
            .map(|r| {
                let folder = r
                    .folder_id
                    .as_ref()
                    .and_then(|id| folders.iter().find(|f| &f.id == id))
                    .map(|f| f.name.clone())
                    .unwrap_or_default();
                (
                    folder.clone(),
                    r.sort_priority,
                    RequestRow {
                        id: r.id,
                        name: if r.name.is_empty() { r.url.clone() } else { r.name },
                        method: r.method,
                        url: r.url,
                        folder,
                    },
                )
            })
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));
        self.requests.set(rows.into_iter().map(|(_, _, r)| r).collect());
    }

    /// The workspace's sub-environments, for the picker; the base environment always applies.
    fn reload_environments(self) {
        let envs = self
            .workspace_id()
            .and_then(|id| engine().db().list_environments(&id).ok())
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e.parent_model != "workspace" || e.parent_id.is_some())
            .collect::<Vec<_>>();
        self.environments.set(envs);
        self.environment.set(0);
    }

    /// The active sub-environment's id, for a send.
    pub(crate) fn environment_id(self) -> Option<String> {
        let index = self.environment.get_untracked();
        index
            .checked_sub(1)
            .and_then(|i| self.environments.get_untracked().get(i).map(|e| e.id.clone()))
    }

    /// Open a request in the editor. Where the editor appears is the nav host's call: beside
    /// the list on a wide window, pushed over it on a narrow one.
    pub(crate) fn open(self, id: &str) {
        let Ok(request) = engine().db().get_http_request(id) else {
            return;
        };
        self.loading.set(true);
        self.selected.set(Some(id.to_string()));
        self.name.set(request.name.clone());
        self.method
            .set(METHODS.iter().position(|m| m.eq_ignore_ascii_case(&request.method)).unwrap_or(0));
        self.url.set(request.url.clone());
        self.body_kind.set(
            BODY_TYPES
                .iter()
                .position(|t| *t == request.body_type.as_deref())
                .unwrap_or(if request.body_type.is_some() { 2 } else { 0 }),
        );
        self.body
            .set(request.body.get("text").and_then(|v| v.as_str()).unwrap_or_default().to_string());
        self.headers.restructure("load", Op::Set, 0, |k| {
            for id in k.items().iter().map(|h| h.id).collect::<Vec<_>>() {
                k.remove(id);
            }
            for (i, h) in request.headers.iter().enumerate() {
                k.push(HeaderRow {
                    id: i as u64 + 1,
                    enabled: h.enabled,
                    name: h.name.clone(),
                    value: h.value.clone(),
                });
            }
        });
        self.response.set(engine().last_response(id));
        self.pane.set(0);
        self.loading.set(false);
        self.detail_open.set(true);
    }

    pub(crate) fn clear_selection(self) {
        self.selected.set(None);
    }

    /// Write the editor's fields over the open request.
    fn save_editor(self) {
        let Some(id) = self.selected.get_untracked() else {
            return;
        };
        let db = engine().db();
        let Ok(mut request) = db.get_http_request(&id) else {
            return;
        };
        request.name = self.name.get_untracked();
        request.method = METHODS[self.method.get_untracked().min(METHODS.len() - 1)].to_string();
        request.url = self.url.get_untracked();
        request.body_type =
            BODY_TYPES[self.body_kind.get_untracked().min(BODY_TYPES.len() - 1)].map(String::from);
        let body = self.body.get_untracked();
        if request.body_type.is_some() {
            request.body.insert("text".to_string(), serde_json::Value::String(body));
        } else {
            request.body.clear();
        }
        request.headers = self.headers.with_untracked(|k| {
            k.items()
                .iter()
                .map(|h| HttpRequestHeader {
                    enabled: h.enabled,
                    name: h.name.clone(),
                    value: h.value.clone(),
                    id: None,
                })
                .collect()
        });
        let saved = engine()
            .query_manager()
            .with_tx(|tx| tx.upsert_http_request(&request, &engine::update_source()));
        if let Err(e) = saved {
            log::warn!("cannot save the request: {e}");
            return;
        }
        // The list shows the name, method and URL, so it follows the edit.
        self.requests.update(|rows| {
            if let Some(row) = rows.iter_mut().find(|r| r.id == id) {
                row.name = if request.name.is_empty() { request.url.clone() } else { request.name };
                row.method = request.method;
                row.url = request.url;
            }
        });
    }

    /// A new request at the end of the workspace, opened for editing.
    pub(crate) fn new_request(self) {
        let Some(workspace_id) = self.workspace_id() else {
            return;
        };
        let db = engine().db();
        let last = db
            .list_http_requests(&workspace_id)
            .unwrap_or_default()
            .iter()
            .map(|r| r.sort_priority)
            .fold(0.0_f64, f64::max);
        let request = engine::blank_request(&workspace_id, None, last + 1.0);
        let saved = engine()
            .query_manager()
            .with_tx(|tx| tx.upsert_http_request(&request, &engine::update_source()));
        match saved {
            Ok(saved) => {
                self.reload_requests();
                self.open(&saved.id);
                self.scroll_to
                    .set(self.requests.get_untracked().iter().position(|r| r.id == saved.id));
            }
            Err(e) => log::warn!("cannot create a request: {e}"),
        }
    }

    pub(crate) fn delete_request(self, id: &str) {
        let deleted = engine()
            .query_manager()
            .with_tx(|tx| tx.delete_http_request_by_id(id, &engine::update_source()));
        if let Err(e) = deleted {
            log::warn!("cannot delete the request: {e}");
            return;
        }
        if self.selected.get_untracked().as_deref() == Some(id) {
            self.selected.set(None);
            self.response.set(None);
        }
        self.reload_requests();
    }

    pub(crate) fn delete_selected(self) {
        if let Some(id) = self.selected.get_untracked() {
            self.delete_request(&id);
        }
    }

    pub(crate) fn add_header(self) {
        let id =
            self.headers.with_untracked(|k| k.items().iter().map(|h| h.id).max().unwrap_or(0)) + 1;
        self.headers.restructure("add", Op::Insert, id, |k| {
            k.push(HeaderRow { id, enabled: true, name: String::new(), value: String::new() })
        });
    }

    pub(crate) fn remove_header(self, id: u64) {
        self.headers.restructure("remove", Op::Delete, id, |k| {
            k.remove(id);
        });
    }

    /// Send the open request. The engine runs it on its own runtime and the setter brings the
    /// outcome back to this thread.
    pub(crate) fn send(self) {
        let Some(id) = self.selected.get_untracked() else {
            return;
        };
        if self.sending.get_untracked() {
            return;
        }
        // Whatever is typed is sent: the writes-back run on the same turn as the edit.
        self.sending.set(true);
        self.pane.set(1);
        let sending = self.sending;
        let response = self.response.setter();
        let done = Signal::new(None::<Sent>);
        watch(
            move || done.get(),
            move |sent, _| {
                if let Some(sent) = sent.clone() {
                    sending.set(false);
                    response.set(Some(sent));
                }
            },
        );
        engine().send(id, self.environment_id(), done.setter());
    }

    /// The open request's row, for the window title and the pushed page's bar.
    pub(crate) fn selected_row(self) -> Option<RequestRow> {
        let id = self.selected.get()?;
        self.requests.get().into_iter().find(|r| r.id == id)
    }

    /// The display index of the open request, for the list's highlight.
    pub(crate) fn selected_index(self) -> Option<usize> {
        let id = self.selected.get()?;
        self.requests.get().iter().position(|r| r.id == id)
    }
}
