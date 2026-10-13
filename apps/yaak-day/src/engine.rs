//! Yaak's engine, hosted for Day.
//!
//! Yaak keeps its workspaces, requests and responses in SQLite through `yaak-models`, and
//! sends requests through `yaak`'s send pipeline, which is async over tokio. Day runs no
//! tokio, so this module owns one runtime on background threads and is the only place that
//! touches it (https://daybrite.dev/docs/async, rule 4). The database is read and written
//! synchronously, on whichever thread asks; a send runs on the runtime inside a
//! `day::spawn_blocking` job, so the UI awaits its outcome like any other future.
//!
//! The plugin runtime (a Node.js sidecar) is not started. Yaak ships every authentication type as
//! a plugin, so this module applies the three that need no more than the request itself (Basic,
//! Bearer and API key) through the pipeline's own hook, with the semantics of Yaak's plugins;
//! the other types and plugin template functions are declined with a clear message. Everything
//! else in a send, including environment variables, inheritance, cookies and redirects, is
//! Yaak's own code.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use async_trait::async_trait;
use base64::Engine as _;
use log::{info, warn};
use tokio::sync::{mpsc, watch};
use yaak::send::{
    CookieBehavior, HttpSendRuntimeConfig, PrepareSendableRequest, ResponseStorage,
    SendHttpRequestParams, SendRequestExecutor, resolve_send_inputs, send_http_request,
};
use yaak_http::client::HttpConnectionOptions;
use yaak_http::manager::HttpConnectionManager;
use yaak_http::sender::{HttpResponse as SenderHttpResponse, HttpResponseEvent, ReqwestSender};
use yaak_http::transaction::HttpTransaction;
use yaak_http::types::{SendableHttpRequest, append_query_params};
use yaak_models::blob_manager::BlobManager;
use yaak_models::client_db::ClientDb;
use yaak_models::models::{HttpRequest, HttpResponse, HttpResponseEventData};
use yaak_models::query_manager::QueryManager;
use yaak_models::util::UpdateSource;
use yaak_templates::TemplateCallback;
use yaak_tls::find_client_certificate;

/// The id Yaak's desktop app stores its data under; the Day client opens that store when it
/// finds one, so both show the same workspaces.
const DESKTOP_APP_ID: &str = "app.yaak.desktop";
/// Where this client keeps its own store when there is no desktop store beside it (every phone).
const OWN_DIR: &str = "yaak-day";
/// The most of a response body the UI shows; the rest stays in the file.
const MAX_BODY_DISPLAY: usize = 512 * 1024;
/// How many past responses the history picker offers for one request.
const HISTORY_LIMIT: u64 = 30;

pub struct Engine {
    query_manager: QueryManager,
    blob_manager: BlobManager,
    connection_manager: HttpConnectionManager,
    runtime: tokio::runtime::Runtime,
    responses_dir: PathBuf,
    /// The cancel switch of each send in flight, by request id.
    cancels: Mutex<HashMap<String, watch::Sender<bool>>>,
    /// Whether the open store is the desktop app's.
    pub shared_with_desktop: bool,
    /// The path of the open store, for the About text.
    pub data_dir: PathBuf,
}

static ENGINE: OnceLock<Engine> = OnceLock::new();

/// The process's engine, opened on first use.
pub fn engine() -> &'static Engine {
    ENGINE.get_or_init(Engine::open)
}

/// The id every model write carries, so Yaak's change log can tell this client's writes apart.
pub fn update_source() -> UpdateSource {
    UpdateSource::from_window_label("day")
}

impl Engine {
    fn open() -> Engine {
        let (data_dir, shared_with_desktop) = choose_data_dir();
        if let Err(e) = std::fs::create_dir_all(&data_dir) {
            warn!("cannot create {}: {e}", data_dir.display());
        }
        info!(
            "Yaak store: {} ({})",
            data_dir.display(),
            if shared_with_desktop { "shared with the desktop app" } else { "this client's own" }
        );
        let (query_manager, blob_manager, _changes) =
            yaak_models::init_standalone(data_dir.join("db.sqlite"), data_dir.join("blobs.sqlite"))
                .unwrap_or_else(|e| {
                    panic!("cannot open the Yaak store at {}: {e}", data_dir.display())
                });
        // A guest beside a possibly live desktop session: only the housekeeping that is safe
        // then, exactly as the CLI does.
        let _ = query_manager.with_tx(|tx| {
            yaak_lifecycle::on_launch(&yaak_lifecycle::Host::guest(), tx, &blob_manager)
        });
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("yaak-engine")
            .enable_all()
            .build()
            .unwrap_or_else(|e| panic!("cannot start the engine runtime: {e}"));
        let engine = Engine {
            query_manager,
            blob_manager,
            connection_manager: HttpConnectionManager::new(),
            runtime,
            responses_dir: data_dir.join("responses"),
            cancels: Mutex::new(HashMap::new()),
            shared_with_desktop,
            data_dir,
        };
        engine.seed_if_empty();
        engine
    }

    /// A fresh store gets Yaak's example workspace, so there is something to open and send.
    fn seed_if_empty(&self) {
        let empty = self.db().list_workspaces().map(|w| w.is_empty()).unwrap_or(false);
        if empty {
            match yaak_models::example::create_example_workspace(
                &self.query_manager,
                &update_source(),
            ) {
                Ok(_) => info!("created the example workspace"),
                Err(e) => warn!("cannot create the example workspace: {e}"),
            }
        }
    }

    /// A connection for reads.
    pub fn db(&self) -> ClientDb<'_> {
        self.query_manager.connect()
    }

    /// The store, for a write transaction (`with_tx`).
    pub fn query_manager(&self) -> &QueryManager {
        &self.query_manager
    }

    /// Send `request_id` with `environment_id` active, on the engine's runtime, and resolve
    /// with the outcome where the caller awaits it (the UI thread, under `day::task`).
    pub async fn send(&'static self, request_id: String, environment_id: Option<String>) -> Sent {
        let (cancel_tx, cancel_rx) = watch::channel(false);
        self.cancel_switches().insert(request_id.clone(), cancel_tx);
        let id = request_id.clone();
        // A blocking job drives the runtime for this one send: `block_on` needs no `Send`
        // bound on the future, which the pipeline's borrowed parameters would not meet.
        let outcome = day::spawn_blocking(move || {
            self.runtime.block_on(self.send_inner(&id, environment_id, cancel_rx))
        })
        .await;
        self.cancel_switches().remove(&request_id);
        outcome.unwrap_or_else(|e| Sent::failed(format!("the send did not complete: {e}")))
    }

    /// Stop the send in flight for `request_id`, if there is one. The send resolves as usual,
    /// with the cancellation as its error.
    pub fn cancel(&self, request_id: &str) {
        if let Some(switch) = self.cancel_switches().get(request_id) {
            let _ = switch.send(true);
        }
    }

    fn cancel_switches(&self) -> std::sync::MutexGuard<'_, HashMap<String, watch::Sender<bool>>> {
        // A panic while holding the lock leaves the map itself intact.
        self.cancels.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// One stored response as the UI shows it, read off the UI thread: the body file can be
    /// hundreds of kilobytes. `None` asks for the request's latest.
    pub async fn response_async(
        &'static self,
        request_id: String,
        response_id: Option<String>,
    ) -> Option<Sent> {
        day::spawn_blocking(move || match response_id {
            Some(id) => self.db().get_http_response(&id).ok().map(|r| self.sent(&r)),
            None => self.last_response(&request_id),
        })
        .await
        .ok()
        .flatten()
    }

    /// The request's past responses, newest first, as the history picker lists them.
    pub fn history(&self, request_id: &str) -> Vec<ResponseSummary> {
        self.db()
            .list_http_responses_for_request(request_id, Some(HISTORY_LIMIT))
            .unwrap_or_default()
            .iter()
            .map(ResponseSummary::from_response)
            .collect()
    }

    /// Forget every stored response of a request, with their body files.
    pub fn clear_history(&self, request_id: &str) {
        let responses =
            self.db().list_http_responses_for_request(request_id, None).unwrap_or_default();
        for response in responses {
            let deleted = self.query_manager.with_tx(|tx| {
                tx.delete_http_response(&response, &update_source(), &self.blob_manager)
            });
            if let Err(e) = deleted {
                warn!("cannot delete a response: {e}");
            }
        }
    }

    /// A stored response with its body and timeline read back.
    fn sent(&self, response: &HttpResponse) -> Sent {
        let mut sent = Sent::from_response(response);
        sent.timeline = self
            .db()
            .list_http_response_events(&response.id)
            .unwrap_or_default()
            .iter()
            .map(|e| TimelineRow::from_event(&e.event))
            .collect();
        sent
    }

    async fn send_inner(
        &self,
        request_id: &str,
        environment_id: Option<String>,
        cancel_rx: watch::Receiver<bool>,
    ) -> Sent {
        let request = match self.db().get_http_request(request_id) {
            Ok(r) => r,
            Err(e) => return Sent::failed(format!("cannot load the request: {e}")),
        };
        if let Err(e) = std::fs::create_dir_all(&self.responses_dir) {
            return Sent::failed(format!("cannot create {}: {e}", self.responses_dir.display()));
        }
        // The jar the workspace uses, as the desktop picks it: the first one.
        let cookie_jar_id = self
            .db()
            .list_cookie_jars(&request.workspace_id)
            .ok()
            .and_then(|jars| jars.into_iter().next())
            .map(|jar| jar.id);
        let mut cookie_jar =
            match yaak::send::load_cookie_jar(&self.query_manager, cookie_jar_id.as_deref()) {
                Ok(jar) => jar,
                Err(e) => return Sent::failed(e.to_string()),
            };
        let inputs = match resolve_send_inputs(
            &self.query_manager,
            &request,
            environment_id.as_deref(),
            cookie_jar.as_ref().map(|jar| jar.cookies.clone()),
        ) {
            Ok(inputs) => inputs,
            Err(e) => return Sent::failed(e.to_string()),
        };
        let cookie_store = inputs.cookie_store.clone();
        let executor = Executor {
            connection_manager: &self.connection_manager,
            runtime_config: inputs.runtime_config.clone(),
            cancel_rx: cancel_rx.clone(),
        };
        // What the store held before, to tell this send's own failure record from an older one.
        let before = self.latest_response_id(request_id);
        let result = send_http_request(SendHttpRequestParams {
            inputs,
            template_callback: &NoPlugins,
            storage: Some(ResponseStorage {
                query_manager: &self.query_manager,
                blob_manager: &self.blob_manager,
                update_source: update_source(),
                response_dir: &self.responses_dir,
            }),
            emit_events_to: None,
            emit_response_body_chunks_to: None,
            cancelled_rx: Some(cancel_rx),
            existing_response: None,
            prepare_sendable_request: Some(&NativeAuth),
            executor: &executor,
        })
        .await;
        if let Err(e) = yaak::send::persist_cookies_after_send(
            &self.query_manager,
            cookie_jar.as_mut(),
            cookie_store.as_ref(),
        ) {
            warn!("cannot persist cookies: {e}");
        }
        match result {
            Ok(result) => self.sent(&result.response),
            Err(e) => {
                // A send that failed on the wire left a response behind, with the timeline up
                // to the failure; one that failed before that (a template, a missing file) did
                // not.
                let stored = self
                    .db()
                    .list_http_responses_for_request(request_id, Some(1))
                    .ok()
                    .and_then(|r| r.into_iter().next())
                    .filter(|r| Some(&r.id) != before.as_ref());
                match stored {
                    Some(response) => {
                        let mut sent = self.sent(&response);
                        sent.error.get_or_insert(e.to_string());
                        sent
                    }
                    None => Sent::failed(e.to_string()),
                }
            }
        }
    }

    fn latest_response_id(&self, request_id: &str) -> Option<String> {
        self.db()
            .list_http_responses_for_request(request_id, Some(1))
            .ok()
            .and_then(|r| r.into_iter().next())
            .map(|r| r.id)
    }

    /// The last response stored for a request, as the UI shows one.
    pub fn last_response(&self, request_id: &str) -> Option<Sent> {
        self.db()
            .list_http_responses_for_request(request_id, Some(1))
            .ok()
            .and_then(|r| r.into_iter().next())
            .map(|r| self.sent(&r))
    }
}

/// The desktop app's store when one exists under the platform's application-data directory
/// (`<platform app data>/<id>/db.sqlite`, where the `yaak` CLI looks too), else this client's
/// own. A phone's sandbox holds no other application's data and takes the fallback.
fn choose_data_dir() -> (PathBuf, bool) {
    let own =
        day_part_fs::data_dir().map(|d| d.join(OWN_DIR)).unwrap_or_else(|_| PathBuf::from(OWN_DIR));
    // A host that names the data directory (a scripted run, CI) gets a store of its own: a
    // walkthrough must never edit the desktop app's workspaces.
    let redirected = std::env::var_os("DAY_DATA_DIR").is_some_and(|d| !d.is_empty());
    if !redirected
        && cfg!(any(target_os = "macos", target_os = "linux", target_os = "windows"))
        && let Ok(root) = day_part_fs::platform_data_dir()
    {
        let desktop = root.join(DESKTOP_APP_ID);
        if desktop.join("db.sqlite").is_file() {
            return (desktop, true);
        }
    }
    (own, false)
}

/// The template callback of a client with no plugin runtime: variables render, plugin
/// functions say why they cannot.
struct NoPlugins;

impl TemplateCallback for NoPlugins {
    fn run(
        &self,
        fn_name: &str,
        _args: std::collections::HashMap<String, serde_json::Value>,
    ) -> impl Future<Output = yaak_templates::error::Result<String>> + Send {
        let name = fn_name.to_owned();
        async move {
            Err(yaak_templates::error::Error::RenderError(format!(
                "{name}() needs Yaak's plugin runtime, which this client does not run"
            )))
        }
    }

    fn transform_arg(
        &self,
        _fn_name: &str,
        _arg_name: &str,
        arg_value: &str,
    ) -> yaak_templates::error::Result<String> {
        Ok(arg_value.to_string())
    }
}

/// The authentication types this client applies itself, as the request stores them.
pub const NATIVE_AUTH_TYPES: [&str; 3] = ["basic", "bearer", "apikey"];

/// The pipeline's authentication hook, without a plugin runtime behind it.
struct NativeAuth;

#[async_trait]
impl PrepareSendableRequest for NativeAuth {
    async fn prepare_sendable_request(
        &self,
        rendered_request: &HttpRequest,
        _auth_context_id: &str,
        sendable_request: &mut SendableHttpRequest,
    ) -> Result<(), String> {
        apply_native_auth(
            rendered_request.authentication_type.as_deref(),
            &rendered_request.authentication,
            sendable_request,
        )
    }
}

/// Apply a request's (already inherited and rendered) authentication to what goes on the wire.
/// Each arm follows the Yaak plugin of the same name (`plugins/auth-basic`, `auth-bearer`,
/// `auth-apikey`), including the defaults the plugin runtime fills in for a value never set.
fn apply_native_auth(
    kind: Option<&str>,
    values: &BTreeMap<String, serde_json::Value>,
    sendable: &mut SendableHttpRequest,
) -> Result<(), String> {
    let Some(kind) = kind.filter(|k| *k != "none") else {
        return Ok(());
    };
    // The editor's "enabled" switch, stored inverted beside the values.
    if values.get("disabled").and_then(|v| v.as_bool()) == Some(true) {
        return Ok(());
    }
    let text = |key: &str, default: &str| match values.get(key) {
        None | Some(serde_json::Value::Null) => default.to_string(),
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    };
    match kind {
        "basic" => {
            let pair = format!("{}:{}", text("username", ""), text("password", ""));
            let encoded = base64::engine::general_purpose::STANDARD.encode(pair);
            sendable.insert_header(("Authorization".into(), format!("Basic {encoded}")));
        }
        "bearer" => {
            let (prefix, token) = (text("prefix", "Bearer"), text("token", ""));
            let value = format!("{} {}", prefix.trim(), token.trim()).trim().to_string();
            sendable.insert_header(("Authorization".into(), value));
        }
        "apikey" => {
            let (key, value) = (text("key", ""), text("value", ""));
            if key.is_empty() {
                return Ok(());
            }
            if text("location", "header") == "query" {
                sendable.url = append_query_params(&sendable.url, vec![(key, value)]);
            } else {
                sendable.insert_header((key, value));
            }
        }
        other => {
            return Err(format!(
                "{other} authentication needs Yaak's plugin runtime, which this client does not run"
            ));
        }
    }
    Ok(())
}

/// The send executor over Yaak's connection manager: the one the desktop and the CLI use
/// (`yaak::send`'s own, which is private to that crate), transcribed.
struct Executor<'a> {
    connection_manager: &'a HttpConnectionManager,
    runtime_config: HttpSendRuntimeConfig,
    cancel_rx: watch::Receiver<bool>,
}

#[async_trait]
impl SendRequestExecutor for Executor<'_> {
    async fn send(
        &self,
        sendable_request: SendableHttpRequest,
        event_tx: mpsc::Sender<HttpResponseEvent>,
        cookie_behavior: CookieBehavior,
    ) -> yaak_http::error::Result<SenderHttpResponse> {
        let runtime_config = &self.runtime_config;
        let client_certificate =
            find_client_certificate(&sendable_request.url, &runtime_config.client_certificates);
        let cached_client = self
            .connection_manager
            .get_client(&HttpConnectionOptions {
                id: "day".to_string(),
                validate_certificates: runtime_config.settings.validate_certificates.value,
                http_version: runtime_config.settings.http_version.value,
                proxy: runtime_config.proxy.clone(),
                client_certificate,
                dns_overrides: runtime_config.dns_overrides.clone(),
                address_filter: None,
            })
            .await?;
        cached_client.resolver.set_event_sender(Some(event_tx.clone())).await;
        let sender = ReqwestSender::with_client(cached_client.client);
        let transaction = match cookie_behavior.store {
            Some(store) => HttpTransaction::with_cookie_behavior(
                sender,
                store,
                cookie_behavior.send_cookies,
                cookie_behavior.store_cookies,
            ),
            None => HttpTransaction::new(sender),
        };
        let result = transaction
            .execute_with_cancellation(sendable_request, self.cancel_rx.clone(), event_tx)
            .await;
        cached_client.resolver.set_event_sender(None).await;
        result
    }
}

/// One line of a response's timeline, in curl's notation: `>` sent, `<` received, `*` a note.
#[derive(Clone, Debug, PartialEq)]
pub struct TimelineRow {
    pub mark: char,
    pub text: String,
}

impl TimelineRow {
    fn from_event(event: &HttpResponseEventData) -> TimelineRow {
        use HttpResponseEventData as E;
        let (mark, text) = match event {
            E::SendUrl { method, path, query, fragment, .. } => {
                let query = if query.is_empty() { String::new() } else { format!("?{query}") };
                let fragment =
                    if fragment.is_empty() { String::new() } else { format!("#{fragment}") };
                ('>', format!("{method} {path}{query}{fragment}"))
            }
            E::ReceiveUrl { version, status } => ('<', format!("{version} {status}")),
            E::HeaderUp { name, value } => ('>', format!("{name}: {value}")),
            E::HeaderDown { name, value } => ('<', format!("{name}: {value}")),
            E::Redirect { url, status, .. } => ('*', format!("Redirect {status} → {url}")),
            E::Setting { name, value, .. } => ('*', format!("Setting {name}={value}")),
            E::Info { message } => ('*', message.clone()),
            E::ChunkSent { bytes } => ('*', format!("[{} sent]", size_text(*bytes as u64))),
            E::ChunkReceived { bytes } => ('*', format!("[{} received]", size_text(*bytes as u64))),
            E::DnsResolved { hostname, addresses, duration, overridden } => {
                let addresses = addresses.join(", ");
                if *overridden {
                    ('*', format!("DNS override {hostname} → {addresses}"))
                } else {
                    ('*', format!("DNS resolved {hostname} to {addresses} ({duration} ms)"))
                }
            }
        };
        TimelineRow { mark, text }
    }
}

/// A byte count as the UI writes one.
pub fn size_text(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// When a stored model was created, as a local clock time.
fn clock_text(created_at: &chrono::NaiveDateTime) -> String {
    use chrono::TimeZone;
    chrono::Local.from_utc_datetime(created_at).format("%H:%M:%S").to_string()
}

/// One past response, as the history picker names it.
#[derive(Clone, Debug, PartialEq)]
pub struct ResponseSummary {
    pub id: String,
    pub status: i32,
    pub status_reason: String,
    pub failed: bool,
    pub elapsed_ms: i32,
    /// The local clock time it was sent at.
    pub sent_at: String,
}

impl ResponseSummary {
    fn from_response(r: &HttpResponse) -> ResponseSummary {
        ResponseSummary {
            id: r.id.clone(),
            status: r.status,
            status_reason: r.status_reason.clone().unwrap_or_default(),
            failed: r.error.is_some() || r.status == 0,
            elapsed_ms: r.elapsed,
            sent_at: clock_text(&r.created_at),
        }
    }
}

/// A response as the UI shows it: the stored model's facts, plus its body read back.
#[derive(Clone, Debug, PartialEq)]
pub struct Sent {
    /// The stored response's id; empty for a send that failed before one was stored.
    pub id: String,
    pub status: i32,
    pub status_reason: String,
    pub elapsed_ms: i32,
    /// Time to the response headers, and the share of it spent resolving the host.
    pub elapsed_headers_ms: i32,
    pub elapsed_dns_ms: i32,
    pub url: String,
    pub version: String,
    pub remote_addr: String,
    pub headers: Vec<(String, String)>,
    /// The headers as they went out, with authentication and cookies applied.
    pub request_headers: Vec<(String, String)>,
    pub timeline: Vec<TimelineRow>,
    pub error: Option<String>,
    pub content_type: Option<String>,
    /// The body as text, pretty-printed when it is JSON; empty for a binary body.
    pub body: String,
    pub body_len: u64,
    pub body_truncated: bool,
}

impl Sent {
    fn failed(error: String) -> Sent {
        Sent {
            id: String::new(),
            status: 0,
            status_reason: String::new(),
            elapsed_ms: 0,
            elapsed_headers_ms: 0,
            elapsed_dns_ms: 0,
            url: String::new(),
            version: String::new(),
            remote_addr: String::new(),
            headers: Vec::new(),
            request_headers: Vec::new(),
            timeline: Vec::new(),
            error: Some(error),
            content_type: None,
            body: String::new(),
            body_len: 0,
            body_truncated: false,
        }
    }

    fn from_response(r: &HttpResponse) -> Sent {
        let content_type = r
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("content-type"))
            .map(|h| h.value.clone());
        let (body, body_len, body_truncated) = match &r.body_path {
            Some(path) => read_body(Path::new(path), content_type.as_deref()),
            None => (String::new(), 0, false),
        };
        let pairs = |headers: &[yaak_models::models::HttpResponseHeader]| {
            headers.iter().map(|h| (h.name.clone(), h.value.clone())).collect::<Vec<_>>()
        };
        Sent {
            id: r.id.clone(),
            status: r.status,
            status_reason: r.status_reason.clone().unwrap_or_default(),
            elapsed_ms: r.elapsed,
            elapsed_headers_ms: r.elapsed_headers,
            elapsed_dns_ms: r.elapsed_dns,
            url: r.url.clone(),
            version: r.version.clone().unwrap_or_default(),
            remote_addr: r.remote_addr.clone().unwrap_or_default(),
            headers: pairs(&r.headers),
            request_headers: pairs(&r.request_headers),
            timeline: Vec::new(),
            error: r.error.clone(),
            content_type,
            body,
            body_len,
            body_truncated,
        }
    }

    /// Whether the response came back at all (as opposed to a transport error).
    pub fn ok(&self) -> bool {
        self.error.is_none() && self.status > 0
    }
}

fn read_body(path: &Path, content_type: Option<&str>) -> (String, u64, bool) {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return (String::new(), 0, false),
    };
    let len = bytes.len() as u64;
    let truncated = bytes.len() > MAX_BODY_DISPLAY;
    let shown = &bytes[..bytes.len().min(MAX_BODY_DISPLAY)];
    let Ok(text) = std::str::from_utf8(shown) else {
        return (String::new(), len, truncated);
    };
    let is_json = content_type.is_some_and(|t| t.contains("json"));
    if is_json
        && !truncated
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(text)
        && let Ok(pretty) = serde_json::to_string_pretty(&value)
    {
        return (pretty, len, false);
    }
    (text.to_string(), len, truncated)
}

/// The request as the editor holds it, and the columns it writes back.
pub fn blank_request(
    workspace_id: &str,
    folder_id: Option<String>,
    sort_priority: f64,
) -> HttpRequest {
    HttpRequest {
        workspace_id: workspace_id.to_string(),
        folder_id,
        method: "GET".to_string(),
        name: String::new(),
        sort_priority,
        ..HttpRequest::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(url: &str) -> SendableHttpRequest {
        SendableHttpRequest {
            url: url.to_string(),
            method: "GET".to_string(),
            headers: vec![("Accept".to_string(), "*/*".to_string())],
            body: None,
            options: Default::default(),
        }
    }

    fn values(v: serde_json::Value) -> BTreeMap<String, serde_json::Value> {
        serde_json::from_value(v).unwrap()
    }

    fn header<'a>(r: &'a SendableHttpRequest, name: &str) -> Option<&'a str> {
        r.headers.iter().find(|h| h.0.eq_ignore_ascii_case(name)).map(|h| h.1.as_str())
    }

    #[test]
    fn basic_encodes_the_pair_and_tolerates_missing_values() {
        let mut r = request("https://example.com");
        let v = values(json!({"username": "aladdin", "password": "open sesame"}));
        apply_native_auth(Some("basic"), &v, &mut r).unwrap();
        assert_eq!(header(&r, "authorization"), Some("Basic YWxhZGRpbjpvcGVuIHNlc2FtZQ=="));

        let mut r = request("https://example.com");
        apply_native_auth(Some("basic"), &values(json!({})), &mut r).unwrap();
        assert_eq!(header(&r, "authorization"), Some("Basic Og=="));
    }

    #[test]
    fn bearer_defaults_its_prefix_and_honors_an_emptied_one() {
        let mut r = request("https://example.com");
        apply_native_auth(Some("bearer"), &values(json!({"token": " abc "})), &mut r).unwrap();
        assert_eq!(header(&r, "authorization"), Some("Bearer abc"));

        let mut r = request("https://example.com");
        let v = values(json!({"token": "abc", "prefix": ""}));
        apply_native_auth(Some("bearer"), &v, &mut r).unwrap();
        assert_eq!(header(&r, "authorization"), Some("abc"));

        let mut r = request("https://example.com");
        let v = values(json!({"token": "abc", "prefix": "Token"}));
        apply_native_auth(Some("bearer"), &v, &mut r).unwrap();
        assert_eq!(header(&r, "authorization"), Some("Token abc"));
    }

    #[test]
    fn an_api_key_goes_in_a_header_or_the_query() {
        let mut r = request("https://example.com/a?x=1");
        let v = values(json!({"key": "X-Api-Key", "value": "s3cret"}));
        apply_native_auth(Some("apikey"), &v, &mut r).unwrap();
        assert_eq!(header(&r, "x-api-key"), Some("s3cret"));
        assert_eq!(r.url, "https://example.com/a?x=1");

        let mut r = request("https://example.com/a?x=1");
        let v = values(json!({"location": "query", "key": "api key", "value": "a&b"}));
        apply_native_auth(Some("apikey"), &v, &mut r).unwrap();
        assert_eq!(r.url, "https://example.com/a?x=1&api%20key=a%26b");
        assert_eq!(r.headers.len(), 1);
    }

    #[test]
    fn nothing_is_applied_for_none_inherited_nothing_or_a_disabled_type() {
        for (kind, v) in [
            (None, json!({"token": "abc"})),
            (Some("none"), json!({"token": "abc"})),
            (Some("bearer"), json!({"token": "abc", "disabled": true})),
            (Some("apikey"), json!({"value": "abc"})),
        ] {
            let mut r = request("https://example.com");
            apply_native_auth(kind, &values(v), &mut r).unwrap();
            assert_eq!(r.headers.len(), 1, "{kind:?}");
            assert_eq!(r.url, "https://example.com");
        }
    }

    #[test]
    fn a_plugin_only_type_is_refused_by_name() {
        let mut r = request("https://example.com");
        let error = apply_native_auth(Some("oauth2"), &values(json!({})), &mut r).unwrap_err();
        assert!(error.starts_with("oauth2 authentication needs"), "{error}");
        assert_eq!(r.headers.len(), 1);
    }

    #[test]
    fn timeline_rows_read_like_curl() {
        use HttpResponseEventData as E;
        let row = |e| TimelineRow::from_event(&e);
        let sent = row(E::SendUrl {
            method: "GET".into(),
            scheme: "https".into(),
            username: String::new(),
            password: String::new(),
            host: "example.com".into(),
            port: 443,
            path: "/a".into(),
            query: "x=1".into(),
            fragment: String::new(),
        });
        assert_eq!((sent.mark, sent.text.as_str()), ('>', "GET /a?x=1"));
        let down = row(E::HeaderDown { name: "Server".into(), value: "day".into() });
        assert_eq!((down.mark, down.text.as_str()), ('<', "Server: day"));
        let chunk = row(E::ChunkReceived { bytes: 2048 });
        assert_eq!((chunk.mark, chunk.text.as_str()), ('*', "[2.0 KB received]"));
    }
}
