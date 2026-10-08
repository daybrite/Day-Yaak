//! Yaak's engine, hosted for Day.
//!
//! Yaak keeps its workspaces, requests and responses in SQLite through `yaak-models`, and
//! sends requests through `yaak`'s send pipeline, which is async over tokio. Day runs no
//! tokio, so this module owns one runtime on background threads and is the only place that
//! touches it (https://daybrite.dev/docs/async, rule 4). The database is read and written
//! synchronously, on whichever thread asks; a send runs on the runtime and reports back to the
//! UI through a `Setter`.
//!
//! The plugin runtime (a Node.js sidecar) is not started: template functions and plugin-provided
//! authentication are declined at render time with a clear message, and everything else in a
//! send, including environment variables, inheritance, cookies and redirects, is Yaak's own code.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use async_trait::async_trait;
use day::reactive::Setter;
use log::{info, warn};
use tokio::sync::{mpsc, watch};
use yaak::send::{
    CookieBehavior, HttpSendRuntimeConfig, ResponseStorage, SendHttpRequestParams,
    SendRequestExecutor, resolve_send_inputs, send_http_request,
};
use yaak_http::client::HttpConnectionOptions;
use yaak_http::manager::HttpConnectionManager;
use yaak_http::sender::{HttpResponse as SenderHttpResponse, HttpResponseEvent, ReqwestSender};
use yaak_http::transaction::HttpTransaction;
use yaak_http::types::SendableHttpRequest;
use yaak_models::blob_manager::BlobManager;
use yaak_models::client_db::ClientDb;
use yaak_models::models::{HttpRequest, HttpResponse};
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

pub struct Engine {
    query_manager: QueryManager,
    blob_manager: BlobManager,
    connection_manager: HttpConnectionManager,
    runtime: tokio::runtime::Runtime,
    responses_dir: PathBuf,
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

    /// Send `request_id` with `environment_id` active, on the engine's runtime; `done` receives
    /// the outcome on the UI thread.
    pub fn send(
        &'static self,
        request_id: String,
        environment_id: Option<String>,
        done: Setter<Option<Sent>>,
    ) {
        // A plain thread drives the runtime for this one send: `block_on` needs no `Send`
        // bound on the future, which the pipeline's borrowed parameters would not meet.
        std::thread::Builder::new()
            .name("yaak-send".into())
            .spawn(move || {
                let outcome = self.runtime.block_on(self.send_inner(&request_id, environment_id));
                done.set(Some(outcome));
            })
            .unwrap_or_else(|e| panic!("cannot start a send thread: {e}"));
    }

    async fn send_inner(&self, request_id: &str, environment_id: Option<String>) -> Sent {
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
        };
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
            cancelled_rx: None,
            existing_response: None,
            prepare_sendable_request: None,
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
            Ok(result) => Sent::from_response(&result.response),
            Err(e) => Sent::failed(e.to_string()),
        }
    }

    /// The last response stored for a request, as the UI shows one.
    pub fn last_response(&self, request_id: &str) -> Option<Sent> {
        self.db()
            .list_http_responses_for_request(request_id, Some(1))
            .ok()
            .and_then(|r| r.into_iter().next())
            .map(|r| Sent::from_response(&r))
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

/// The send executor over Yaak's connection manager: the one the desktop and the CLI use
/// (`yaak::send`'s own, which is private to that crate), transcribed.
struct Executor<'a> {
    connection_manager: &'a HttpConnectionManager,
    runtime_config: HttpSendRuntimeConfig,
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
        let (_cancel_tx, cancel_rx) = watch::channel(false);
        let result =
            transaction.execute_with_cancellation(sendable_request, cancel_rx, event_tx).await;
        cached_client.resolver.set_event_sender(None).await;
        result
    }
}

/// A response as the UI shows it: the stored model's facts, plus its body read back.
#[derive(Clone, Debug, PartialEq)]
pub struct Sent {
    pub status: i32,
    pub status_reason: String,
    pub elapsed_ms: i32,
    pub url: String,
    pub headers: Vec<(String, String)>,
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
            status: 0,
            status_reason: String::new(),
            elapsed_ms: 0,
            url: String::new(),
            headers: Vec::new(),
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
        Sent {
            status: r.status,
            status_reason: r.status_reason.clone().unwrap_or_default(),
            elapsed_ms: r.elapsed,
            url: r.url.clone(),
            headers: r.headers.iter().map(|h| (h.name.clone(), h.value.clone())).collect(),
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
