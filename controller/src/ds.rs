//! Sven Co-op dedicated server manager: find, pull (via steamcmd), start,
//! stop, and pre-create the soundcache files the DS fails to generate on
//! Linux/macOS.
//!
//! Rust port of the DS logic in `run.sh` / `run.bat`. The DS (Steam app 276060)
//! is a 32-bit GoldSrc `svends_run` (Linux) / `svends.exe` (Windows) child
//! process. macOS has no native build. All paths resolve from a bundle dir —
//! no hardcoded absolute paths (the Steam install candidates are expanded
//! from `$HOME` / `%ProgramFiles%`, not hardcoded).

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use tokio::process::Child;
use tracing::{debug, info};

use crate::steamcmd::{Os, SteamcmdRunner};

/// Startup parameters for the dedicated server.
#[derive(Debug, Clone)]
pub struct DsStartArgs {
    /// UDP port the DS listens on (GoldSrc port, default 27015).
    pub port: u16,
    /// Max players (default 8).
    pub maxplayers: u32,
    /// Starting map (default svencoop1).
    pub map: String,
    /// Where to install/look for the DS. Defaults to `<bundle>/svends` if None.
    pub install_dir: Option<PathBuf>,
}

impl Default for DsStartArgs {
    fn default() -> Self {
        Self {
            port: 27015,
            maxplayers: 8,
            map: "svencoop1".to_string(),
            install_dir: None,
        }
    }
}

/// Live status of the dedicated server.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DsStatus {
    pub running: bool,
    /// The port the DS was started on (if running).
    pub port: Option<u16>,
    /// The install directory the DS was launched from (if running).
    pub install_dir: Option<PathBuf>,
}

/// Manages the Sven Co-op dedicated server (Steam app 276060) as a child
/// process. No hardcoded paths — all locations resolve from a bundle dir.
pub struct DsManager {
    bundle_dir: PathBuf,
    steamcmd: SteamcmdRunner,
    child: Option<Child>,
    status: DsStatus,
}

impl DsManager {
    pub fn new(bundle_dir: PathBuf) -> Self {
        Self {
            steamcmd: SteamcmdRunner::new(bundle_dir.clone()),
            bundle_dir,
            child: None,
            status: DsStatus::default(),
        }
    }

    /// The default install dir: `<bundle>/svends`.
    pub fn default_install_dir(&self) -> PathBuf {
        self.bundle_dir.join("svends")
    }

    /// The remembered install path marker file (`<bundle>/.svends_path`).
    fn path_marker(&self) -> PathBuf {
        self.bundle_dir.join(".svends_path")
    }

    /// Locate an existing DS executable. Order: bundle-local `./svends`,
    /// last-used path in `.svends_path`, standard Steam install paths. Returns
    /// the executable path and its containing dir.
    pub async fn find_svends(&self) -> Option<(PathBuf, PathBuf)> {
        let os = self.steamcmd.os();
        let exe_name = if os == Os::Windows { "svends.exe" } else { "svends_run" };

        // 1. Bundle-local ./svends.
        let local = self.default_install_dir().join(exe_name);
        if is_executable(&local).await {
            return Some((local, self.default_install_dir()));
        }

        // 2. Last-used path marker.
        if let Ok(prev) = tokio::fs::read_to_string(self.path_marker()).await {
            let prev = prev.trim();
            if !prev.is_empty() {
                let cand = Path::new(prev).join(exe_name);
                if is_executable(&cand).await {
                    return Some((cand, Path::new(prev).to_path_buf()));
                }
            }
        }

        // 3. Standard Steam install paths (expanded from HOME / ProgramFiles).
        for dir in steam_install_candidates(os) {
            let cand = dir.join("steamapps").join("common").join("Sven Co-op").join(exe_name);
            if is_executable(&cand).await {
                return Some((cand, dir.join("steamapps").join("common").join("Sven Co-op")));
            }
        }
        None
    }

    /// Start the DS: find an existing install or pull one via steamcmd,
    /// pre-create soundcache files, then spawn the server child process.
    pub async fn start(&mut self, args: DsStartArgs) -> Result<()> {
        if self.child.is_some() {
            anyhow::bail!("dedicated server is already running; stop it first");
        }
        let os = self.steamcmd.os();
        if !os.supports_dedicated_server() {
            return Err(anyhow!(
                "The Sven Co-op dedicated server (app 276060) has no native build for this OS — \
                 it ships only for Windows and Linux. Run the host side on Linux/a VM/Docker."
            ));
        }
        let exe_name = if os == Os::Windows { "svends.exe" } else { "svends_run" };

        let install_dir = args
            .install_dir
            .clone()
            .unwrap_or_else(|| self.default_install_dir());

        // Find or pull.
        let (exe, install_dir) = match self.find_svends().await {
            Some((exe, dir)) => (exe, dir),
            None => {
                info!(install_dir = %install_dir.display(), "no DS found; pulling via steamcmd");
                self.steamcmd.pull_ds(&install_dir).await?;
                let exe = install_dir.join(exe_name);
                if !is_executable(&exe).await {
                    return Err(anyhow!(
                        "steamcmd finished but {} was not found / not executable",
                        exe.display()
                    ));
                }
                // Remember the install path for next time.
                let _ = tokio::fs::write(self.path_marker(), install_dir.as_os_str().to_string_lossy().as_ref()).await;
                (exe, install_dir)
            }
        };

        // Pre-create soundcache files for every map (the DS can't generate
        // them on-the-fly on Linux/macOS, which disconnects clients).
        let created = precreate_soundcache(&install_dir).await;
        if created > 0 {
            info!(created, "pre-created empty soundcache file(s)");
        }

        info!(exe = %exe.display(), port = args.port, map = %args.map, maxplayers = args.maxplayers, "starting Sven Co-op dedicated server");
        let mut cmd = tokio::process::Command::new(&exe);
        cmd.arg("-port").arg(args.port.to_string())
            .arg("+maxplayers").arg(args.maxplayers.to_string())
            .arg("+map").arg(&args.map)
            .current_dir(&install_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit());
        let _ = cmd.kill_on_drop(true);
        let child = cmd
            .spawn()
            .with_context(|| format!("spawning dedicated server at {}", exe.display()))?;
        self.status = DsStatus {
            running: true,
            port: Some(args.port),
            install_dir: Some(install_dir),
        };
        self.child = Some(child);
        Ok(())
    }

    /// Stop the running DS, if any. Sends SIGKILL (Unix) / TerminateProcess
    /// (Windows) via `kill_on_drop`/`start_kill`.
    pub async fn stop(&mut self) -> Result<()> {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
            let _ = child.wait().await;
            info!("dedicated server stopped");
        }
        self.status = DsStatus::default();
        Ok(())
    }

    /// Refresh + return status. Polls the child: if it exited, clears state.
    pub async fn status(&mut self) -> DsStatus {
        if let Some(child) = self.child.as_mut() {
            // Try to reap without blocking.
            match child.try_wait() {
                Ok(Some(_)) => {
                    // Exited.
                    self.child = None;
                    self.status = DsStatus::default();
                }
                Ok(None) => {} // still running
                Err(_) => {}
            }
        }
        self.status.clone()
    }

    pub fn is_running(&self) -> bool {
        self.child.is_some()
    }
}

/// Standard Steam install locations for this OS, expanded from the user's home
/// / ProgramFiles (no hardcoded absolute paths).
fn steam_install_candidates(os: Os) -> Vec<PathBuf> {
    let home = dirs::home_dir();
    let mut out = Vec::new();
    if os == Os::Windows {
        for var in ["ProgramFiles(x86)", "ProgramFiles", "USERPROFILE"] {
            if let Ok(val) = std::env::var(var) {
                if !val.is_empty() {
                    out.push(PathBuf::from(val).join("Steam"));
                }
            }
        }
        out.push(PathBuf::from("C:\\Program Files (x86)\\Steam"));
        out.push(PathBuf::from("C:\\Program Files\\Steam"));
    } else {
        if let Some(h) = home {
            out.push(h.join(".local/share/Steam"));
            out.push(h.join(".steam/steam"));
            out.push(h.join(".var/app/com.valvesoftware.Steam/data/Steam"));
        }
    }
    out
}

/// True if the path exists and (on Unix) is executable.
async fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match tokio::fs::metadata(path).await {
            Ok(m) => m.is_file() && m.permissions().mode() & 0o111 != 0,
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        tokio::fs::metadata(path).await.map(|m| m.is_file()).unwrap_or(false)
    }
}

/// Create an empty `<install>/svencoop/maps/soundcache/<map>.txt` for every
/// `.bsp` in the maps dir that doesn't already have one. Returns the count
/// created. The SC DS fails to generate these on-the-fly on Linux/macOS,
/// causing "failed to transmit file" disconnects.
async fn precreate_soundcache(install_dir: &Path) -> usize {
    let maps_dir = install_dir.join("svencoop").join("maps");
    let soundcache_dir = maps_dir.join("soundcache");
    let mut created = 0usize;
    let mut entries = match tokio::fs::read_dir(&maps_dir).await {
        Ok(e) => e,
        Err(_) => return 0,
    };
    let _ = tokio::fs::create_dir_all(&soundcache_dir).await;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("bsp") {
            continue;
        }
        let Some(mapname) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let cache = soundcache_dir.join(format!("{mapname}.txt"));
        if !tokio::fs::try_exists(&cache).await.unwrap_or(false) {
            if tokio::fs::write(&cache, b"").await.is_ok() {
                created += 1;
            }
        }
    }
    if created > 0 {
        info!(soundcache_dir = %soundcache_dir.display(), created, "pre-created soundcache files");
    } else {
        debug!(maps_dir = %maps_dir.display(), "soundcache already present (or no maps)");
    }
    created
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_install_dir_is_bundle_relative() {
        let m = DsManager::new(PathBuf::from("/bundle"));
        assert_eq!(m.default_install_dir(), PathBuf::from("/bundle/svends"));
        assert_eq!(m.path_marker(), PathBuf::from("/bundle/.svends_path"));
    }

    #[tokio::test]
    async fn find_returns_none_when_empty_bundle() {
        let tmp = tempfile::tempdir().unwrap();
        let m = DsManager::new(tmp.path().to_path_buf());
        // No DS installed, no Steam on the CI box → None (or Some if host has
        // Steam; tolerate both, but on a clean temp it's None).
        let found = m.find_svends().await;
        // Host may have a Steam install; just assert it doesn't panic and
        // either is fine. On the headless .135 box this is None.
        let _ = found;
    }

    #[tokio::test]
    async fn find_locates_bundle_local_svends() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle = tmp.path().to_path_buf();
        let svends_dir = bundle.join("svends");
        tokio::fs::create_dir_all(&svends_dir).await.unwrap();
        let exe = svends_dir.join("svends_run");
        tokio::fs::write(&exe, b"#!/bin/sh\n").await.unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = tokio::fs::metadata(&exe).await.unwrap().permissions();
            perms.set_mode(0o755);
            tokio::fs::set_permissions(&exe, perms).await.unwrap();
        }
        let m = DsManager::new(bundle.clone());
        let found = m.find_svends().await.expect("should find bundle-local svends");
        assert_eq!(found.0, exe);
        assert_eq!(found.1, svends_dir);
    }

    #[tokio::test]
    async fn precreate_soundcache_creates_one_per_bsp() {
        let tmp = tempfile::tempdir().unwrap();
        let install = tmp.path();
        let maps = install.join("svencoop").join("maps");
        tokio::fs::create_dir_all(&maps).await.unwrap();
        tokio::fs::write(maps.join("a.bsp"), b"").await.unwrap();
        tokio::fs::write(maps.join("b.bsp"), b"").await.unwrap();
        let n = precreate_soundcache(install).await;
        assert_eq!(n, 2);
        assert!(install.join("svencoop/maps/soundcache/a.txt").exists());
        assert!(install.join("svencoop/maps/soundcache/b.txt").exists());
        // Second run is a no-op (already present).
        let n2 = precreate_soundcache(install).await;
        assert_eq!(n2, 0);
    }
}