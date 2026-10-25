//! The per-window `Scene`: which workspace and request a window shows, the editor's bindings
//! over the open request, and the response on show with its history.
//!
//! Yaak's store is the document; the scene is a projection of it. Reads go straight to the
//! database through the engine. The editor's fields are plain signals, loaded when a request is
//! opened and written back as the user types, so the store (and the desktop app, when it has
//! the same store open) always holds what the editor shows. A write happens only when a field
//! differs from what is stored, so opening a request never touches it, and whatever the stored
//! request holds that this editor has no field for is written back as found.

use crate::engine::{self, NATIVE_AUTH_TYPES, ResponseSummary, Sent, engine};
use day::model::Op;
use day::prelude::*;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use yaak_models::models::{
    Environment, HttpRequest, HttpRequestHeader, HttpUrlParameter, HttpVersion,
    InheritedBoolSetting, Workspace,
};

/// The methods the picker offers, in its order.
pub(crate) const METHODS: [&str; 7] = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

/// The request editor's tabs, by title.
pub(crate) const TABS: [fn() -> day::LocalizedText; 5] = [
    crate::res::str::tab_body,
    crate::res::str::tab_params,
    crate::res::str::tab_headers,
    crate::res::str::tab_auth,
    crate::res::str::tab_settings,
];

/// The body kinds the picker offers, by title, and the `body_type` Yaak stores for each.
/// Text kinds keep their text under `body.text`, the two forms their fields under `body.form`,
/// GraphQL its `query` and `variables`, and a file upload its `filePath`.
pub(crate) const BODY_KINDS: [fn() -> day::LocalizedText; 8] = [
    crate::res::str::body_none,
    crate::res::str::body_json,
    crate::res::str::body_text,
    crate::res::str::body_xml,
    crate::res::str::body_form,
    crate::res::str::body_multipart,
    crate::res::str::body_graphql,
    crate::res::str::body_binary,
];
const BODY_TYPES: [Option<&str>; 8] = [
    None,
    Some("application/json"),
    Some("other"),
    Some("text/xml"),
    Some("application/x-www-form-urlencoded"),
    Some("multipart/form-data"),
    Some("graphql"),
    Some("binary"),
];
/// Indexes into [`BODY_KINDS`].
pub(crate) mod body {
    pub const NONE: usize = 0;
    pub const JSON: usize = 1;
    pub const TEXT: usize = 2;
    pub const XML: usize = 3;
    pub const FORM: usize = 4;
    pub const MULTIPART: usize = 5;
    pub const GRAPHQL: usize = 6;
    pub const BINARY: usize = 7;
}

/// The authentication choices, by title: what the folder or workspace gives, none, and
/// the three types this client applies itself ([`NATIVE_AUTH_TYPES`], in the same order).
pub(crate) const AUTH_KINDS: [fn() -> day::LocalizedText; 5] = [
    crate::res::str::auth_inherit,
    crate::res::str::auth_none,
    crate::res::str::auth_basic,
    crate::res::str::auth_bearer,
    crate::res::str::auth_apikey,
];
/// Indexes into [`AUTH_KINDS`], and one past it for a type only Yaak's plugins provide.
pub(crate) mod auth {
    pub const INHERIT: usize = 0;
    pub const NONE: usize = 1;
    pub const BASIC: usize = 2;
    pub const BEARER: usize = 3;
    pub const API_KEY: usize = 4;
    /// A plugin's type (OAuth 2, AWS, …): shown by name and kept exactly as stored.
    pub const FOREIGN: usize = 5;
}
/// Where an API key goes, by title; Yaak stores `header` or `query`.
pub(crate) const AUTH_LOCATIONS: [fn() -> day::LocalizedText; 2] = [
    crate::res::str::auth_location_header,
    crate::res::str::auth_location_query,
];

/// A setting the request may leave to its folder and workspace, by title.
pub(crate) const TRI_STATE: [fn() -> day::LocalizedText; 3] = [
    crate::res::str::setting_inherit,
    crate::res::str::setting_on,
    crate::res::str::setting_off,
];
/// The HTTP version choices, by title.
pub(crate) const HTTP_VERSIONS: [fn() -> day::LocalizedText; 4] = [
    crate::res::str::setting_inherit,
    crate::res::str::http_version_auto,
    crate::res::str::http_version_1,
    crate::res::str::http_version_2,
];

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

/// One name and value in the editor: a header, a URL parameter or a form field.
/// `#[derive(Observable)]` gives each field a two-way binding.
#[derive(Observable, Clone, PartialEq)]
pub(crate) struct PairRow {
    #[obs(key)]
    pub id: u64,
    pub enabled: bool,
    pub name: String,
    pub value: String,
    /// The file a multipart field uploads; empty for a field that sends its value.
    pub file: String,
    /// What the stored row holds beyond these (its id, a part's content type and file name),
    /// written back as found.
    #[obs(skip)]
    pub extra: Map<String, Value>,
}

impl PairRow {
    fn new(id: u64, enabled: bool, name: &str, value: &str) -> PairRow {
        PairRow {
            id,
            enabled,
            name: name.to_string(),
            value: value.to_string(),
            file: String::new(),
            extra: Map::new(),
        }
    }

    /// A stored row with its Yaak id kept for the write-back.
    fn stored(
        index: usize,
        enabled: bool,
        name: &str,
        value: &str,
        id: &Option<String>,
    ) -> PairRow {
        let mut row = PairRow::new(index as u64 + 1, enabled, name, value);
        if let Some(id) = id {
            row.extra.insert("id".to_string(), Value::String(id.clone()));
        }
        row
    }

    fn stored_id(&self) -> Option<String> {
        self.extra.get("id").and_then(Value::as_str).map(str::to_string)
    }

    /// A field of `body.form`, as Yaak stores one.
    fn from_form(index: usize, field: &Value) -> PairRow {
        let text = |key: &str| field.get(key).and_then(Value::as_str).unwrap_or_default();
        let mut row = PairRow::new(
            index as u64 + 1,
            field.get("enabled").and_then(Value::as_bool).unwrap_or(true),
            text("name"),
            text("value"),
        );
        row.file = text("file").to_string();
        if let Some(object) = field.as_object() {
            row.extra = object
                .iter()
                .filter(|(k, _)| !["enabled", "name", "value", "file"].contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
        }
        row
    }

    fn to_form(&self) -> Value {
        let mut field = self.extra.clone();
        field.insert("enabled".to_string(), Value::Bool(self.enabled));
        field.insert("name".to_string(), Value::String(self.name.clone()));
        field.insert("value".to_string(), Value::String(self.value.clone()));
        if !self.file.is_empty() {
            field.insert("file".to_string(), Value::String(self.file.clone()));
        }
        Value::Object(field)
    }
}

/// A keyed store of rows, as the scene holds its headers, parameters and form fields.
pub(crate) type Rows = Store<Keyed<PairRow>>;

fn load_rows(rows: Rows, loaded: Vec<PairRow>) {
    rows.restructure("load", Op::Set, 0, |k| {
        for id in k.items().iter().map(|r| r.id).collect::<Vec<_>>() {
            k.remove(id);
        }
        for row in loaded {
            k.push(row);
        }
    });
}

fn rows_of(rows: Rows) -> Vec<PairRow> {
    rows.with_untracked(|k| k.items().to_vec())
}

/// A string stored among a request's authentication values, or `default` where there is none.
fn stored_text(values: &BTreeMap<String, Value>, key: &str, default: &str) -> String {
    match values.get(key) {
        None | Some(Value::Null) => default.to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// Write `value` under `key` unless that is what the stored values already amount to, so an
/// unset value with a default stays unset.
fn store_text(values: &mut BTreeMap<String, Value>, key: &str, value: String, default: &str) {
    if stored_text(values, key, default) != value {
        values.insert(key.to_string(), Value::String(value));
    }
}

fn tri_state(setting: &InheritedBoolSetting) -> usize {
    match (setting.enabled, setting.value) {
        (false, _) => 0,
        (true, true) => 1,
        (true, false) => 2,
    }
}

fn from_tri_state(index: usize, stored: &InheritedBoolSetting) -> InheritedBoolSetting {
    match index {
        1 => InheritedBoolSetting { enabled: true, value: true },
        2 => InheritedBoolSetting { enabled: true, value: false },
        // Inheriting keeps the value the override last had, as Yaak does.
        _ => InheritedBoolSetting { enabled: false, value: stored.value },
    }
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
    /// Which of [`TABS`] the request pane shows.
    pub tab: Signal<usize>,
    pub params: Rows,
    pub headers: Rows,

    /// Index into [`BODY_KINDS`].
    pub body_kind: Signal<usize>,
    /// The kind whose side effects (the Content-Type header, the method) have been applied: a
    /// loaded request's own, so only a choice the user makes sets them.
    body_kind_applied: Signal<usize>,
    /// A stored text `body_type` the picker has no entry for, kept while the kind stays Text.
    foreign_body_type: Signal<Option<String>>,
    pub body: Signal<String>,
    pub form: Rows,
    pub graphql_query: Signal<String>,
    pub graphql_variables: Signal<String>,
    pub binary_path: Signal<String>,

    /// Index into [`AUTH_KINDS`], or [`auth::FOREIGN`].
    pub auth_kind: Signal<usize>,
    /// The stored type's name while the kind is [`auth::FOREIGN`].
    pub auth_foreign: Signal<String>,
    pub auth_enabled: Signal<bool>,
    pub auth_username: Signal<String>,
    pub auth_password: Signal<String>,
    pub auth_token: Signal<String>,
    pub auth_prefix: Signal<String>,
    /// Index into [`AUTH_LOCATIONS`].
    pub auth_location: Signal<usize>,
    pub auth_key: Signal<String>,
    pub auth_value: Signal<String>,

    // Per-request settings: indexes into [`TRI_STATE`] and [`HTTP_VERSIONS`].
    pub follow_redirects: Signal<usize>,
    pub validate_certificates: Signal<usize>,
    pub send_cookies: Signal<usize>,
    pub store_cookies: Signal<usize>,
    pub http_version: Signal<usize>,
    /// The timeout in milliseconds as typed; empty inherits.
    pub timeout: Signal<String>,

    pub sending: Signal<bool>,
    /// The request the send in flight belongs to, for Cancel.
    sending_request: Signal<Option<String>>,
    pub response: Signal<Option<Sent>>,
    /// The open request's stored responses, newest first, and which one is on show.
    pub history: Signal<Vec<ResponseSummary>>,
    pub history_index: Signal<usize>,
    /// The history entry whose response has been loaded, so only a choice the user makes loads.
    history_applied: Signal<usize>,
    /// Which part of the response shows: body, headers or timeline.
    pub response_tab: Signal<usize>,
    /// Which pane a compact window shows: the request or the response.
    pub pane: Signal<usize>,
    /// The wide editor's layout: the response under the request (`true`) or beside it.
    pub stacked: Signal<bool>,
    /// The request pane's share of the wide editor's split.
    pub split: Signal<f64>,
}

impl Ambient for Scene {
    fn create() -> Self {
        let workspaces = engine().db().list_workspaces().unwrap_or_default();
        let first = workspaces.first().map(|w| w.id.clone());
        let text = || Signal::new(String::new());
        let rows = || Store::new(Keyed::new(Vec::new()));
        Scene {
            workspaces: Signal::new(workspaces),
            section: Signal::new(first),
            requests: Signal::new(Vec::new()),
            environments: Signal::new(Vec::new()),
            environment: Signal::new(0),
            selected: Signal::new(None),
            detail_open: Signal::new(false),
            scroll_to: Signal::new(None),
            name: text(),
            method: Signal::new(0),
            url: text(),
            tab: Signal::new(0),
            params: rows(),
            headers: rows(),
            body_kind: Signal::new(0),
            body_kind_applied: Signal::new(0),
            foreign_body_type: Signal::new(None),
            body: text(),
            form: rows(),
            graphql_query: text(),
            graphql_variables: text(),
            binary_path: text(),
            auth_kind: Signal::new(0),
            auth_foreign: text(),
            auth_enabled: Signal::new(true),
            auth_username: text(),
            auth_password: text(),
            auth_token: text(),
            auth_prefix: text(),
            auth_location: Signal::new(0),
            auth_key: text(),
            auth_value: text(),
            follow_redirects: Signal::new(0),
            validate_certificates: Signal::new(0),
            send_cookies: Signal::new(0),
            store_cookies: Signal::new(0),
            http_version: Signal::new(0),
            timeout: text(),
            sending: Signal::new(false),
            sending_request: Signal::new(None),
            response: Signal::new(None),
            history: Signal::new(Vec::new()),
            history_index: Signal::new(0),
            history_applied: Signal::new(0),
            response_tab: Signal::new(0),
            pane: Signal::new(0),
            stacked: Signal::new(false),
            // A little past half: the request side holds rows of controls, the response text.
            split: Signal::new(0.56),
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
                self.history.set(Vec::new());
                self.reload_requests();
                self.reload_environments();
            },
        );
        // One subscription per editor field: a tracked read of each, then one write, which
        // `save_editor` skips when the store already holds what the editor shows.
        let save = move || self.save_editor();
        for text in [
            self.name,
            self.url,
            self.body,
            self.graphql_query,
            self.graphql_variables,
            self.binary_path,
            self.auth_username,
            self.auth_password,
            self.auth_token,
            self.auth_prefix,
            self.auth_key,
            self.auth_value,
            self.timeout,
        ] {
            watch(move || text.get(), move |_, _| save());
        }
        for choice in [
            self.method,
            self.auth_kind,
            self.auth_location,
            self.follow_redirects,
            self.validate_certificates,
            self.send_cookies,
            self.store_cookies,
            self.http_version,
        ] {
            watch(move || choice.get(), move |_, _| save());
        }
        watch(move || self.auth_enabled.get(), move |_, _| save());
        for rows in [self.params, self.headers, self.form] {
            watch(
                move || {
                    rows.with(|_| {});
                    rows.version()
                },
                move |_, _| save(),
            );
        }
        // A body kind the user picks also sets the Content-Type header and, where a body needs
        // one, the method; a kind that arrives with a loaded request sets nothing.
        watch(
            move || self.body_kind.get(),
            move |kind, _| {
                if *kind != self.body_kind_applied.get_untracked() {
                    self.body_kind_chosen(*kind);
                }
                save();
            },
        );
        // Likewise for the history picker: a choice loads that response, a refresh does not.
        watch(
            move || self.history_index.get(),
            move |index, _| {
                if *index != self.history_applied.get_untracked() {
                    self.show_history(*index);
                }
            },
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
        self.selected.set(Some(id.to_string()));
        self.name.set(request.name.clone());
        self.method
            .set(METHODS.iter().position(|m| m.eq_ignore_ascii_case(&request.method)).unwrap_or(0));
        self.url.set(request.url.clone());
        load_rows(
            self.params,
            request
                .url_parameters
                .iter()
                .enumerate()
                .map(|(i, p)| PairRow::stored(i, p.enabled, &p.name, &p.value, &p.id))
                .collect(),
        );
        load_rows(
            self.headers,
            request
                .headers
                .iter()
                .enumerate()
                .map(|(i, h)| PairRow::stored(i, h.enabled, &h.name, &h.value, &h.id))
                .collect(),
        );
        self.load_body(&request);
        self.load_authentication(&request);
        self.follow_redirects.set(tri_state(&request.setting_follow_redirects));
        self.validate_certificates.set(tri_state(&request.setting_validate_certificates));
        self.send_cookies.set(tri_state(&request.setting_send_cookies));
        self.store_cookies.set(tri_state(&request.setting_store_cookies));
        self.http_version.set(
            match (request.setting_http_version.enabled, &request.setting_http_version.value) {
                (false, _) => 0,
                (true, HttpVersion::Auto) => 1,
                (true, HttpVersion::Http1) => 2,
                (true, HttpVersion::Http2) => 3,
            },
        );
        self.timeout.set(if request.setting_request_timeout.enabled {
            request.setting_request_timeout.value.to_string()
        } else {
            String::new()
        });

        // The history is a list of facts, read here; the response it names arrives a turn
        // later, from a pool thread, and a selection that moved on meanwhile keeps its own.
        self.response.set(None);
        self.history.set(engine().history(id));
        self.history_applied.set(0);
        self.history_index.set(0);
        self.load_response(id.to_string(), None);
        self.tab.set(0);
        self.response_tab.set(0);
        self.pane.set(0);
        self.detail_open.set(true);
    }

    fn load_body(self, request: &HttpRequest) {
        let kind = BODY_TYPES.iter().position(|t| *t == request.body_type.as_deref());
        // A text type the picker has no entry for edits as Text and keeps its own name.
        self.foreign_body_type.set(match kind {
            None => request.body_type.clone(),
            Some(_) => None,
        });
        let kind = kind.unwrap_or(body::TEXT);
        self.body_kind_applied.set(kind);
        self.body_kind.set(kind);
        let text = |key: &str| {
            request.body.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
        };
        self.body.set(text("text"));
        self.graphql_query.set(text("query"));
        self.graphql_variables.set(text("variables"));
        self.binary_path.set(text("filePath"));
        load_rows(
            self.form,
            request
                .body
                .get("form")
                .and_then(Value::as_array)
                .map(|fields| {
                    fields.iter().enumerate().map(|(i, f)| PairRow::from_form(i, f)).collect()
                })
                .unwrap_or_default(),
        );
    }

    fn load_authentication(self, request: &HttpRequest) {
        let values = &request.authentication;
        let kind = match request.authentication_type.as_deref() {
            None => auth::INHERIT,
            Some("none") => auth::NONE,
            Some(name) => match NATIVE_AUTH_TYPES.iter().position(|t| *t == name) {
                Some(native) => auth::BASIC + native,
                None => auth::FOREIGN,
            },
        };
        self.auth_foreign.set(if kind == auth::FOREIGN {
            request.authentication_type.clone().unwrap_or_default()
        } else {
            String::new()
        });
        self.auth_kind.set(kind);
        self.auth_enabled.set(values.get("disabled").and_then(Value::as_bool) != Some(true));
        self.auth_username.set(stored_text(values, "username", ""));
        self.auth_password.set(stored_text(values, "password", ""));
        self.auth_token.set(stored_text(values, "token", ""));
        self.auth_prefix.set(stored_text(values, "prefix", "Bearer"));
        self.auth_location.set(if stored_text(values, "location", "header") == "query" {
            1
        } else {
            0
        });
        self.auth_key.set(stored_text(values, "key", ""));
        self.auth_value.set(stored_text(values, "value", ""));
    }

    /// Show one stored response of `request_id`: the entry `response_id` names, or the latest.
    fn load_response(self, request_id: String, response_id: Option<String>) {
        let (selected, response) = (self.selected, self.response);
        day::task(async move {
            let sent = engine().response_async(request_id.clone(), response_id).await;
            if selected.get_untracked().as_deref() == Some(request_id.as_str()) {
                response.set(sent);
            }
        });
    }

    /// The history picker moved to `index`: show that response.
    fn show_history(self, index: usize) {
        self.history_applied.set(index);
        let Some(request_id) = self.selected.get_untracked() else {
            return;
        };
        if let Some(entry) = self.history.get_untracked().get(index) {
            self.load_response(request_id, Some(entry.id.clone()));
        }
    }

    /// Forget the open request's stored responses, once the user confirms.
    pub(crate) fn clear_history(self) {
        let Some(request_id) = self.selected.get_untracked() else {
            return;
        };
        day::task(async move {
            let confirmed = confirm(crate::res::str::confirm_clear_history())
                .message(crate::res::str::confirm_clear_history_detail())
                .confirm_label(crate::res::str::cmd_clear_history())
                .destructive()
                .await;
            if !confirmed {
                return;
            }
            engine().clear_history(&request_id);
            if self.selected.get_untracked().as_deref() == Some(request_id.as_str()) {
                self.history.set(Vec::new());
                self.history_applied.set(0);
                self.history_index.set(0);
                self.response.set(None);
            }
        });
    }

    pub(crate) fn clear_selection(self) {
        self.selected.set(None);
    }

    /// The user picked a body kind: give the request the Content-Type header that kind is sent
    /// with, and a method that carries a body where the kind needs one, as Yaak's editor does.
    fn body_kind_chosen(self, kind: usize) {
        let previous = self.body_kind_applied.get_untracked();
        self.body_kind_applied.set(kind);
        self.foreign_body_type.set(None);
        // `None` leaves the header alone (a file upload names its own type or none).
        let content_type = match kind {
            body::NONE => Some(None),
            body::JSON | body::GRAPHQL => Some(Some("application/json")),
            body::TEXT => Some(Some("text/plain")),
            body::XML => Some(Some("text/xml")),
            body::FORM => Some(Some("application/x-www-form-urlencoded")),
            body::MULTIPART => Some(Some("multipart/form-data")),
            _ => None,
        };
        let is_get = self.method.get_untracked() == 0;
        let needs_post = match kind {
            body::GRAPHQL | body::MULTIPART => true,
            body::JSON | body::TEXT | body::XML | body::FORM => previous == body::NONE && is_get,
            _ => false,
        };
        if needs_post {
            self.method.set(METHODS.iter().position(|m| *m == "POST").unwrap_or(1));
        }
        if let Some(content_type) = content_type {
            let next = self.next_row_id(self.headers);
            self.headers.restructure("content type", Op::Set, 0, |k| {
                let stale = k
                    .items()
                    .iter()
                    .filter(|h| h.name.eq_ignore_ascii_case("content-type"))
                    .map(|h| h.id)
                    .collect::<Vec<_>>();
                for id in stale {
                    k.remove(id);
                }
                if let Some(value) = content_type {
                    k.push(PairRow::new(next, true, "Content-Type", value));
                }
            });
        }
    }

    /// The open request as the editor describes it, over what is stored for everything the
    /// editor has no field for.
    fn edited(self, stored: &HttpRequest) -> HttpRequest {
        let mut request = stored.clone();
        request.name = self.name.get_untracked();
        request.method = METHODS[self.method.get_untracked().min(METHODS.len() - 1)].to_string();
        request.url = self.url.get_untracked();
        request.url_parameters = rows_of(self.params)
            .iter()
            .map(|p| HttpUrlParameter {
                enabled: p.enabled,
                name: p.name.clone(),
                value: p.value.clone(),
                id: p.stored_id(),
            })
            .collect();
        request.headers = rows_of(self.headers)
            .iter()
            .map(|h| HttpRequestHeader {
                enabled: h.enabled,
                name: h.name.clone(),
                value: h.value.clone(),
                id: h.stored_id(),
            })
            .collect();
        self.write_body(&mut request);
        self.write_authentication(&mut request);
        request.setting_follow_redirects =
            from_tri_state(self.follow_redirects.get_untracked(), &stored.setting_follow_redirects);
        request.setting_validate_certificates = from_tri_state(
            self.validate_certificates.get_untracked(),
            &stored.setting_validate_certificates,
        );
        request.setting_send_cookies =
            from_tri_state(self.send_cookies.get_untracked(), &stored.setting_send_cookies);
        request.setting_store_cookies =
            from_tri_state(self.store_cookies.get_untracked(), &stored.setting_store_cookies);
        match self.http_version.get_untracked() {
            0 => request.setting_http_version.enabled = false,
            choice => {
                request.setting_http_version.enabled = true;
                request.setting_http_version.value = match choice {
                    2 => HttpVersion::Http1,
                    3 => HttpVersion::Http2,
                    _ => HttpVersion::Auto,
                };
            }
        }
        // Empty inherits; a number overrides; anything else is a field mid-edit and waits.
        let timeout = self.timeout.get_untracked();
        match timeout.trim() {
            "" => request.setting_request_timeout.enabled = false,
            typed => {
                if let Ok(ms) = typed.parse::<i32>()
                    && ms >= 0
                {
                    request.setting_request_timeout.enabled = true;
                    request.setting_request_timeout.value = ms;
                }
            }
        }
        request
    }

    fn write_body(self, request: &mut HttpRequest) {
        let kind = self.body_kind.get_untracked().min(BODY_TYPES.len() - 1);
        request.body_type = match (kind, self.foreign_body_type.get_untracked()) {
            (body::TEXT, Some(foreign)) => Some(foreign),
            _ => BODY_TYPES[kind].map(String::from),
        };
        // Only the open kind's keys are written, with the options Yaak keeps beside them.
        let kept = |key: &str| request.body.get(key).cloned().map(|v| (key.to_string(), v));
        let text = |key: &str, value: String| (key.to_string(), Value::String(value));
        let mut written: BTreeMap<String, Value> = BTreeMap::new();
        match kind {
            body::NONE => {}
            body::FORM | body::MULTIPART => {
                let fields = rows_of(self.form).iter().map(PairRow::to_form).collect();
                written.insert("form".to_string(), Value::Array(fields));
            }
            body::GRAPHQL => {
                written.extend([text("query", self.graphql_query.get_untracked())]);
                let variables = self.graphql_variables.get_untracked();
                if !variables.is_empty() {
                    written.extend([text("variables", variables)]);
                }
                written.extend(kept("operationName"));
            }
            body::BINARY => {
                let path = self.binary_path.get_untracked();
                if !path.is_empty() {
                    written.extend([text("filePath", path)]);
                }
            }
            _ => {
                written.extend([text("text", self.body.get_untracked())]);
                written.extend(kept("sendJsonComments"));
            }
        }
        request.body = written;
    }

    fn write_authentication(self, request: &mut HttpRequest) {
        let kind = self.auth_kind.get_untracked();
        match kind {
            auth::INHERIT => request.authentication_type = None,
            auth::NONE => request.authentication_type = Some("none".to_string()),
            auth::BASIC | auth::BEARER | auth::API_KEY => {
                request.authentication_type =
                    Some(NATIVE_AUTH_TYPES[kind - auth::BASIC].to_string());
                let values = &mut request.authentication;
                match kind {
                    auth::BASIC => {
                        store_text(values, "username", self.auth_username.get_untracked(), "");
                        store_text(values, "password", self.auth_password.get_untracked(), "");
                    }
                    auth::BEARER => {
                        store_text(values, "token", self.auth_token.get_untracked(), "");
                        store_text(values, "prefix", self.auth_prefix.get_untracked(), "Bearer");
                    }
                    _ => {
                        let location = if self.auth_location.get_untracked() == 1 {
                            "query"
                        } else {
                            "header"
                        };
                        store_text(values, "location", location.to_string(), "header");
                        store_text(values, "key", self.auth_key.get_untracked(), "");
                        store_text(values, "value", self.auth_value.get_untracked(), "");
                    }
                }
                let disabled = !self.auth_enabled.get_untracked();
                if (values.get("disabled").and_then(Value::as_bool) == Some(true)) != disabled {
                    values.insert("disabled".to_string(), Value::Bool(disabled));
                }
            }
            // A plugin's type stays exactly as stored.
            _ => {}
        }
    }

    /// Write the editor's fields over the open request, when they say something new.
    fn save_editor(self) {
        let Some(id) = self.selected.get_untracked() else {
            return;
        };
        let db = engine().db();
        let Ok(stored) = db.get_http_request(&id) else {
            return;
        };
        let request = self.edited(&stored);
        if request == stored {
            return;
        }
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
            self.history.set(Vec::new());
        }
        self.reload_requests();
    }

    pub(crate) fn delete_selected(self) {
        if let Some(id) = self.selected.get_untracked() {
            self.delete_request(&id);
        }
    }

    fn next_row_id(self, rows: Rows) -> u64 {
        rows.with_untracked(|k| k.items().iter().map(|r| r.id).max().unwrap_or(0)) + 1
    }

    /// A new empty row at the end of `rows` (the headers, the parameters or the form).
    pub(crate) fn add_row(self, rows: Rows) {
        let id = self.next_row_id(rows);
        rows.restructure("add", Op::Insert, id, |k| k.push(PairRow::new(id, true, "", "")));
    }

    pub(crate) fn remove_row(self, rows: Rows, id: u64) {
        rows.restructure("remove", Op::Delete, id, |k| {
            k.remove(id);
        });
    }

    /// Ask for a file and hand its path to `picked`. Every platform's picker answers with a
    /// path the engine can read (https://daybrite.dev/docs/reference/files).
    fn pick_file(self, picked: impl FnOnce(String) + 'static) {
        day::task(async move {
            if let Some(file) = open_file().await
                && let Some(path) = file.local_path()
            {
                picked(path.to_string_lossy().into_owned());
            }
        });
    }

    /// Choose the file a binary body uploads.
    pub(crate) fn pick_binary(self) {
        self.pick_file(move |path| self.binary_path.set(path));
    }

    /// Choose the file a multipart field uploads.
    pub(crate) fn pick_form_file(self, id: u64) {
        let form = self.form;
        self.pick_file(move |path| form.elem(id).file().write_commit(path));
    }

    /// Send the open request. The engine runs it on its own runtime and the task brings the
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
        self.sending_request.set(Some(id.clone()));
        self.pane.set(1);
        let environment_id = self.environment_id();
        day::task(async move {
            let sent = engine().send(id.clone(), environment_id).await;
            self.sending.set(false);
            self.sending_request.set(None);
            // The response belongs to the request that sent it; a window that has moved on to
            // another finds it in that request's history.
            if self.selected.get_untracked().as_deref() == Some(id.as_str()) {
                self.history.set(engine().history(&id));
                self.history_applied.set(0);
                self.history_index.set(0);
                self.response.set(Some(sent));
            }
        });
    }

    /// Stop the send in flight. It resolves with the cancellation as its error.
    pub(crate) fn cancel(self) {
        if let Some(id) = self.sending_request.get_untracked() {
            engine().cancel(&id);
        }
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
