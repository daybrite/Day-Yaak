# Yaak for Day

Yaak's API client with its interface built from [Day](https://daybrite.dev)'s native pieces,
over the same Rust engine the desktop app and the CLI use. One codebase runs as a native app on
macOS, Linux, Windows, iOS, Android and HarmonyOS: a three-pane window on a desktop (workspaces,
requests, the open request with its response), three pushed layers on a phone.

This directory is additive to the Yaak repository: it is its own Cargo workspace and depends on
`../../crates/*` by path. Nothing under `crates/` changes for it.

## What it does

- Lists workspaces, and each workspace's HTTP requests with their folders.
- Edits a request: method, URL, name and the environment to send with, then in tabs its body,
  URL parameters, headers, authentication and send settings. Edits write straight to Yaak's
  store as you type, and a request that is only opened stays exactly as stored.
- Bodies: JSON, XML and text in a highlighted editor, URL-encoded and multipart forms (a
  multipart field can upload a file), a GraphQL query with its variables, and a file as the
  whole body. Choosing a kind sets the Content-Type header, as Yaak's editor does.
- URL parameters: query pairs, and values for the `:name` placeholders in the URL's path.
- Authentication: Basic, Bearer token and API key (as a header or a query parameter), applied by
  this client with the semantics of Yaak's plugins of the same names. A request can also inherit
  its folder's or workspace's authentication, or opt out of it.
- Send settings per request: follow redirects, validate certificates, send and store cookies,
  HTTP version and timeout. Each inherits from the folder and the workspace unless set.
- Sends through Yaak's own pipeline (`yaak::send`): environment variables, inherited headers and
  authentication, cookies, redirects, client certificates and proxies all behave as in the
  desktop app. A send in flight can be cancelled.
- Shows the response: status, time and size, the body highlighted by its content type, the
  headers received and the headers sent, and a timeline of the send (settings, DNS, each header
  both ways, redirects). Responses are stored like the desktop app's, and a request's earlier
  ones stay in its history menu until cleared.
- Creates and deletes requests.

Not yet: gRPC, WebSocket and SSE requests, folders as a tree, and anything that needs Yaak's
plugin runtime (a Node.js sidecar): template functions, importers, and the authentication types
beyond the three above (OAuth, AWS, Digest, NTLM, JWT). A request that uses one of those keeps
it exactly as stored, and sending it reports that the type cannot be applied here. A template
that calls a plugin function says so when sent.

## The data

On a desktop with Yaak installed, the client opens the desktop app's own store
(`app.yaak.desktop/db.sqlite` under the platform's application-data directory), the way the
`yaak` CLI does, so it shows your real workspaces. Everywhere else, and when `DAY_DATA_DIR` is
set, it keeps a store of its own and seeds it with Yaak's example workspace on first launch.

Opening the desktop store applies this checkout's database migrations to it, as the CLI would:
keep the fork near the version of Yaak you run. Settings ▸ Data says which store is open and
where it lives.

## Run it

```sh
day doctor
day launch -t macos-appkit                           # build + run
day launch -t ios-uikit --ios-simulator <simulator-id>
day launch -t android-mdc --android-device <serial>
day launch -t macos-appkit --script dayscript/demo.yaml   # the walkthrough, with screenshots
```

`dayscript/send-local.yaml` sends real requests to a server on this machine and asserts on what
went over the wire: each authentication type, URL parameters, every body kind including a file
upload, a redirect with the setting off, a cancelled send, and the history. Start its server
first:

```sh
python3 dayscript/echo-server.py &
day launch -t macos-appkit --script dayscript/send-local.yaml
```

## Layout

- `src/engine.rs` — Yaak's store and send pipeline, hosted: one tokio runtime on background
  threads, results back to the UI through `Setter`. Also the three authentication types the
  client applies itself, through the pipeline's own hook, with their unit tests.
- `src/model.rs` — the per-window scene: selection, the editor's bindings and their write-back,
  the response on show and the request's history.
- `src/ui.rs` — the request list, the tabbed editor, the response's views, the Data group of
  Settings.
- `src/lib.rs` — the window: the nav host, the menu bar, and the Settings and About declaration.
- `dayscript/` — walkthroughs; `.github/workflows/day.yml` at the repository root runs
  `demo.yaml` on every target through the shared Day workflow. `echo-server.py` and
  `data/upload.txt` serve `send-local.yaml`.
- `website/` — the project website (daysite), which the same workflow publishes to GitHub Pages.
  Its About section is the fork's README at `.github/README.md`, which also tells how this
  directory was added to Yaak.


## Generated native projects

Use a Day CLI with automatic native project generation; Apple builds require **Xcode 27
or newer**. Xcode project documents, Android Gradle files, and HarmonyOS hvigor files are
created under `build/` by `day prepare`, `day build`, and `day launch`. Open the selected
project with `day open -t <target>`; edits to generated files are disposable. Apple Swift
sources, plists and `.xcconfig` files, and any Android resource overlays, remain app-owned
under `platform/`. For additional native configuration, use
`day project add-target <target> --persist-native-project` to create a persistent override.
