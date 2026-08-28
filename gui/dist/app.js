// Sven Co-op over Reticulum — GUI frontend (vanilla JS, no bundler).
// Transport-agnostic: under the Tauri desktop shell it calls
// window.__TAURI__.core.invoke; served from the docker host it POSTs to /api.
// One frontend, two shells.
//
// Wrapped in an IIFE so top-level `let`/`const` can't collide with a second
// script evaluation (WebKitGTK's Tauri webview re-runs inline scripts on
// early paint in some cases, and a bare top-level `let` throws "Identifier
// has already been declared" on the second pass — silently breaking every
// handler below it with no visible error).
(function () {

window.addEventListener("error", (ev) => {
  console.error("[sc-rns-gui] uncaught error", ev.error || ev.message);
  try { toast("GUI error: " + ((ev.error && ev.error.message) || ev.message || "unknown"), "error"); } catch (_) {}
});
window.addEventListener("unhandledrejection", (ev) => {
  console.error("[sc-rns-gui] unhandled rejection", ev.reason);
  try { toast("GUI error: " + ((ev.reason && ev.reason.message) || ev.reason), "error"); } catch (_) {}
});

let tauriInvoke = (window.__TAURI__?.core?.invoke) || (window.__TAURI__?.invoke);

// Fallback: if the __TAURI_IIFE__ global isn't ready yet but the internal
// invoke is available (injected by Tauri's init script before page scripts
// run), use it directly.
if (!tauriInvoke && window.__TAURI_INTERNALS__?.invoke) {
  tauriInvoke = (cmd, args) => window.__TAURI_INTERNALS__.invoke(cmd, args);
}

const isTauri = !!tauriInvoke;

const $ = (id) => document.getElementById(id);
let pollTimer = null;

function toast(msg, kind = "info") {
  const t = $("toast");
  t.textContent = msg;
  t.className = kind;
  clearTimeout(t._timer);
  t._timer = setTimeout(() => { t.className = ""; t.textContent = ""; }, 4000);
}

async function call(cmd, args = {}) {
  try {
    if (isTauri) {
      return await tauriInvoke(cmd, args);
    }
    // Web: POST /api/<cmd> with a JSON body (camelCase keys, matching Tauri).
    const r = await fetch("/api/" + cmd, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(args ?? {}),
    });
    if (!r.ok) {
      const t = await r.text();
      throw new Error(t || (r.status + " " + r.statusText));
    }
    const ct = r.headers.get("content-type") || "";
    return ct.includes("json") ? await r.json() : await r.text();
  } catch (e) {
    toast(String(e), "error");
    throw e;
  }
}

function val(id, fallback = "") {
  const el = $(id);
  return el ? (el.value === "" ? fallback : el.value) : fallback;
}

function num(id, fallback) {
  const v = val(id, String(fallback));
  const n = Number(v);
  return Number.isFinite(n) ? n : fallback;
}

function checked(id) {
  const el = $(id);
  return el ? el.checked : false;
}

function optStr(id) {
  const v = val(id, "");
  return v ? v : null;
}

// ---- tabs ----
document.querySelectorAll("#tab-nav button").forEach((btn) => {
  btn.addEventListener("click", () => {
    document.querySelectorAll("#tab-nav button").forEach((b) => b.classList.remove("active"));
    btn.classList.add("active");
    document.querySelectorAll(".tab").forEach((t) => (t.hidden = true));
    $("tab-" + btn.dataset.tab).hidden = false;
  });
});

// ---- DS ----
$("ds-start").addEventListener("click", async () => {
  await call("ds_start", {
    port: num("ds-port", 27015),
    maxplayers: num("ds-maxplayers", 8),
    map: val("ds-map", "svencoop1"),
    installDir: optStr("ds-install"),
  });
  toast("Dedicated server starting (will pull via steamcmd if needed).", "info");
});
$("ds-stop").addEventListener("click", async () => { await call("ds_stop"); toast("DS stopped."); });

// ---- bridge server ----
$("srv-start").addEventListener("click", async () => {
  await call("start_bridge_server", {
    scHost: val("srv-schost", "127.0.0.1"),
    scPort: num("srv-scport", 27015),
    tcp: optStr("srv-tcp"),
    auto: checked("srv-auto"),
    announceInterval: num("srv-ann", 15),
  });
  toast("Bridge server started.");
});
$("srv-restart").addEventListener("click", async () => { await call("restart_bridge_server"); toast("Bridge server restarted."); });
$("srv-stop").addEventListener("click", async () => { await call("stop_bridge_server"); toast("Bridge server stopped."); });

// ---- client ----
$("cli-start").addEventListener("click", async () => {
  await call("start_client", {
    listenPort: num("cli-listen", 27015),
    serverHash: optStr("cli-hash"),
    tcp: optStr("cli-tcp"),
    auto: checked("cli-auto"),
  });
  toast("Bridge client started.");
});
$("cli-restart").addEventListener("click", async () => { await call("restart_client"); toast("Client restarted."); });
$("cli-stop").addEventListener("click", async () => { await call("stop_client"); toast("Client stopped."); });

// ---- interfaces ----
$("if-add-tcp").addEventListener("click", async () => {
  await call("add_interface_tcp", {
    addr: val("if-tcp", ""),
    role: val("if-role", "server"),
    ifacName: optStr("if-ifac"),
    ifacPassphrase: optStr("if-ifac-pass"),
  });
  toast("Interface added.");
});
$("if-add-auto").addEventListener("click", async () => {
  await call("add_interface_auto", {
    role: val("if-role", "server"),
    ifacName: optStr("if-ifac"),
    ifacPassphrase: optStr("if-ifac-pass"),
  });
  toast("Auto interface added.");
});
$("if-add-udp").addEventListener("click", async () => {
  await call("add_interface_udp", {
    local: val("if-udp-local", ""),
    peer: val("if-udp-peer", ""),
    role: val("if-role", "server"),
    ifacName: optStr("if-ifac"),
    ifacPassphrase: optStr("if-ifac-pass"),
  });
  toast("UDP interface added.");
});
$("if-add-ws").addEventListener("click", async () => {
  await call("add_interface_websocket", {
    addr: val("if-ws", ""),
    role: val("if-role", "server"),
    ifacName: optStr("if-ifac"),
    ifacPassphrase: optStr("if-ifac-pass"),
  });
  toast("WebSocket interface added.");
});

function ifaceRow(i) {
  const tr = document.createElement("tr");
  const roleLabel = i.role === "client" ? "Client" : "Server";
  tr.innerHTML = `<td>${roleLabel}</td><td>${i.name || "—"}</td><td>${i.mode}</td><td>${i.connection}</td><td>${i.links}</td><td>${i.rx_bytes}</td><td>${i.tx_bytes}</td>`;
  const td = document.createElement("td");
  const rename = document.createElement("button");
  rename.textContent = "Rename";
  rename.addEventListener("click", async () => {
    const name = prompt("New name for interface " + i.id.slice(0, 8));
    if (name) { await call("rename_interface", { id: i.id, name }); refresh(); }
  });
  const remove = document.createElement("button");
  remove.textContent = "Remove";
  remove.className = "danger";
  remove.addEventListener("click", async () => { await call("remove_interface", { id: i.id }); refresh(); });
  td.append(rename, remove);
  tr.appendChild(td);
  return tr;
}

// ---- connect + launch ----
async function connectAndLaunch(hash) {
  await call("connect_and_launch", { serverHash: hash });
  toast("Launching Sven Co-op, connecting to localhost…", "info");
}

// ---- refresh / poll ----
function serverRow(s) {
  const tr = document.createElement("tr");
  tr.innerHTML = `<td><code title="${s.destination_hash}">${s.destination_hash.slice(0, 16)}…</code></td><td>${s.last_seen_ago_secs}s ago</td>`;
  const td = document.createElement("td");
  const btn = document.createElement("button");
  btn.textContent = "Connect";
  btn.addEventListener("click", () => connectAndLaunch(s.destination_hash));
  td.appendChild(btn);
  tr.appendChild(td);
  return tr;
}

async function refresh() {
  try {
    const s = await call("get_state");
    // Server browser.
    const tbody = $("server-table").querySelector("tbody");
    tbody.innerHTML = "";
    $("browser-empty").style.display = s.servers.length ? "none" : "block";
    s.servers.forEach((srv) => tbody.appendChild(serverRow(srv)));
    // Interfaces.
    const ibody = $("iface-table").querySelector("tbody");
    ibody.innerHTML = "";
    s.interfaces.forEach((i) => ibody.appendChild(ifaceRow(i)));
    // DS status + download progress.
    const ds = s.ds || {};
    const phase = ds.phase || "idle";
    const progressEl = $("ds-progress");
    const fillEl = $("ds-progress-fill");
    const lineEl = $("ds-progress-line");
    if (phase === "pulling" || phase === "starting") {
      progressEl.hidden = false;
      const pct = (ds.progress_pct != null) ? ds.progress_pct : 0;
      fillEl.style.width = Math.max(0, Math.min(100, pct)).toFixed(1) + "%";
      lineEl.textContent = ds.last_line || (phase === "pulling" ? "downloading…" : "starting…");
    } else {
      progressEl.hidden = true;
    }
    $("ds-status-line").textContent = ds.running
      ? `Running on port ${ds.port ?? "?"} (${ds.install_dir ?? "?"})`
      : (phase === "error" ? ("Error: " + (ds.last_line || "unknown")) : "Stopped.");
    // Bridge server / client status — independent of each other.
    $("srv-status-line").textContent = s.server_running ? "Running." : "Stopped.";
    $("cli-status-line").textContent = s.client_running ? "Running." : "Stopped.";
    // Resume warnings (rare): surface inline so the operator notices.
    const re = $("resume-errors");
    if (re) {
      if (s.resume_errors && s.resume_errors.length) {
        re.textContent = "On startup: " + s.resume_errors.join("; ");
        re.hidden = false;
      } else {
        re.hidden = true;
      }
    }
    // Status pill.
    $("status-pill").textContent = s.bridge_running ? (s.bridge_role || "running") : "idle";
    $("status-pill").className = s.bridge_running ? "on" : "";
  } catch (e) {
    // Silent on poll errors to avoid toast spam.
  }
}

$("refresh").addEventListener("click", refresh);
pollTimer = setInterval(refresh, 2000);
refresh();

})();