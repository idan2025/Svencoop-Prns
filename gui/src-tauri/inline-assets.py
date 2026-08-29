#!/usr/bin/env python3
"""Inline app.js and style.css into index.html for the Tauri build.

The Tauri v2 asset resolver on Linux/WebKit doesn't serve external
scripts/styles referenced by relative path correctly. Inlining them
into index.html ensures they load. The separate app.js + style.css
files are kept for the headless web panel (Docker), which serves
them via ServeDir.

This script is idempotent: it reads app.js and style.css fresh each
time and regenerates index.html from a template.
"""
import pathlib

dist = pathlib.Path(__file__).parent.parent / "dist"

# Read the source files
css = (dist / "style.css").read_text()
js = (dist / "app.js").read_text()

# Read the current index.html
html = (dist / "index.html").read_text()

# If the HTML already has inlined content (from a previous run),
# rebuild it from the original template. We detect this by checking
# for inline <style> or <script> blocks.
if "<style>" in html or "tauriInvoke" in html:
    # Already inlined — we need to restore the template first.
    # The template is the HTML without inlined content.
    # Find the <head> section up to the closing </head>
    # and the body content, then rebuild.
    pass  # We'll just do a full replace below

# Build the inlined HTML from the known template
template = '''<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>Sven Co-op over Reticulum</title>
  <style>
__CSS__
  </style>
</head>
<body>
  <header>
    <h1>Sven Co-op <span>over Reticulum</span></h1>
    <div id="status-pill">idle</div>
  </header>

  <main>
    <!-- Left: server browser -->
    <section id="browser">
      <h2>Servers</h2>
      <table id="server-table">
        <thead>
          <tr><th>Name</th><th>Server hash</th><th>Last seen</th><th></th></tr>
        </thead>
        <tbody></tbody>
      </table>
      <p id="browser-empty">No servers discovered yet. Start a bridge server on the host and a client here.</p>
      <button id="refresh" title="Force a refresh">Refresh</button>
    </section>

    <!-- Right: tabs -->
    <section id="tabs">
      <nav id="tab-nav">
        <button data-tab="ds" class="active">DS</button>
        <button data-tab="server">Bridge Server</button>
        <button data-tab="client">Client</button>
        <button data-tab="ifaces">Interfaces</button>
      </nav>

      <!-- DS tab -->
      <div class="tab" id="tab-ds">
        <h2>Sven Co-op dedicated server</h2>
        <label>UDP port <input id="ds-port" type="number" value="27015" /></label>
        <label>Max players <input id="ds-maxplayers" type="number" value="8" /></label>
        <label>Starting map
          <input id="ds-map" type="text" value="svencoop1" list="ds-map-list" />
          <datalist id="ds-map-list"></datalist>
        </label>
        <label>Install dir <input id="ds-install" type="text" placeholder="(default: bundle/svends)" /></label>
        <label>Allow cheats (sv_cheats) <input id="ds-sv-cheats" type="checkbox" /></label>
        <div class="row">
          <button id="ds-start">Start / pull</button>
          <button id="ds-stop" class="danger">Stop</button>
        </div>
        <div id="ds-progress" class="ds-progress" hidden>
          <div class="ds-progress-bar">
            <div id="ds-progress-fill" class="ds-progress-fill"></div>
          </div>
          <p id="ds-progress-line" class="ds-progress-line"></p>
        </div>
        <p id="ds-status-line"></p>
        <fieldset>
          <legend>Change map (live, no restart)</legend>
          <label>Map <select id="ds-changelevel-map"></select></label>
          <div class="row">
            <button id="ds-changelevel">Change map</button>
            <button id="ds-refresh-maps">Refresh map list</button>
          </div>
          <p class="hint">Lists <code>.bsp</code> files actually installed under <code>svencoop/maps/</code>. Campaign maps often <code>changelevel</code> into <code>sp_campaign_portal</code> on their own when a round ends — that's the game's own campaign flow, not a bridge setting.</p>
        </fieldset>
        <fieldset id="ds-stats-box" hidden>
          <legend>Live server stats</legend>
          <p id="ds-stats-summary" class="hint"></p>
          <table id="ds-players-table">
            <thead><tr><th>Player</th><th>Score</th><th>Time connected</th></tr></thead>
            <tbody></tbody>
          </table>
          <p id="ds-players-empty" class="hint">No players connected.</p>
        </fieldset>
        <p id="resume-errors" class="hint" hidden style="color: var(--danger)"></p>
      </div>

      <!-- Bridge server tab -->
      <div class="tab" id="tab-server" hidden>
        <h2>Bridge server</h2>
        <label>SC host <input id="srv-schost" type="text" value="127.0.0.1" /></label>
        <label>SC port <input id="srv-scport" type="number" value="27015" /></label>
        <label>TCP bind (e.g. 0.0.0.0:4234) <input id="srv-tcp" type="text" placeholder="0.0.0.0:4234" /></label>
        <label>Auto (Wi-Fi/LAN) <input id="srv-auto" type="checkbox" /></label>
        <label>Announce interval (s) <input id="srv-ann" type="number" value="15" /></label>
        <label>Server name (broadcast in announces) <input id="srv-name" type="text" placeholder="e.g. Idan's Sven Co-op" /></label>
        <div class="row">
          <button id="srv-start">Start</button>
          <button id="srv-restart">Restart</button>
          <button id="srv-stop" class="danger">Stop</button>
        </div>
        <p id="srv-status-line"></p>
        <div id="srv-hash-box" hidden>
          <label>Server hash — give this to players</label>
          <div class="row">
            <input id="srv-hash" type="text" readonly />
            <button id="srv-hash-copy">Copy</button>
            <button id="srv-announce-now">Announce now</button>
          </div>
        </div>
        <fieldset id="srv-clients-box">
          <legend>Connected clients</legend>
          <table id="client-table">
            <thead><tr><th>Client identity hash</th></tr></thead>
            <tbody></tbody>
          </table>
          <p id="srv-clients-empty" class="hint">No clients connected.</p>
        </fieldset>
        <fieldset>
          <legend>Trace a destination</legend>
          <div class="row">
            <input id="srv-trace-hash" type="text" placeholder="32 hex chars" />
            <button id="srv-trace-btn">Trace</button>
          </div>
          <p id="srv-trace-result" class="hint"></p>
        </fieldset>
        <p class="hint">Independent of the Client tab — both can run at once (on different ports) if you want to host and connect out from the same machine.</p>
      </div>

      <!-- Client tab -->
      <div class="tab" id="tab-client" hidden>
        <h2>Bridge client</h2>
        <label>Local listen port <input id="cli-listen" type="number" value="27015" /></label>
        <label>TCP host:port <input id="cli-tcp" type="text" placeholder="example.com:4234" /></label>
        <label>Auto (Wi-Fi/LAN) <input id="cli-auto" type="checkbox" /></label>
        <label>Server hash (blank = auto-discover) <input id="cli-hash" type="text" placeholder="32 hex chars" /></label>
        <div class="row">
          <button id="cli-start">Start</button>
          <button id="cli-restart">Restart</button>
          <button id="cli-stop" class="danger">Stop</button>
        </div>
        <p id="cli-status-line"></p>
        <div id="cli-hash-box" hidden>
          <label>Client hash</label>
          <div class="row">
            <input id="cli-own-hash" type="text" readonly />
            <button id="cli-hash-copy">Copy</button>
          </div>
        </div>
        <fieldset>
          <legend>Trace the server</legend>
          <div class="row">
            <button id="cli-trace-btn">Trace</button>
          </div>
          <p class="hint">Traces the server hash entered above (or the one auto-discovered, once known).</p>
          <p id="cli-trace-result" class="hint"></p>
        </fieldset>
        <p class="hint">Then pick a server in the browser and click Connect — it launches the game joined to localhost:&lt;listen port&gt;. Independent of the Bridge Server tab — both can run at once (on different ports).</p>
      </div>

      <!-- Interfaces tab -->
      <div class="tab" id="tab-ifaces" hidden>
        <h2>Reticulum interfaces</h2>
        <table id="iface-table">
          <thead><tr><th>Role</th><th>Name</th><th>Mode</th><th>State</th><th>Links</th><th>RX</th><th>TX</th><th></th></tr></thead>
          <tbody></tbody>
        </table>
        <fieldset>
          <legend>Add interface</legend>
          <label>Attach to
            <select id="if-role">
              <option value="server">Bridge Server</option>
              <option value="client">Client</option>
            </select>
          </label>
          <label>TCP host:port (0.0.0.0:PORT to bind) <input id="if-tcp" type="text" placeholder="0.0.0.0:4234" /></label>
          <label>UDP local host:port <input id="if-udp-local" type="text" placeholder="0.0.0.0:4235" /></label>
          <label>UDP peer host:port <input id="if-udp-peer" type="text" placeholder="203.0.113.5:4235" /></label>
          <label>WebSocket — ws://host:port to connect, or host:port to bind <input id="if-ws" type="text" placeholder="ws://example.com:8080 or 0.0.0.0:8080" /></label>
          <label>IFAC network name (blank = open) <input id="if-ifac" type="text" /></label>
          <label>IFAC passphrase (the actual shared secret — name alone is guessable) <input id="if-ifac-pass" type="password" /></label>
          <div class="row">
            <button id="if-add-tcp">Add TCP</button>
            <button id="if-add-auto">Add Wi-Fi/LAN auto</button>
            <button id="if-add-udp">Add UDP</button>
            <button id="if-add-ws">Add WebSocket</button>
          </div>
        </fieldset>
      </div>
    </section>
  </main>

  <div id="toast"></div>
  <script>
__JS__
  </script>
</body>
</html>
'''

html = template.replace("__CSS__", css).replace("__JS__", js)
(dist / "index.html").write_text(html)
print("Inlined CSS and JS into index.html")