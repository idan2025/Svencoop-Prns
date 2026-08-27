//! Headless platform core for the Sven Co-op over Reticulum GUI.
//!
//! This crate is the orchestration layer the GUI (a thin Tauri shell) drives:
//! it owns a [`BridgeSession`] (server or client role), manages the Sven Co-op
//! dedicated server as a child process, downloads it via steamcmd on demand,
//! edits Reticulum interfaces live through the `PrnsNodeHandle`, and launches
//! the Sven Co-op game auto-connected to a chosen server.
//!
//! It deliberately has **no Tauri/webview dependency**, so it builds and runs
//! headless (e.g. on `.135`) — the GUI shell is built separately where the
//! webview stack exists.

pub mod controller;
pub mod ds;
pub mod game;
pub mod server_entry;
pub mod steamcmd;

pub use controller::{BridgeController, ControllerState, InterfaceDescriptor, InterfaceInfo, Settings};
pub use ds::{DsManager, DsStartArgs, DsStatus};
pub use game::GameLauncher;
pub use server_entry::ServerEntry;
pub use steamcmd::SteamcmdRunner;