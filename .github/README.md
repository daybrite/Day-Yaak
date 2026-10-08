# Day-Yaak

A fork of [Yaak](https://github.com/mountain-loop/yaak), the desktop API client, that adds a
second front end built with [Day](https://daybrite.dev): native pieces on macOS, Linux, Windows,
iOS, Android and HarmonyOS, over the same Rust engine the Tauri app and the `yaak` CLI use.
Yaak's own code is unchanged. The fork adds one directory, `apps/yaak-day/`, and one workflow,
`.github/workflows/day.yml`; Yaak's README is still at the repository root.

The purpose is to show how an existing Tauri application takes on a Day interface without being
rewritten, and what that interface looks like on a phone as well as on a desktop. The Day client
is a subset of Yaak today (HTTP requests, no plugins; see
[apps/yaak-day/README.md](../apps/yaak-day/README.md) for the list), and it grows in the same
additive way.

## What the Day client does

- Lists workspaces and their HTTP requests, with folders.
- Edits a request's method, URL, name, headers, body and environment, writing to Yaak's store as
  you type.
- Sends through `yaak::send`, so variables, inherited headers and authentication, cookies,
  redirects, client certificates and proxies behave as in the desktop app, and the response is
  stored the same way.
- On a desktop with Yaak installed, opens the desktop app's own database, the way the `yaak` CLI
  does. On a phone it keeps a store of its own, seeded with Yaak's example workspace.

A desktop window shows three panes (workspaces, requests, the open request beside its response);
a phone shows the same three as pushed layers, with a Request/Response switch in the editor.

## Why Yaak lends itself to this

Yaak keeps its application logic in Tauri-free crates under `crates/`: `yaak-models` (the SQLite
store and its migrations), `yaak-http`, `yaak-templates`, `yaak-tls`, `yaak-lifecycle`, and `yaak`
itself, which holds the send pipeline. The Tauri shell under `crates-tauri/` and the CLI under
`crates-cli/` are both consumers of those crates. A third consumer can be added beside them, and
that is all the Day client is.

Two parts of Yaak stay with the Tauri app for now. The plugin runtime is a Node.js sidecar, so
template functions, plugin authentication and importers are not available in the Day client; a
template that calls one says so when sent. And `yaak::send`'s request executor is a private type,
so the Day client carries a forty-line transcription of it (`apps/yaak-day/src/engine.rs`);
making the type public upstream would remove the copy.

## How the Day project was added

The steps, in the order they happened, so another Tauri project can follow them.

1. **Scaffold a Day app inside the repository.** `day new app yaak-day` with the eleven targets,
   the app id and title, run under `apps/`. The scaffold is its own Cargo workspace (a
   `[workspace]` table in `apps/yaak-day/Cargo.toml`), so Yaak's root workspace, lockfile and
   CI know nothing of it, and it depends on Yaak's crates by path: `yaak = { path =
   "../../crates/yaak" }` and so on. Nothing under `crates/` changes.

2. **Host the engine.** Yaak's crates are tokio-bound. Day's rule for such a dependency is that
   it owns a runtime on background threads and hands results back to the main thread. The engine
   (`src/engine.rs`) opens the store with `yaak_models::init_standalone`, runs
   `yaak_lifecycle::on_launch`, keeps a two-worker tokio runtime, and sends a request on a thread
   that blocks on `send_http_request`; the result reaches the window through a `Setter`.

3. **Choose where the data lives.** `day_part_fs::platform_data_dir()` names the platform's
   application-data directory; Yaak desktop's store is `app.yaak.desktop/db.sqlite` under it.
   On a desktop the client opens that when it exists, otherwise, and on every phone, it opens a
   store of its own under Day's data directory. A host that sets `DAY_DATA_DIR` (a scripted run,
   CI) always gets the private store, so a walkthrough never edits real workspaces.

4. **Model the window.** `src/model.rs` holds one `Scene` per window: the selected workspace and
   request as signals, the editor's fields bound to the store, the header rows as a keyed store,
   the last response. Reading a request fills the fields; a `watch` on each field writes the
   store back through `yaak-models`' write transaction.

5. **Build the interface from pieces.** `src/ui.rs` and `src/lib.rs`: a `nav` host in sidebar
   style with the request list as its content list and the editor as its detail, a `list` of
   request rows with a context menu, pickers for method, environment and body kind, a text area
   for the body, labels for the response. One source; each platform's toolkit draws it. The
   editor reads `day::size_class()` to put request and response beside each other when the
   window is wide and behind a segmented picker when it is not.

6. **Localize and script.** The strings live in `resource/locales/en/app.ftl`; `dayscript/demo.yaml`
   drives the app by element id (open a request, rename it, create one, switch to the compact
   layout) and takes screenshots, and is the same test on every target.

7. **Add CI at the repository root.** `.github/workflows/day.yml` calls the shared
   `daybrite/actions` workflow with `project-path: apps/yaak-day`, which builds every target,
   runs the walkthrough, packs the artifacts and publishes the project website. Yaak's own
   workflows are untouched.

Two build facts were learned along the way and are now in Day's documentation. reqwest's
system-proxy support links Apple's `SystemConfiguration` framework through a Rust `#[link]`
attribute that does not survive the cargo-to-Xcode link, so `apps/yaak-day/Cargo.toml` declares
the framework under `[package.metadata.day.macos]` and `.ios`. And Yaak's `native-tls` needs
OpenSSL on Android, which no Android image carries, so the Android target builds a vendored copy.

## Building it

Everything below runs in `apps/yaak-day/`.

```sh
day doctor
day launch -p macos-appkit                             # build and run
day launch -p ios-uikit --ios-simulator <simulator-id>
day launch -p android-mdc --android-device <serial>
day launch -p macos-appkit --script dayscript/demo.yaml # the walkthrough, with screenshots
```

The desktop client reads the installed Yaak's database and applies this checkout's migrations to
it, as the CLI would. Keep the fork near the version of Yaak you run, or set `DAY_DATA_DIR` to
use a private store.

## Layout of the addition

```
.github/workflows/day.yml      the Day CI workflow (Yaak's own stay in place)
apps/yaak-day/
├── Cargo.toml                 its own workspace; path dependencies on ../../crates
├── Day.toml                   app id, title, targets
├── src/engine.rs              Yaak's store and send pipeline, hosted on a tokio runtime
├── src/model.rs               the per-window scene
├── src/ui.rs, src/lib.rs      the pieces and the window
├── resource/locales/          strings
├── dayscript/                 walkthroughs
├── platform/                  the generated host projects (Xcode, Gradle, HarmonyOS)
├── store/storefront.toml      the listing text the website and the stores show
└── website/site.toml          the project website, built by CI from this README
```

Yaak is MIT licensed; the Day client under `apps/yaak-day/` is part of this fork under the same
license.
