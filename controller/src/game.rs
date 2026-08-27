//! Sven Co-op game launcher: opens the game auto-connected to a bridge
//! client's localhost listen port via the OS URI handler.
//!
//! The Sven Co-op *game* is Steam app **225840** (the DS is 276060). The game
//! connects to the **bridge client's local UDP port** — not to Reticulum
//! directly — so the URI is `steam://run/225840//+connect%20127.0.0.1:<port>`.

use anyhow::{anyhow, Context, Result};

/// The Steam appid of the Sven Co-op *game* client.
pub const SC_GAME_APPID: u32 = 225840;

/// Launches the Sven Co-op game connected to `127.0.0.1:<listen_port>`.
pub struct GameLauncher;

impl GameLauncher {
    /// Build the `steam://` URI that runs the game and connects to a server.
    /// `steam://run/225840//+connect%20127.0.0.1:<port>`.
    pub fn connect_uri(listen_port: u16) -> String {
        format!("steam://run/{SC_GAME_APPID}//+connect%20127.0.0.1:{listen_port}")
    }

    /// Open the game via the OS URI handler. Fire-and-forget: spawns the
    /// handler detached and returns. No hardcoded paths — uses `open`
    /// (macOS), `xdg-open` (Linux), or `cmd /C start` (Windows).
    pub fn launch(listen_port: u16) -> Result<()> {
        let uri = Self::connect_uri(listen_port);
        let mut cmd = match std::env::consts::OS {
            "macos" => {
                let mut c = std::process::Command::new("open");
                c.arg(&uri);
                c
            }
            "linux" => {
                let mut c = std::process::Command::new("xdg-open");
                c.arg(&uri);
                c
            }
            "windows" => {
                let mut c = std::process::Command::new("cmd");
                c.arg("/C").arg("start").arg("").arg(&uri);
                c
            }
            other => return Err(anyhow!("no URI handler for OS {other}")),
        };
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // Detach: don't wait — the handler returns immediately and the game
        // launches out-of-band via Steam.
        cmd.spawn()
            .with_context(|| format!("opening game via URI handler: {uri}"))?;
        tracing::info!(uri, "launched Sven Co-op game");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_has_appid_and_connect() {
        let uri = GameLauncher::connect_uri(27016);
        assert!(uri.contains("steam://run/225840//"));
        assert!(uri.contains("+connect%20127.0.0.1:27016"));
    }

    #[test]
    fn appid_is_game_not_ds() {
        assert_eq!(SC_GAME_APPID, 225840, "game appid, not DS 276060");
    }
}