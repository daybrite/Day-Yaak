# Yaak for Day

Yaak's API client with its interface built from [Day](https://daybrite.dev)'s native pieces,
over the same Rust engine the desktop app and the CLI use. One codebase runs as a native app on
macOS, Linux, Windows, iOS, Android and HarmonyOS: a three-pane window on a desktop (workspaces,
requests, the open request with its response), three pushed layers on a phone.

This directory is additive to the Yaak repository: it is its own Cargo workspace and depends on
`../../crates/*` by path. Nothing under `crates/` changes for it.

## What it does

- Lists workspaces, and each workspace's HTTP requests with their folders.
- Edits a request: method, URL, name, headers, a JSON or text body, and the environment to send
  with. Edits write straight to Yaak's store as you type.
- Sends through Yaak's own pipeline (`yaak::send`): environment variables, inherited headers and
  authentication, cookies, redirects, client certificates and proxies all behave as in the
  desktop app. The response is stored like the desktop app's, and the last one shows beside the
  request.
- Creates and deletes requests.

Not yet: gRPC, WebSocket and SSE requests, folders as a tree, and anything that needs Yaak's
plugin runtime (a Node.js sidecar): template functions, plugin authentication types, importers.
A template that calls a plugin function says so when sent.

## The data

On a desktop with Yaak installed, the client opens the desktop app's own store
(`app.yaak.desktop/db.sqlite` under the platform's application-data directory), the way the
`yaak` CLI does, so it shows your real workspaces. Everywhere else, and when `DAY_DATA_DIR` is
set, it keeps a store of its own and seeds it with Yaak's example workspace on first launch.

Opening the desktop store applies this checkout's database migrations to it, as the CLI would:
keep the fork near the version of Yaak you run.

## Run it

```sh
day doctor
day launch -p macos-appkit                           # build + run
day launch -p ios-uikit --ios-simulator <simulator-id>
day launch -p android-mdc --android-device <serial>
day launch -p macos-appkit --script dayscript/demo.yaml   # the walkthrough, with screenshots
```

`dayscript/send-local.yaml` sends a real request to a server on this machine
(`python3 -m http.server 8765` in a directory holding `hello.json`).

## Layout

- `src/engine.rs` — Yaak's store and send pipeline, hosted: one tokio runtime on background
  threads, results back to the UI through `Setter`.
- `src/model.rs` — the per-window scene: selection, the editor's bindings, the last response.
- `src/ui.rs` — the request list, the editor and response, Settings.
- `src/lib.rs` — the window: the nav host and the menu bar.
- `dayscript/` — walkthroughs; `.github/workflows/day.yml` at the repository root runs
  `demo.yaml` on every target through the shared Day workflow.
- `website/` — the project website (daysite), which the same workflow publishes to GitHub Pages.
  Its About section is the fork's README at `.github/README.md`, which also tells how this
  directory was added to Yaak.
