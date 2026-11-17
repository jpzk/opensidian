# Settings › Security — design (issue #144)

Status: design, written before any toggle is implemented (operator 2026-11-17).
Base: `develop` 23e83f0. Scope: release 0.9.6 (milestone 096).

## 0. Ground rules this design keeps

- **Defaults = today's behaviour.** A user who never opens the section sees no change.
  Every default below is the value that reproduces `develop` byte for byte
  (same CSP strings, same commands answering, same env).
- **App-wide, outside every vault.** All values live under ONE key, `"security"`, in
  `~/.opensidian.json`, written only through `cfg_update` (cfgstore lock; a confined
  window goes through the spawner like every other key). Nothing under `<vault>/` is
  ever read to decide a security value — so a foreign vault cannot ship a weaker one.
  A missing / malformed key reads as the defaults; an unknown field is ignored.
- **One reader.** `src-tauri/src/security.rs` (new) owns the struct, the defaults,
  the lockdown overlay and the precedence rules (3 overrides 1 and 2; 6 forces 5;
  12 forces everything). Every boundary asks `security::effective()`; nothing else
  parses the key. The UI reads the same struct through `security_get` and changes it
  only through `security_set` (one field per call, validated, written as an `Op::Set`
  of the whole merged object under the cfg lock).
- **Lowering a protection is asked natively.** Community plugin code shares the page
  and therefore the `security_set` command with the Settings pane. So a change that
  moves a field AWAY from its strict value (`security::weakens`: offline off, WebGL on,
  lockdown off, ...) first shows a native GTK confirm (`consent::Kind::Security`,
  title `opensidian: lower protection?`, default answer "Keep protection"), which page
  script cannot answer — the same mechanism as the trust prompt. Raising a protection
  is written at once. `cfgstore::apply` refuses every generic op naming `"security"`;
  only `Op::SecuritySet(field, bool)` with a field `security::validate` knows writes it.
- **Visible when off.** Every refusal carries the toggle's name in its error string
  (`"<surface> is off (Settings › Security › <Toggle name>)"`), and every UI surface
  that loses its content shows a placeholder with the same sentence.
- **Plugins can ask.** `app.opensidian.security` (read-only frozen snapshot, also
  `window.opensidianSecurity`) lists the effective values so a plugin can degrade
  instead of throwing. It is informational only, never an enforcement point.

### Stored shape (`~/.opensidian.json`)

```
"security": {
  "remoteMedia": false,        // 1  default off  (today: no remote media)
  "pluginNet": true,           // 2  default allow (operator decision)
  "offline": false,            // 3  default off
  "vaultMedia": true,          // 4  default on
  "webgl": true,               // 5  default on
  "canvas": true,              // 6  default on
  "jit": true,                 // 7  default on
  "pluginClipboard": true,     // 8  default on (stock parity, operator decision)
  "appClipboard": true,        // 9  default on
  "pluginSpawn": true,         // 10 default on (each run still asks)
  "mathjax": true, "prism": true, "mermaid": true,   // 11 default on
  "lockdown": false,           // 12 default off
  "lockdownClipboard": false   // 12 confirm checkbox "also turn off the clipboard"
}
```

Lockdown is an OVERLAY, not a rewrite: the stored per-toggle values are never
touched by turning lockdown on, so turning it off restores exactly the earlier
values (issue: "back to the value it had before, not to the default").
`effective()` = stored values, then the overlay, then the precedence rules.

Restart-bound values (7 JIT, 12 lockdown) are latched at process start:
`security::boot()` reads them once in `main()` before the webview exists; the
running process keeps the latched value, and the UI shows "applies after restart"
plus a "Restart now" button when the stored value differs from the latched one.

## 1. Toggle by toggle

Format per toggle: **boundary** (what actually refuses), **files**, **visible state**,
**T3 probe** (harness phase name `sec<N>-…` in `scripts/phases.d/`, listed in `scripts/t3.list`).

### 3 — Cut off all network (offline mode)  [implemented first: it is the choke point]

- **Boundary:** a backend choke point, `security::net_allowed(Net::<kind>, who)`,
  called at the top of EVERY outbound path the app has on develop:
  - `plugin_store::http_get` — the only HTTP client in the app (libsoup). Store list /
    detail / install / update, `builtins.rs` theme / snippet fetches and
    `updates.rs` (api.github.com) all reach the network only through it.
  - `desk_http::request` via `desk_http_request` (plugin `request` / `requestUrl`).
  - `desk_net_listen` (plugin `net.createServer`): a listener is a network surface too.
  - Page network: the page CSP (`page_csp` → `csp_by_trust`) — `connect-src`, `img-src`,
    `media-src` never carry a remote origin while offline (today they never do; offline
    additionally strips what toggle 1 would add).
  - `open_external` (system browser): **allowed, with a confirm** (operator decision):
    the UI asks "Offline mode is on. Open <host> in your browser?"; the backend command
    takes `confirmed: true` while offline and refuses without it, so a plugin calling
    `invoke('open_external')` directly cannot skip the dialog's intent marker silently
    (it is a plugin-reachable command; the refusal names the toggle).
  - A unit test (`every_outbound_path_goes_through_the_choke_point`) greps the source:
    any `soup::Session::new`, `TcpStream::connect`, `UdpSocket::bind`, `TcpListener::bind`
    outside the allow-listed functions, or an allow-listed function that does not call
    `security::net_allowed` first, fails T0 — the "any new network feature must go
    through the same choke point" rule as a test.
- **Precedence:** while on, 1 is forced off and 2 forced on (deny), shown greyed out.
- **Files:** `src-tauri/src/security.rs` (new), `plugin_store.rs`, `desk_http.rs` call
  site in `main.rs`, `desk_net_listen` in `main.rs`, `open_external`, `updates.rs` /
  store UI (`ui/main.js`: "Offline mode is on" state instead of a generic failure).
- **Visible:** error `"offline mode is on (Settings › Security › Offline mode)"`; the
  store pane and the update check show that sentence as their state.
- **T3 probe `secoffline`:** offline on → update check, store list, a builtin theme
  fetch, a probe plugin's `requestUrl`, `fetch`, `XMLHttpRequest`, `WebSocket` and
  `net.createServer` all refuse; the store pane shows "offline mode is on". Control:
  offline off → the same store list call reaches the network code (refusal string absent).

### 1 — Load external resources (remote images / audio / video)

- **Boundary:** the CSP plus the renderer. Off (default): CSP exactly as today (no remote
  origin), and the renderer's R29.10 branch (`image_html`, `ui/editor.js imgEl`) keeps
  minting the placeholder instead of a URL. On: the page CSP gains `https:` in `img-src`
  and `media-src` ONLY (no change to script/worker/connect/frame/font), and the renderer
  emits `<img>` / `<audio>` / `<video>` with the https URL (http:// stays a placeholder).
  Offline (3) forces it off.
- **Files:** `security.rs` (`csp_remote`, `PAGE_REMOTE`, `with_remote`), `main.rs`
  (`page_csp` applies `csp_remote` after `csp_by_trust`; `image_html` + `remote_html`;
  `render` / `render_view` / `render_blocks` run the renderer under `with_remote`),
  `ui/editor.js` (`imgEl` / `remoteEl`, live preview), `ui/security.js` (reload on
  disagreement), `ui/style.css`, `ui/main.js` (`[xr:]` census).
- **Renderer and CSP agree by construction (implementation decision):** layer 1 does
  not re-read the config per render; it follows `PAGE_REMOTE` = the value THIS page's
  CSP got at load. A change of the setting (or of offline / lockdown) makes
  `security_get.pageRemote` disagree with the effective value, and the pane reloads the
  page once (after the save debounce; two failed reloads stop with a warning), which
  re-runs `page_csp`. Another open window keeps its header until it reloads.
- **Visible:** the existing R29.4 sentence plus the reason when the setting is the cause:
  `“remote image” could not be found. (Settings › Security › Load external resources is off)`
  (reading view: `span.secwhy` text; live preview: `data-why`, painted by CSS — the row's
  text must stay its source line). The R29.10 rule "the URL never reaches the DOM"
  holds while off. On, an `http://` target says `… only https:// addresses load`.
- **T3 probe `secremote`:** a note with `![](https://<box-local https or unreachable>/x.png)`:
  off → placeholder naming the toggle, CSP header in `[csp]` log has no `https:`;
  on → CSP `img-src` contains `https:`, `<img src="https://…">` present, `script-src`
  unchanged.

### 2 — Cut off network for community plugins

- **Boundary:** backend checks on the plugin request commands — `desk_http_request`
  (requestUrl / request / http(s).request) and `desk_net_listen` (net/http.createServer)
  call `main.rs plugin_net_allowed` → `security::plugin_net_check` before anything is
  sent or bound. It asks the offline choke point first (offline wins and its refusal is
  the one shown), then this toggle. `fetch` / XHR / `WebSocket` to a remote host are
  refused by WebKit already: a page's `connect-src` is `ipc: http://ipc.localhost
  ['self']` (no remote origin) whatever the toggle says. T0
  `connect_src_never_names_a_remote_origin` pins that for both trust states × toggle 1;
  `every_outbound_path_goes_through_the_choke_point` pins that both commands ask
  `plugin_net_allowed(` before their network call. Core plugins do not use these
  commands (they are the community-plugin node shims).
- **Plugin name:** each plugin's view in `ui/plugins/node-desktop.js` hands out its own
  `obsidian.request/requestUrl` (per (api, plugin id)) and `net/http/https.createServer`
  that pass the plugin id. The error is
  `EPERM: requestUrl: network is off for community plugins (Settings › Security › Cut off network for all community plugins) — plugin <id>`
  (id sanitized to `[A-Za-z0-9._-]`, ≤ 64). The id is a label, not an authority: the
  refusal does not depend on it; `new net.Server()` and `http.request` stay unlabelled.
- **Default:** allow (operator decision) = today.
- **Files:** `security.rs` (`plugin_net_check`, `plugin_label`), `main.rs`
  (`plugin_net_allowed`, `desk_http_request`, `desk_net_listen`, T0),
  `ui/plugins/node-desktop.js` (label plumbing).
- **T3 probe `secplgnet`:** probe plugin calls `requestUrl('http://127.0.0.1:<port>')`
  against a box-local listener, `fetch`, XHR, `WebSocket`, `net.createServer`: off → all
  refuse, the requestUrl error names the plugin; on → requestUrl reaches the listener
  (control).

### 4 — Render vault media

- **Boundary:** backend + CSP. Off: the `opensidian-img:` scheme handler answers 403 for
  every image and PDF-page request, `media_read`, `pdf_info`, `audio_*` playback arming and
  canvas media reads refuse — so no bytes reach WebKit's image decoder, GStreamer or the
  PDF renderer (`pdf.rs` is never queued). CSP: `img-src` drops `opensidian-img:` (keeps
  `'self' data:` for app assets), `media-src` drops `blob:`.
  Vault media only (operator decision): theme / snippet CSS images are not covered.
- **Files:** `main.rs` (IMG_SCHEME handler, `media_read`, `pdf_info`, audio commands,
  `image_html` embed branch, `csp_by_trust`), `canvasfs.rs` if it serves media,
  `ui/editor.js`, `ui/main.js` (image / PDF / audio views, canvas media cards).
- **Visible:** each embed and each view shows `<file name> — media rendering is off
  (Settings › Security › Render media)`.
- **T3 probe `secmedia`:** vault with png, pdf, mp3, a canvas with a media card; off →
  direct `fetch('opensidian-img://localhost/a.png')` from a probe plugin fails, `media_read`
  and `pdf_info` refuse, embeds + views show the placeholder; on → the png embed decodes
  (naturalWidth > 0).

### 5 — WebGL

- **Boundary:** native. `WebKitSettings::set_enable_webgl(false)` on the page webview
  (via `with_webview` at window creation, and live from `security_set`). After that every
  `getContext("webgl"|"webgl2"|"experimental-webgl")` returns `null` for app and plugins.
  Live change (operator decision): the setting flips at once; the graph view re-opens on
  the new path immediately (`ui/main.js` graph teardown + reopen, it already falls back to
  Canvas 2D on "no webgl context"); plugins see it after a reload (existing contexts keep
  running). Android: `WebSettings` has no WebGL switch — documented as not available there.
- **Files:** `main.rs` (`page_window`, `security_set`), `security.rs`, `ui/main.js`
  (re-open the graph, census reports `[gl:2d]`).
- **T3 probe `secwebgl`:** off → probe plugin gets `null` for all three names, graph
  census reports the 2D path; on → `webgl` or `webgl2` is non-null (control).

### 10 — Community plugins may start programs

- **Boundary:** backend, before the consent dialog: `desk_proc_spawn` and `desk_shell`
  return `EPERM` naming the plugin when off; no native dialog is created (the check sits
  above `consent::ask`).
- **Files:** `main.rs` (`desk_proc_spawn`, `desk_shell`), `security.rs`.
- **T3 probe `secspawn`:** off → probe plugin's `child_process.spawn('true')` errors with
  EPERM + plugin id and no consent window appears (boxd window list); on → the consent
  dialog appears (control; denied by the phase).

### 8 + 9 — Clipboard for community plugins / app clipboard

The wrinkle: `clip_text` / `clip_has_text` / `clip_set_text` are called by the app
(`ui/edmenu.js`, `ui/editor.js` Ctrl+Shift+V) and are equally reachable by a plugin via
`window.__TAURI_INTERNALS__.invoke`. A plugin shares the page realm with the app.

**Decision: (a) the backend reads the calling page's REALM state. Not (b) an app-held
capability.**

Why not (b): any capability the app holds in the page realm is exercisable by code in the
same realm. A token minted by an init script and captured in a closure still has to leave
the closure through Tauri's IPC (`__TAURI_INTERNALS__.invoke` → `fetch`/`postMessage`
looked up at call time), which a plugin can wrap to read the token; and even a perfectly
sealed closure is driven by app UI (the edit-menu Copy row), which a plugin can drive
with a synthetic `click()`. Gating on `event.isTrusted` moves the check back into JS the
plugin shares prototypes with. A same-realm capability is a JS shim with extra steps —
the issue forbids exactly that.

What (a) means concretely:
- The backend already knows when community code enters a page: every plugin `main.js`
  is served by the `opensidian-plugin:` scheme handler (`plugins::serve` → `Served::Js`).
  That handler sets `REALM_HAS_PLUGIN_CODE` for the window; `page_csp` (which runs for
  each new HTML page of the window) clears it. A realm that has run plugin code is
  "tainted" until the page reloads.
- **Toggle 8 off:** `clip_*` refuse with `EPERM: the clipboard is off for community
  plugins (Settings › Security)` whenever the window's realm is tainted. In a realm with
  no community code (restricted vault, or no plugin enabled) the app's own calls keep
  working, because then the only caller is first-party code.
- What still works in a tainted realm with 8 off: **Ctrl+C / Ctrl+X / Ctrl+V** — these
  are WebKit's native editing commands firing trusted `copy`/`cut`/`paste` events
  (`ui/editor.js`), not `clip_*`. The edit-menu Copy/Cut rows switch to
  `document.execCommand('copy'|'cut')` inside the click (WebKit honours it only with a
  real user gesture); the menu Paste / Paste-as-plain rows are shown disabled with
  "Clipboard for plugins is off — use Ctrl+V", since a programmatic read is exactly the
  surface being cut.
- Residual (documented): a plugin in the same realm still sees the data of a paste the
  USER performs (it can listen to `paste` events) and can write with
  `execCommand('copy')` during a user gesture. The toggle removes the silent,
  gesture-less read/write of the system clipboard (pentest H2), which is what is
  reachable today.
- **Toggle 9 off:** `clip_*` refuse for every caller (`EPERM: the app clipboard is off
  (Settings › Security › App clipboard)`), and natively the clipboard key bindings are
  cut: a GTK `key-press-event` handler on the webview widget (runs before WebKit, page
  script cannot reach it) swallows Ctrl+C/X/V, Ctrl+Shift+V, Shift+Insert, Ctrl+Insert,
  Shift+Delete; `WebKitSettings::javascript-can-access-clipboard` is set false
  (execCommand copy/paste refuse); the edit-menu rows show disabled with the toggle name.
  The middle-click PRIMARY selection paste is also refused (button-press handler for
  button 2 on the webview).
- **Files:** `clip.rs` (gate), `main.rs` (plugin scheme handler taint, `page_csp` reset,
  `page_window` key handler + webkit setting), `security.rs`, `ui/edmenu.js`,
  `ui/editor.js` (Ctrl+Shift+V path surfaces the refusal).
- **T3 probe `secclip`** (extends the 989-pentest attacker plugin): 8 off → the plugin's
  `clip_text` / `clip_set_text` return EPERM, a native editor Ctrl+C then Ctrl+V of a
  marker still lands in the note (app works); 9 off → Ctrl+C/Ctrl+V of a marker does NOT
  land, menu rows disabled with the toggle name; control: both on → the pentest H2
  silent read succeeds (today's behaviour).

### 11 — MathJax / Prism / Mermaid

- **Boundary:** the asset response hook (`on_web_resource_request` in `page_window`)
  answers 403 for `/vendor/mathjax/*`, `/vendor/prism/*`, `/vendor/mermaid/*` while the
  engine's switch is off — the parser bytes never reach the page, whoever asks. The loader
  (`ui/plugins/api-functions.js` `loadMathJax` / `loadPrism` / `loadMermaid`) rejects first
  with an error naming the toggle (clear message; the 403 is the boundary).
- **State on develop:** `loadMathJax` and `loadPrism` exist (vendored); `loadMermaid` always
  rejects (Mermaid is not bundled); the app's own math / code / mermaid rendering is #185 /
  #186 / #187 and is not on develop. So the switch gates the two loaders that exist and is a
  **no-op-safe gate** for the rest: the 403 rule already covers `/vendor/mermaid/*` and any
  future first-party renderer must load through `loadScript`, which checks the switch. The
  PR says so.
- **Files:** `main.rs` (resource hook), `security.rs`, `ui/plugins/api-functions.js`.
- **T3 probe `secmath`:** all three off → probe plugin's `loadMathJax()`, `loadPrism()`,
  `loadMermaid()` reject naming the toggle, `window.MathJax` / `window.Prism` /
  `window.mermaid` undefined, a direct `<script src="/vendor/mathjax/…">` fires `error`;
  each engine checked with only its own switch off; on → `loadPrism()` resolves (control).

### 7 — JavaScript JIT

- **Boundary:** the process env before the webview exists. `security::boot()` in `main()`
  sets `JSC_useJIT=0` (plus `JSC_useDFGJIT=0`, `JSC_useFTLJIT=0`, `JSC_useBaselineJIT=0`
  for older JSC builds that read only the tier flags) when jit is off; the WebKit web
  process inherits the env. Restart-bound: the UI says "applies after restart" with
  "Restart now". Android: no WebView equivalent — documented.
- **Files:** `main.rs` (`main()` before the Builder), `security.rs`.
- **T3 probe `secjit`:** jit off, launch → `/proc/<WebKitWebProcess pid>/environ` contains
  `JSC_useJIT=0` (best-effort marker per the issue) and the app logs `[security] jit=off`;
  control: default launch has no `JSC_useJIT`.

### 6 — HTML canvas  (hardening, not a boundary against trusted plugins)

- **Boundary (stated limit):** WebKitGTK has no setting for `<canvas>`. Best available:
  a native user script (`WebKitUserContentManager`, `UserContentInjectedFrames::AllFrames`,
  `UserScriptInjectionTime::Start`, added in `page_window` via `with_webview`) that runs in
  every frame before any app / plugin code and replaces `HTMLCanvasElement.prototype.getContext`
  / `toDataURL` / `toBlob`, `OffscreenCanvas`, `OffscreenCanvasRenderingContext2D`,
  `CanvasRenderingContext2D.prototype.getImageData` with refusing stubs, defined
  non-writable + non-configurable. Canvas off forces WebGL (5) off (native).
- **Bypass routes:** fresh realm — `about:blank` / `srcdoc` iframes are frames, so the
  all-frames user script reaches them (closed; probed). Workers — the page CSP drops `blob:`
  from `worker-src` while canvas is off, and the worker realm gets no user script, so a
  same-origin worker (`'self'`) could still have `OffscreenCanvas`: documented as NOT
  closed for a trusted-vault plugin (which already has `require('fs')` / native helpers,
  stock parity). Against note content and app code it is a hard switch.
- **What shows "needs canvas":** graph view (both paths), PDF view / embeds (PDF pages
  are backend-rendered PNGs here, so PDFs keep working — noted), Canvas board
  "Export as image", paste re-encode in `canvas-drop.js`, text-measure helpers in
  `ui/main.js` / `ui/editor.js` get a no-canvas path (fallback width estimate).
- **Files:** `main.rs` (`page_window` user script), `security.rs`, `ui/main.js`,
  `ui/editor.js`, `ui/plugins/canvas-export.js`, `ui/plugins/canvas-drop.js`, CSP.
- **T3 probe `seccanvas`:** off → probe plugin `getContext('2d')` throws / null, prototype
  restore fails, a fresh `srcdoc` iframe's canvas also refuses, blob worker refused by CSP,
  graph shows "needs canvas", no uncaught page error; on → 2d context works (control).

### 12 — Lockdown mode (preset)

- **Boundary:** `security::effective()` overlay + the boundaries above + the trust check:
  `plugins::is_trusted` is wrapped so under lockdown every vault reads restricted
  (`plugins_trusted` entries stay on disk, untouched). CSP: the restricted header WITHOUT
  `'wasm-unsafe-eval'`. Forced: 1 off, 2 deny, 3 on, 4 off, 5 off, 6 off, 7 off, 8 off,
  10 off, 11 off; 9 only when the confirm's "also turn off the clipboard" was ticked.
  Foreign vaults open restricted with no extra prompt (operator decision).
- **UI:** a switch at the top of the section; confirm dialog lists what breaks, has the
  "also turn off the clipboard" checkbox, says a restart is needed and offers
  "Restart now". While on: a "Lockdown" status-bar item and " — Lockdown" in the window
  title; every toggle greyed with its forced value.
- **Restart-bound:** latched at start (JIT is read only there).
- **Files:** `security.rs`, `plugins.rs` (trust wrapper call sites in `main.rs`),
  `main.rs` (CSP, title), `ui/main.js` (status bar, dialog).
- **T3 probe `seclockdown`:** one launch with lockdown on runs every per-toggle probe
  (1–11) as a core-side probe (plugins do not run: probes run from the census/devui hook),
  each must report its surface off; the trusted vault boots `[plg:R]` (restricted); then lockdown off +
  restart → the vault boots trusted again and the earlier toggle values are intact.

## 2. Settings tab

- A **Security** entry in the settings left nav, after "Community plugins" in the
  Options group (stock 1.13.7 has no Security section; this is a recorded addition).
  Rows use the stock setting-item markup the pane already renders (`.setting-item`,
  `.setting-item-info` / `-name` / `-description`, `.setting-item-control` with the
  `.checkbox-container` toggle) — verified against STOCK's tab chrome via `box-run.sh stock`
  + cdp (nav item, heading, row, toggle geometry), not against a Security section.
- Order: Lockdown mode (top, with its own heading) · Network (3 offline, 1 external
  resources, 2 plugin network) · Media & rendering (4, 5, 6, 11 ×3) · Plugins & clipboard
  (10, 8, 9) · Engine (7). Forced rows are `aria-disabled` and show "(forced by …)".
- Storage is in `nav.tsv`-independent code (`settings.rs` overlay `SECURITY_ROWS`), so the
  Obsidian transcript oracle stays untouched.

## 3. T3 harness phases (opensidian-harness, `scripts/phases.d/`)

One phase per toggle (`secoffline secremote secplgnet secmedia secwebgl secspawn secclip
secmath secjit seccanvas`) + `seclockdown`, each listed in `scripts/t3.list`, each with a
toggle-off refusal assertion that greps the toggle's name in the visible placeholder / error
and an "on" control. Settings are written to the test HOME's `~/.opensidian.json` before
launch (the same file the UI writes). Existing pentest phases keep passing because the
defaults do not change.

## 4. Known bypasses / limits (copied to the PR)

- 6 HTML canvas: hardening against trusted plugins (same-origin worker realm, native
  helpers); hard against note content / app code.
- 8 plugin clipboard: user-initiated paste events and gesture-bound `execCommand('copy')`
  remain visible to same-realm plugin code.
- 5 / 7 on Android: no WebView switch; documented as desktop-only.
- 3 offline: the system browser (`open_external`) is allowed after a confirm.
