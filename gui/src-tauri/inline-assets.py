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
          <tr><th>Server hash</th><th>Last seen</th><th></th></tr>
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
        <label>Starting map <input id="ds-map" type="text" value="svencoop1" /></label>
        <label>Install dir <input id="ds-install" type="text" placeholder="(default: bundle/svends)" /></label>
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
        <div class="row">
          <button id="srv-start">Start</button>
          <button id="srv-restart">Restart</button>
          <button id="srv-stop" class="danger">Stop</button>
        </div>
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
        <p class="hint">Then pick a server in the browser and click Connect — it launches the game joined to localhost:&lt;listen port&gt;.</p>
      </div>

      <!-- Interfaces tab -->
      <div class="tab" id="tab-ifaces" hidden>
        <h2>Reticulum interfaces</h2>
        <table id="iface-table">
          <thead><tr><th>Name</th><th>Mode</th><th>State</th><th>Links</th><th>RX</th><th>TX</th><th></th></tr></thead>
          <tbody></tbody>
        </table>
        <fieldset>
          <legend>Add interface</legend>
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