//! Headless web control panel for the Sven Co-op over Reticulum host.
//!
//! One process serves the static frontend (`gui/dist`) + a small REST API that
//! maps 1:1 onto `BridgeController` methods, runs the bridge server in-process
//! (via `BridgeSession`), and manages the Sven Co-op dedicated server as a
//! child process. On boot it calls `resume()` to restore the last running
//! config from `<bundle>/settings.json` (persisted on the volume).
//!
//! This is the Docker host entrypoint. The same `gui/dist` frontend also runs
//! under the Tauri desktop shell; the frontend's `call()` shim picks transport
//! (Tauri `invoke` vs HTTP `fetch`) at runtime.
//!
//! Builds headless (no webview dependency) — only `axum` + `tower-http` are
//! added, which are pure-Rust.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tower_http::services::ServeDir;

use sc_rns_controller::BridgeController;
use sc_rns_bridge::{ClientArgs, ServerArgs};

type CtrlState = Arc<Mutex<BridgeController>>;

#[derive(Clone)]
struct AppState {
    ctrl: CtrlState,
    bundle_dir: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sc_rns_controller=info,sc_rns_bridge=info,personal_rns=warn".into()),
        )
        .init();

    let bundle_dir = PathBuf::from(std::env::var("BUNDLE_DIR").unwrap_or_else(|_| "/data".to_string()));
    std::fs::create_dir_all(&bundle_dir).ok();

    let ctrl = Arc::new(Mutex::new(BridgeController::new(bundle_dir.clone())));

    // Restore the last running state (bridge + interfaces + DS) from settings.json.
    {
        let mut c = ctrl.lock().await;
        c.resume().await;
    }

    let state = AppState { ctrl: ctrl.clone(), bundle_dir: bundle_dir.clone() };

    let static_dir =
        PathBuf::from(std::env::var("GUI_DIST_DIR").unwrap_or_else(|_| "/app/gui/dist".to_string()));

    let app = Router::new()
        .route("/api/state", get(get_state))
        .route("/api/:cmd", post(api_dispatch))
        .with_state(state)
        // Static frontend: `/` → index.html, `/app.js` + `/style.css` → files.
        // The panel uses DOM tabs, no client-side routing, so no SPA fallback.
        .fallback_service(ServeDir::new(&static_dir));

    let port: u16 = std::env::var("WEB_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!(%addr, static_dir = %static_dir.display(), bundle_dir = %bundle_dir.display(), "sc-rns-controller-web listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn get_state(State(st): State<AppState>) -> impl IntoResponse {
    let mut ctrl = st.ctrl.lock().await;
    match ctrl.state().await {
        Ok(s) => Ok(Json(json!(s))),
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

/// Deserialize a camelCase JSON body into a typed request struct.
fn parse<T: for<'de> Deserialize<'de>>(body: Value) -> Result<T, String> {
    serde_json::from_value(body).map_err(|e| format!("invalid request body: {e}"))
}

/// One POST handler dispatching on the command name (the same names the Tauri
/// shell registers). Bodies use camelCase keys to match the frontend.
async fn api_dispatch(
    Path(cmd): Path<String>,
    State(st): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let mut ctrl = st.ctrl.lock().await;
    let bundle_dir = st.bundle_dir.clone();
    let ok = || Ok(Json(Value::Null));

    match cmd.as_str() {
        "get_state" | "state" => {
            let s = ctrl.state().await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
            Ok(Json(json!(s)))
        }

        // ---- dedicated server ----
        "ds_start" => {
            let r: DsStartReq = parse(body).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
            let args = sc_rns_controller::DsStartArgs {
                port: r.port,
                maxplayers: r.maxplayers,
                map: r.map,
                install_dir: r.install_dir.map(PathBuf::from),
            };
            ctrl.ds_start(args).await.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            ok()
        }
        "ds_stop" => {
            ctrl.ds_stop().await.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            ok()
        }

        // ---- bridge server ----
        "start_bridge_server" => {
            let r: ServerStartReq = parse(body).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
            let args = ServerArgs {
                sc_host: r.sc_host,
                sc_port: r.sc_port,
                identity: bundle_dir.join("server.identity"),
                tcp: r.tcp,
                auto: r.auto,
                announce_interval: r.announce_interval,
            };
            ctrl.start_bridge_server(args).await.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            ok()
        }
        "stop_bridge_server" => {
            ctrl.stop_bridge_server().await.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            ok()
        }
        "restart_bridge_server" => {
            ctrl.restart_bridge_server().await.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            ok()
        }

        // ---- bridge client (host rarely uses this, but supported) ----
        "start_client" => {
            let r: ClientStartReq = parse(body).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
            let args = ClientArgs {
                listen_port: r.listen_port,
                server_hash: r.server_hash,
                identity: bundle_dir.join("client.identity"),
                tcp: r.tcp,
                auto: r.auto,
            };
            ctrl.start_client(args).await.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            ok()
        }
        "stop_client" => {
            ctrl.stop_client().await.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            ok()
        }
        "restart_client" => {
            ctrl.restart_client().await.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            ok()
        }

        // ---- interfaces ----
        "add_interface_tcp" => {
            let r: AddTcpReq = parse(body).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
            ctrl.add_interface_tcp(r.addr, r.ifac_name)
                .await
                .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            ok()
        }
        "add_interface_auto" => {
            ctrl.add_interface_auto().map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            ok()
        }
        "remove_interface" => {
            let r: IdReq = parse(body).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
            ctrl.remove_interface(&r.id).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            ok()
        }
        "rename_interface" => {
            let r: RenameReq = parse(body).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
            ctrl.rename_interface(&r.id, r.name).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            ok()
        }

        // ---- connect + launch (no game client in docker; for the desktop shell) ----
        "connect_and_launch" => {
            let r: ServerHashReq = parse(body).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
            ctrl.connect_and_launch(r.server_hash)
                .await
                .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            ok()
        }

        other => Err((StatusCode::NOT_FOUND, format!("unknown command: {other}"))),
    }
}

// ---- request types (camelCase to match the frontend) ----

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DsStartReq {
    #[serde(default = "default_ds_port")]
    port: u16,
    #[serde(default = "default_ds_maxplayers")]
    maxplayers: u32,
    #[serde(default = "default_ds_map")]
    map: String,
    install_dir: Option<String>,
}
fn default_ds_port() -> u16 { 27015 }
fn default_ds_maxplayers() -> u32 { 8 }
fn default_ds_map() -> String { "svencoop1".to_string() }

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerStartReq {
    #[serde(default = "default_sc_host")]
    sc_host: String,
    #[serde(default = "default_ds_port")]
    sc_port: u16,
    tcp: Option<String>,
    #[serde(default)]
    auto: bool,
    #[serde(default = "default_announce")]
    announce_interval: u64,
}
fn default_sc_host() -> String { "127.0.0.1".to_string() }
fn default_announce() -> u64 { 15 }

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientStartReq {
    #[serde(default = "default_ds_port")]
    listen_port: u16,
    server_hash: Option<String>,
    tcp: Option<String>,
    #[serde(default)]
    auto: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddTcpReq {
    addr: String,
    ifac_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdReq {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RenameReq {
    id: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerHashReq {
    server_hash: String,
}