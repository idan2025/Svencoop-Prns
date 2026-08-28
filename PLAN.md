# PLAN — Fix Tauri Desktop GUI (DS download + tabs not working)

## Status: fixed and verified (2026-08-28)

## Root Cause (confirmed by reproduction, not just theory)
`gui/dist/app.js` declared its state at the top level of the inlined `<script>`
block with `let`/`const` (`let tauriInvoke`, `const isTauri`, `const $`, ...).
WebKitGTK's Tauri webview can evaluate that inline script more than once
during early page load. On the second pass, the `let`/`const` redeclaration
throws `SyntaxError: Identifier 'tauriInvoke' has already been declared`.

That SyntaxError gets reported to `window.onerror` as a content-free
`Script error. @0:0` — WebKit's generic muted-error message, which looks
exactly like a cross-origin/CSP issue and is very easy to misdiagnose as
"the script never loaded." In fact the script *did* load and *did* run once
successfully — the second (failed) evaluation is what leaves the page
half-wired: whichever click handlers happened to bind before the failing
re-run stay live, everything after it (in practice, all of it, since the
failure recurs at the very top of the file) never gets (re)bound.

This is desktop-only because the Docker/web-panel path serves `app.js` as an
external `<script src>` in a normal browser tab, which doesn't re-evaluate
already-loaded scripts the way the embedded WebKitGTK view apparently does
here.

**How this was actually diagnosed** (worth keeping for next time — `xdotool`
+ `import` screenshots was the only way to see anything, since the Tauri
window title doesn't sync from `document.title` and the WebKit remote
inspector port didn't respond on this build):
1. Ran the real built binary (`release/portable/sc-rns-gui`), not just
   re-read source — the running binary was stale relative to the working
   tree (`lib.rs` mtime was *after* the last build), so the first round of
   "it's still broken" was testing old code.
2. `GDK_BACKEND=x11 ./sc-rns-gui` forces the GTK/WebKit window onto XWayland
   so `xdotool` can find/click it and `import -window <id>` can screenshot
   it (this is a Wayland/KDE session; pure-Wayland clients are invisible to
   X11 tools).
3. Injected a visible on-page debug bar (`dbg()` appending to a fixed red
   `<div>`) at checkpoints through `app.js`, since `document.title` writes
   never reached the OS window titlebar and the WebKit inspector port
   (`WEBKIT_INSPECTOR_SERVER=127.0.0.1:9222`) accepted connections but
   returned empty replies (devtools not enabled in this release build).
4. Wrapping the whole script body in `try { ... } catch (e) { dbg(...) }`
   made the checkpoints run to completion with **no** caught exception —
   proving the failure was a *parse-time* error on a second evaluation
   (try/catch can't catch a syntax error in the same script, but an IIFE
   scoping the declarations prevents the redeclaration from being possible
   at all on a second pass).

## Fix
1. **`gui/dist/app.js`**: wrapped the entire file body in `(function () {
   ... })();` so top-level `let`/`const` can't collide across repeated
   evaluations. Added `window.addEventListener("error"/"unhandledrejection")`
   inside the IIFE that calls `toast(...)` — so *any* future uncaught JS
   error surfaces to the user instead of failing silently (this is the
   direct fix for "fails quietly").
2. `gui/dist/index.html` is regenerated from `app.js`/`style.css` via
   `python3 inline-assets.py` — no template changes needed, the IIFE fix
   lives in `app.js` and flows through automatically.
3. Rebuilt and reproduced the fix end-to-end against the actual shipped
   artifacts, not just `cargo build` output:
   - `gui/src-tauri/target/release/sc-rns-gui`
   - `release/portable/sc-rns-gui`
   - `release/portable/Sven_Co-op_over_Reticulum_0.1.0_portable_x86_64.AppImage`
     (rebuilt via `cargo tauri build` + manual `appimagetool`, since
     `linuxdeploy` still fails on CachyOS — see AGENTS.md)
   Verified via screenshots: all 4 tabs switch, DS "Start / pull" reaches
   `ds_start` → steamcmd pull begins, progress bar renders.

## Secondary issue found and reverted
The working tree had `tauri = { version = "2", features = ["protocol-asset"] }`
in `gui/src-tauri/Cargo.toml` (restoring it per an old AGENTS.md note). The
current `tauri-build` now hard-fails the build if a feature has no matching
`app.security.assetProtocol` entry in `tauri.conf.json` (which this app has
never had — nothing here uses `convertFileSrc`/`asset:`). Reverted to
`features = []` and corrected the stale AGENTS.md note.

## Build Procedure
```
cd gui/src-tauri
python3 inline-assets.py          # only needed if app.js/style.css changed
touch src/lib.rs                  # force asset re-embed if dist/ changed but no .rs changed
cargo build --release             # quick iteration / raw ELF testing
# --- for full release artifacts ---
cargo tauri build                 # linuxdeploy fails on CachyOS, AppDir is still produced
cp "target/release/bundle/appimage/Sven Co-op over Reticulum.AppDir/usr/share/icons/hicolor/256x256@2/apps/sc-rns-gui.png" \
   "target/release/bundle/appimage/Sven Co-op over Reticulum.AppDir/sc-rns-gui.png"
appimagetool "target/release/bundle/appimage/Sven Co-op over Reticulum.AppDir" \
   ../../release/portable/Sven_Co-op_over_Reticulum_0.1.0_portable_x86_64.AppImage
cp target/release/sc-rns-gui ../../release/portable/sc-rns-gui
```

## Testing without a visible desktop / screen-share
`GDK_BACKEND=x11 ./sc-rns-gui` + `xdotool` (click/move) + `import -window
<id>` (screenshot) is the reliable loop on this KDE Plasma Wayland session.
`document.title` does not sync to the Tauri window titlebar — don't rely on
it for debug output; use a visible DOM element instead.
