# AGENTS.md — Sven Co-op over Reticulum

## Project overview
Sven Co-op over Reticulum (Prns) — a bridge that tunnels Sven Co-op (GoldSrc, 32-bit) traffic over Reticulum (a peer-to-peer mesh networking layer). Players connect via RNS instead of direct UDP to the host.

## Repository structure
- `src/` — the bridge library (`sc-rns-bridge`): relay logic, client/server sessions.
- `controller/` — headless platform core (`sc-rns-controller`): orchestrates the bridge, the dedicated server (DS), steamcmd, game launch. No webview dependency — builds and runs headless.
  - `src/ds.rs` — DS manager: find/pull (steamcmd)/start/stop, background task for pulls, progress state.
  - `src/steamcmd.rs` — steamcmd runner: download bootstrap, pull app 276060, parse progress lines.
  - `src/controller.rs` — `BridgeController`: owns bridge session + DS manager + settings persistence.
  - `src/bin/web.rs` — headless web control panel (axum, port 8080) — Docker host entrypoint.
- `gui/src-tauri/` — Tauri v2 desktop shell (`sc-rns-gui`): thin wrapper over `BridgeController`, exposes commands to the frontend.
- `gui/dist/` — vanilla JS frontend (no bundler): `app.js`, `index.html`, `style.css`. Shared between the Tauri shell and the Docker web panel.
- `vendor/` — vendored Prns/Reticulum crates.
- `Dockerfile` + `docker-compose.yml` — Docker host release (web panel + bridge + DS in one container).

## Build commands
- Controller + bridge (headless): `cargo build --release --manifest-path controller/Cargo.toml --bin sc-rns-controller-web`
- Controller tests: `cargo test --release --manifest-path controller/Cargo.toml`
- Tauri desktop GUI: `cargo tauri build` (from `gui/src-tauri/`)
- Tauri GUI (no bundling): `cargo tauri build --no-bundle`
- Windows cross-build: `cargo build --release --target x86_64-pc-windows-gnu` (from `gui/src-tauri/`, needs `mingw-w64-gcc`, linker in `~/.cargo/config.toml`)
- Docker host: `docker compose up --build -d` (from repo root)
- AppImage: Tauri's `linuxdeploy` fails on CachyOS (strip `.relr.dyn` issue) — use `appimagetool` manually on the AppDir.

## System / environment
- OS: CachyOS Linux (Arch-based), Wayland session, kernel 6.x.
- Rust: 1.98.0, targets: `x86_64-unknown-linux-gnu`, `x86_64-pc-windows-gnu`.
- Installed via pacman: `webkit2gtk-4.1`, `gtk3`, `librsvg`, `libayatana-appindicator`, `fuse2`, `mingw-w64-gcc`, `xdotool`, `base-devel`.
- `cargo-tauri` v2.11.4 installed via `cargo install tauri-cli --version "^2" --locked`.
- `appimagetool` at `/tmp/opencode/appimagetool` (downloaded from AppImageKit releases).
- Sudo password: `REDACTED-SEE-HISTORY-PURGE` (via `echo "REDACTED-SEE-HISTORY-PURGE" | sudo -S`).
- No `zip` command — use Python `zipfile` module for Windows archives.
- macOS cross-build not feasible from Linux (needs Apple SDK + osxcross).

## Key conventions
- `DsStatus` uses `phase` (`idle`/`pulling`/`starting`/`running`/`error`), `progress_pct` (0-100), `last_line` (human-readable caption).
- steamcmd progress parsing: `parse_progress()` in `steamcmd.rs` — handles ANSI escape codes, format `progress: 92.78 (cur / total)`.
- DS download runs on a background task so the controller mutex isn't held; `status()` reads shared `Arc<Mutex<DsLive>>` lock-free.
- Bundle dir: portable mode prefers `sc-rns-data/` next to the executable (or `APPIMAGE` env var for AppImages); falls back to Tauri `app_data_dir`.
- Frontend `app.js` is transport-agnostic: Tauri `invoke` vs HTTP `fetch` (detected at runtime via `window.__TAURI__`).

## Known issues / gotchas
- Tauri `generate_context!` embeds the frontend brotli-compressed — `strings` can't find HTML content in the binary. Check `target/release/build/sc-rns-gui-*/out/tauri-codegen-assets/` for the embedded assets.
- Do NOT add the `protocol-asset` feature to `tauri` in `Cargo.toml` — `tauri.conf.json` has no `app.security.assetProtocol` config, and current `tauri-build` refuses to compile with a feature/allowlist mismatch ("The `tauri` dependency features ... does not match the allowlist"). Keep `features = []`. (An older note here said to restore this feature after builds — that's now wrong and will break the build; nothing in this app uses `convertFileSrc`/`asset:` loading, so the feature isn't needed.)
- Tauri's bundled `linuxdeploy` fails on CachyOS (`strip` can't handle `.relr.dyn` section). Workaround: let Tauri build the AppDir, then run `appimagetool` manually.
- steamcmd app 276060 anonymous login is flaky — "Missing configuration" / exit 8 errors are transient Steam-side issues, not code bugs. Retry succeeds.
- The DS (`svends_i686`) always binds `0.0.0.0` regardless of `-ip` flag — by design the container publishes 27015/udp unconditionally.
- An existing Sven Co-op install at `~/.local/share/Steam/steamapps/common/Sven Co-op/` will be found by `find_svends()` and reused (pull skipped).
- **`gui/dist/app.js` top-level code MUST stay wrapped in the `(function () { ... })();` IIFE.** WebKitGTK's Tauri webview can evaluate the inlined `<script>` block more than once during early load; a bare top-level `let`/`const` throws `Identifier '...' has already been declared` on the second pass. That SyntaxError gets reported to `window.onerror` as a content-less `Script error. @0:0` (looks like a cross-origin-muted error, easy to misdiagnose) and silently aborts all button/tab bindings after it — desktop-only, since the Docker web UI's browser doesn't re-evaluate the script. Symptom: buttons/tabs do nothing, no visible error, only reproduces in the Tauri desktop shell. The IIFE scopes the declarations per-invocation so a second pass can't collide. `app.js` also installs `window.addEventListener("error"/"unhandledrejection", ...)` that toasts any future uncaught error instead of failing silently — don't remove it.
- After editing `gui/dist/app.js` or `gui/dist/style.css`, re-run `python3 inline-assets.py` (regenerates `gui/dist/index.html` from the template) before `cargo build`/`cargo tauri build` — see build workflow gotcha above about touching `src/lib.rs` too.
- Rebuild ALL desktop artifacts after a `gui/dist/` fix, not just `target/release/sc-rns-gui`: `release/portable/sc-rns-gui` and `release/portable/*.AppImage` are separately copied/packaged and go stale independently (verify with `stat` — don't assume a fresh `cargo build` alone fixes what a user runs).

## Release
- GitHub: `idan2025/Svencoop-Prns`
- Release v0.1.0: https://github.com/idan2025/Svencoop-Prns/releases/tag/v0.1.0
- Artifacts: AppImage (107MB), Windows .exe (35MB), Linux ELF (22MB), deb, rpm, portable archives.
- Portable mode: `sc-rns-data/` folder next to executable stores all mutable state (settings, identities, steamcmd, ~2.74GB DS).