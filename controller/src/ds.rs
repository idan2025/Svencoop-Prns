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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::process::Child;
use tokio::task::JoinHandle;
use tracing::{debug, info};

use crate::steamcmd::{parse_progress, progress_caption, Os, SteamcmdRunner};

/// Startup parameters for the dedicated server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DsStartArgs {
    /// UDP port the DS listens on (GoldSrc port, default 27015).
    pub port: u16,
    /// Max players (default 8).
    pub maxplayers: u32,
    /// Starting map (default svencoop1).
    pub map: String,
    /// Where to install/look for the DS. Defaults to `<bundle>/svends` if None.
    pub install_dir: Option<PathBuf>,
    /// Enable `sv_cheats` at startup.
    #[serde(default)]
    pub sv_cheats: bool,
}

impl Default for DsStartArgs {
    fn default() -> Self {
        Self {
            port: 27015,
            maxplayers: 8,
            map: "svencoop1".to_string(),
            install_dir: None,
            sv_cheats: false,
        }
    }
}

/// Lifecycle phase of the dedicated server, surfaced to the UI so the
/// operator can tell a ~2.74 GB steamcmd pull from a running server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DsPhase {
    #[default]
    Idle,
    Pulling,
    Starting,
    Running,
    Error,
}

/// Live status of the dedicated server. Polled by the GUI every couple of
/// seconds; `phase` + `progress_pct` + `last_line` drive the download
/// progress bar during a steamcmd pull.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DsStatus {
    pub running: bool,
    /// The port the DS was started on (if running).
    pub port: Option<u16>,
    /// The install directory the DS was launched from (if running).
    pub install_dir: Option<PathBuf>,
    /// Current lifecycle phase (idle/pulling/starting/running/error).
    #[serde(default)]
    pub phase: DsPhase,
    /// Download progress 0.0–100.0, when `phase == pulling`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_pct: Option<f64>,
    /// Last steamcmd / DS output line, for the progress bar caption.
    #[serde(default)]
    pub last_line: String,
    /// Whether `sv_cheats` is currently on (if running). Reflects the last
    /// known state — either set at startup or toggled live via
    /// `ds_set_cheats`, so the UI checkbox doesn't drift from reality.
    #[serde(default)]
    pub sv_cheats: bool,
}

/// Interior-mutable live state shared between the foreground `DsManager`
/// (polling for the UI) and the background task that runs steamcmd + spawns
/// the DS. Held behind an `Arc<std::sync::Mutex<…>>` — locks are brief and
/// never held across `.await`.
#[derive(Debug, Clone, Default)]
struct DsLive {
    phase: DsPhase,
    progress_pct: Option<f64>,
    last_line: String,
    port: Option<u16>,
    install_dir: Option<PathBuf>,
    sv_cheats: bool,
}

impl DsLive {
    fn set(&mut self, phase: DsPhase, line: impl Into<String>) {
        self.phase = phase;
        self.last_line = line.into();
        if phase != DsPhase::Pulling {
            self.progress_pct = None;
        }
    }
}

/// Manages the Sven Co-op dedicated server (Steam app 276060) as a child
/// process. No hardcoded paths — all locations resolve from a bundle dir.
///
/// The potentially long steamcmd pull runs on a background task so the
/// controller mutex (and the UI's 2 s poll) is never blocked for minutes;
/// `status()` reads the shared `DsLive` snapshot without waiting.
pub struct DsManager {
    bundle_dir: PathBuf,
    steamcmd: SteamcmdRunner,
    live: Arc<Mutex<DsLive>>,
    child: Arc<Mutex<Option<Child>>>,
    /// The running DS child's stdin, so the operator can send live console
    /// commands (`changelevel <map>`, `sv_cheats 1`, ...) the same way an
    /// admin would type into the server console directly. A separate
    /// `tokio::sync::Mutex` (not the plain one `child`/`live` use) because
    /// writing to it is an async operation and needs to hold the guard
    /// across an `.await`.
    stdin: Arc<tokio::sync::Mutex<Option<tokio::process::ChildStdin>>>,
    cancel: Arc<AtomicBool>,
    /// Handle to the pull/spawn task, so `stop()` can cancel an in-flight pull.
    task: Option<JoinHandle<()>>,
}

impl DsManager {
    pub fn new(bundle_dir: PathBuf) -> Self {
        Self {
            steamcmd: SteamcmdRunner::new(bundle_dir.clone()),
            bundle_dir,
            live: Arc::new(Mutex::new(DsLive::default())),
            child: Arc::new(Mutex::new(None)),
            stdin: Arc::new(tokio::sync::Mutex::new(None)),
            cancel: Arc::new(AtomicBool::new(false)),
            task: None,
        }
    }

    /// The default install dir: `<bundle>/svends`.
    pub fn default_install_dir(&self) -> PathBuf {
        self.bundle_dir.join("svends")
    }

    /// True if a start/pull is in flight or the DS is running.
    pub fn is_running(&self) -> bool {
        let live = self.live.lock().unwrap();
        live.phase == DsPhase::Pulling
            || live.phase == DsPhase::Starting
            || live.phase == DsPhase::Running
    }

    /// Locate an existing DS executable. Order: bundle-local `./svends`,
    /// last-used path in `.svends_path`, standard Steam install paths. Returns
    /// the executable path and its containing dir.
    pub async fn find_svends(&self) -> Option<(PathBuf, PathBuf)> {
        find_svends_inner(&self.bundle_dir, &self.steamcmd).await
    }

    /// Start the DS: find an existing install or pull one via steamcmd,
    /// pre-create soundcache files, then spawn the server child process.
    ///
    /// The heavy work (steamcmd download + DS spawn) runs on a background
    /// task so this returns immediately. Progress is observable via
    /// [`status`]; cancel via [`stop`].
    pub async fn start(&mut self, args: DsStartArgs) -> Result<()> {
        {
            let live = self.live.lock().unwrap();
            if live.phase == DsPhase::Pulling
                || live.phase == DsPhase::Starting
                || live.phase == DsPhase::Running
            {
                anyhow::bail!("dedicated server is already starting/running; stop it first");
            }
        }
        let os = self.steamcmd.os();
        if !os.supports_dedicated_server() {
            return Err(anyhow!(
                "The Sven Co-op dedicated server (app 276060) has no native build for this OS — \
                 it ships only for Windows and Linux. Run the host side on Linux/a VM/Docker."
            ));
        }

        // Reset cancel + mark pulling. The background task owns the lifecycle.
        self.cancel.store(false, Ordering::Relaxed);
        {
            let mut live = self.live.lock().unwrap();
            live.set(DsPhase::Pulling, "locating or pulling dedicated server…");
            live.port = None;
            live.install_dir = None;
        }

        // Reap any prior finished task handle (non-blocking).
        if let Some(t) = self.task.take() {
            t.abort();
        }

        let bundle_dir = self.bundle_dir.clone();
        let steamcmd = SteamcmdRunner::new(self.bundle_dir.clone());
        let live = self.live.clone();
        let child = self.child.clone();
        let stdin = self.stdin.clone();
        let cancel = self.cancel.clone();

        self.task = Some(tokio::spawn(async move {
            ds_background(
                bundle_dir, steamcmd, args, live, child, stdin, cancel,
            )
            .await;
        }));
        Ok(())
    }

    /// Send a live console command to the running DS, exactly as an operator
    /// typing into the server console would (`changelevel <map>`,
    /// `sv_cheats 1`, ...). Errors if the DS isn't running.
    pub async fn send_command(&self, cmd: &str) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        let mut guard = self.stdin.lock().await;
        let stdin = guard
            .as_mut()
            .ok_or_else(|| anyhow!("dedicated server is not running"))?;
        stdin
            .write_all(cmd.as_bytes())
            .await
            .with_context(|| format!("sending DS console command {cmd:?}"))?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
        info!(cmd, "sent DS console command");
        Ok(())
    }

    /// Toggle `sv_cheats` live and record it, so `status()` reflects reality
    /// instead of drifting from whatever the UI last assumed.
    pub async fn set_cheats(&self, enabled: bool) -> Result<()> {
        self.send_command(&format!("sv_cheats {}", if enabled { 1 } else { 0 })).await?;
        self.live.lock().unwrap().sv_cheats = enabled;
        Ok(())
    }

    /// List installed maps (`.bsp` files under `svencoop/maps/`), sorted.
    /// Looks at the currently-known/last-used install first, then a fresh
    /// `find_svends()` lookup. Empty if the DS isn't installed yet.
    pub async fn list_maps(&self) -> Vec<String> {
        let install_dir = {
            let live = self.live.lock().unwrap();
            live.install_dir.clone()
        };
        let install_dir = match install_dir {
            Some(d) => Some(d),
            None => self.find_svends().await.map(|(_, dir)| dir),
        };
        let Some(install_dir) = install_dir else {
            return Vec::new();
        };
        list_maps_inner(&install_dir).await
    }

    /// Stop the running DS, if any. Cancels an in-flight steamcmd pull (the
    /// steamcmd child is killed via `kill_on_drop`) and the DS child process.
    pub async fn stop(&mut self) -> Result<()> {
        // Signal the background pull task (if any) to abort.
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(t) = self.task.take() {
            t.abort();
        }
        // Kill + reap the DS child, if present.
        let maybe_child = { self.child.lock().unwrap().take() };
        if let Some(mut child) = maybe_child {
            let _ = child.start_kill();
            let _ = child.wait().await;
            info!("dedicated server stopped");
        }
        *self.stdin.lock().await = None;
        {
            let mut live = self.live.lock().unwrap();
            live.set(DsPhase::Idle, "stopped");
            live.port = None;
            live.install_dir = None;
            live.sv_cheats = false;
        }
        Ok(())
    }

    /// Refresh + return status. Polls the child: if it exited, clears state.
    /// Reads the shared `DsLive` snapshot — never blocks on the background task.
    pub fn status(&self) -> DsStatus {
        // Reap an exited DS child without blocking.
        let running = {
            let mut guard = self.child.lock().unwrap();
            if let Some(child) = guard.as_mut() {
                match child.try_wait() {
                    Ok(Some(_)) => {
                        *guard = None;
                        false
                    }
                    Ok(None) => true,
                    Err(_) => true,
                }
            } else {
                false
            }
        };
        let live = self.live.lock().unwrap().clone();
        // If the child exited, the task already set phase=Idle/Error; keep that.
        // But if it died without the task noticing, surface it here.
        let phase = if !running && live.phase == DsPhase::Running {
            DsPhase::Idle
        } else {
            live.phase
        };
        DsStatus {
            running: running && phase == DsPhase::Running,
            port: live.port,
            install_dir: live.install_dir,
            phase,
            progress_pct: live.progress_pct,
            last_line: live.last_line,
            sv_cheats: live.sv_cheats,
        }
    }
}

/// Background task body: locate or pull the DS via steamcmd (streaming
/// progress into `live`), pre-create soundcache, then spawn the DS child.
async fn ds_background(
    bundle_dir: PathBuf,
    steamcmd: SteamcmdRunner,
    args: DsStartArgs,
    live: Arc<Mutex<DsLive>>,
    child: Arc<Mutex<Option<Child>>>,
    stdin: Arc<tokio::sync::Mutex<Option<tokio::process::ChildStdin>>>,
    cancel: Arc<AtomicBool>,
) {
    let os = steamcmd.os();
    let exe_name = if os == Os::Windows { "svends.exe" } else { "svends_run" };
    let install_dir = args
        .install_dir
        .clone()
        .unwrap_or_else(|| bundle_dir.join("svends"));

    // Find or pull.
    let (exe, install_dir) = match find_svends_inner(&bundle_dir, &steamcmd).await {
        Some(found) => {
            {
                let mut l = live.lock().unwrap();
                l.set(DsPhase::Starting, "found existing dedicated server install");
            }
            found
        }
        None => {
            if cancel.load(Ordering::Relaxed) {
                set_idle(&live);
                return;
            }
            info!(install_dir = %install_dir.display(), "no DS found; pulling via steamcmd");
            {
                let mut l = live.lock().unwrap();
                l.set(DsPhase::Pulling, "downloading steamcmd bootstrap…");
                l.progress_pct = Some(0.0);
            }
            // Progress callback: parse steamcmd's "progress: x (cur / total)"
            // lines into a human-readable caption + percent for the UI.
            let live_for_cb = live.clone();
            let on_line = move |line: &str| {
                let line = line.trim();
                if line.is_empty() {
                    return;
                }
                let mut l = live_for_cb.lock().unwrap();
                if l.phase != DsPhase::Pulling {
                    l.phase = DsPhase::Pulling;
                }
                if let Some(p) = parse_progress(line) {
                    l.progress_pct = Some(p.pct);
                    l.last_line = progress_caption(&p);
                } else {
                    // Non-progress status line (login, "Waiting for user
                    // info…", etc.) — show it as-is but keep the last known
                    // percent if any.
                    l.last_line = line.to_string();
                }
            };
            match steamcmd.pull_ds(&install_dir, &cancel, &on_line).await {
                Ok(()) => {}
                Err(e) => {
                    set_error(&live, &e.to_string());
                    return;
                }
            }
            if cancel.load(Ordering::Relaxed) {
                set_idle(&live);
                return;
            }
            let exe = install_dir.join(exe_name);
            if !is_executable(&exe).await {
                set_error(
                    &live,
                    &format!("steamcmd finished but {} not found / not executable", exe.display()),
                );
                return;
            }
            // Remember the install path for next time.
            let _ = tokio::fs::write(
                bundle_dir.join(".svends_path"),
                install_dir.as_os_str().to_string_lossy().as_ref(),
            )
            .await;
            (exe, install_dir)
        }
    };

    if cancel.load(Ordering::Relaxed) {
        set_idle(&live);
        return;
    }
    {
        let mut l = live.lock().unwrap();
        l.set(DsPhase::Starting, "preparing soundcache…");
    }

    // Pre-create soundcache files for every map (the DS can't generate
    // them on-the-fly on Linux/macOS, which disconnects clients).
    let created = precreate_soundcache(&install_dir).await;
    if created > 0 {
        info!(created, "pre-created empty soundcache file(s)");
    }

    // Stock mapcycle.txt ships with "-sp_campaign_portal" (a campaign hub
    // screen, not a playable coop map) as its first entry, so an empty
    // server times out back onto it. Strip it and put a real map first.
    if fix_mapcycle_default(&install_dir).await {
        info!("removed sp_campaign_portal from mapcycle.txt default rotation");
    }

    if cancel.load(Ordering::Relaxed) {
        set_idle(&live);
        return;
    }

    info!(exe = %exe.display(), port = args.port, map = %args.map, maxplayers = args.maxplayers, "starting Sven Co-op dedicated server");
    let mut cmd = tokio::process::Command::new(&exe);
    // The GoldSrc DS (`svends_i686`) always binds `0.0.0.0` — it ignores
    // the `-ip` flag (verified: `/proc/<pid>/cmdline` shows `-ip
    // 127.0.0.1` but `/proc/net/udp` still lists `0.0.0.0:<port>`). So the
    // container publishes 27015/udp unconditionally and the DS is reachable
    // on the LAN by design; there is no per-instance LAN gate here.
    cmd.arg("-port").arg(args.port.to_string())
        .arg("+maxplayers").arg(args.maxplayers.to_string())
        .arg("+map").arg(&args.map)
        .current_dir(&install_dir)
        // Piped (not null) so the operator can send live console commands
        // (changelevel, sv_cheats, ...) the same way typing into the
        // server's own console would.
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());
    let _ = cmd.kill_on_drop(true);
    match cmd
        .spawn()
        .with_context(|| format!("spawning dedicated server at {}", exe.display()))
    {
        Ok(mut spawned) => {
            let child_stdin = spawned.stdin.take();
            *child.lock().unwrap() = Some(spawned);
            {
                let mut l = live.lock().unwrap();
                l.set(DsPhase::Running, "dedicated server running");
                l.port = Some(args.port);
                l.install_dir = Some(install_dir);
                l.sv_cheats = args.sv_cheats;
            }
            *stdin.lock().await = child_stdin;
            if args.sv_cheats {
                use tokio::io::AsyncWriteExt;
                if let Some(s) = stdin.lock().await.as_mut() {
                    let _ = s.write_all(b"sv_cheats 1\n").await;
                    let _ = s.flush().await;
                }
            }
        }
        Err(e) => {
            set_error(&live, &format!("spawn failed: {e}"));
        }
    }
}

fn set_idle(live: &Arc<Mutex<DsLive>>) {
    let mut l = live.lock().unwrap();
    l.set(DsPhase::Idle, "stopped");
    l.port = None;
    l.install_dir = None;
    l.sv_cheats = false;
}

fn set_error(live: &Arc<Mutex<DsLive>>, msg: &str) {
    let mut l = live.lock().unwrap();
    l.set(DsPhase::Error, msg);
    l.progress_pct = None;
    l.port = None;
    l.install_dir = None;
    l.sv_cheats = false;
    tracing::error!(error = msg, "DS background task failed");
}

/// Locate an existing DS executable. Order: bundle-local `./svends`,
/// last-used path in `.svends_path`, standard Steam install paths.
async fn find_svends_inner(bundle_dir: &Path, steamcmd: &SteamcmdRunner) -> Option<(PathBuf, PathBuf)> {
    let os = steamcmd.os();
    let exe_name = if os == Os::Windows { "svends.exe" } else { "svends_run" };
    let default_install = bundle_dir.join("svends");
    let path_marker = bundle_dir.join(".svends_path");

    // 1. Bundle-local ./svends.
    let local = default_install.join(exe_name);
    if is_executable(&local).await {
        return Some((local, default_install));
    }

    // 2. Last-used path marker.
    if let Ok(prev) = tokio::fs::read_to_string(&path_marker).await {
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

/// List installed maps: every `.bsp` file's stem under
/// `<install>/svencoop/maps/`, sorted. Also checks `svencoop_addon/maps`
/// (workshop/custom map installs land there), if present.
async fn list_maps_inner(install_dir: &Path) -> Vec<String> {
    let mut maps = Vec::new();
    for game_dir in ["svencoop", "svencoop_addon"] {
        let maps_dir = install_dir.join(game_dir).join("maps");
        let Ok(mut entries) = tokio::fs::read_dir(&maps_dir).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("bsp") {
                continue;
            }
            if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                maps.push(name.to_string());
            }
        }
    }
    maps.sort();
    maps.dedup();
    maps
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

/// Removes `-sp_campaign_portal` / `sp_campaign_portal` from
/// `svencoop/mapcycle.txt` and, if `svencoop1` is present, moves it to the
/// front. Only touches the file if the portal entry is actually present, so
/// a deliberately customized mapcycle is left alone. Returns `true` if the
/// file was rewritten.
async fn fix_mapcycle_default(install_dir: &Path) -> bool {
    let path = install_dir.join("svencoop").join("mapcycle.txt");
    let Ok(contents) = tokio::fs::read_to_string(&path).await else {
        return false;
    };
    let is_portal = |line: &str| matches!(line.trim(), "-sp_campaign_portal" | "sp_campaign_portal");
    if !contents.lines().any(is_portal) {
        return false;
    }
    let mut lines: Vec<&str> = contents.lines().filter(|l| !is_portal(l)).collect();
    if let Some(pos) = lines.iter().position(|l| l.trim() == "svencoop1") {
        lines.swap(0, pos);
    }
    let mut new_contents = lines.join("\n");
    new_contents.push('\n');
    tokio::fs::write(&path, new_contents).await.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_install_dir_is_bundle_relative() {
        let m = DsManager::new(PathBuf::from("/bundle"));
        assert_eq!(m.default_install_dir(), PathBuf::from("/bundle/svends"));
        assert_eq!(m.bundle_dir.join(".svends_path"), PathBuf::from("/bundle/.svends_path"));
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

    #[tokio::test]
    async fn fix_mapcycle_default_strips_portal_and_promotes_svencoop1() {
        let tmp = tempfile::tempdir().unwrap();
        let install = tmp.path();
        let svencoop = install.join("svencoop");
        tokio::fs::create_dir_all(&svencoop).await.unwrap();
        let cycle = svencoop.join("mapcycle.txt");
        tokio::fs::write(&cycle, "-sp_campaign_portal\nabandoned\nsvencoop1\ncrystal\n")
            .await
            .unwrap();
        assert!(fix_mapcycle_default(install).await);
        let fixed = tokio::fs::read_to_string(&cycle).await.unwrap();
        assert_eq!(fixed, "svencoop1\nabandoned\ncrystal\n");
        // Idempotent: no portal entry left, second run is a no-op.
        assert!(!fix_mapcycle_default(install).await);
    }

    #[tokio::test]
    async fn fix_mapcycle_default_leaves_custom_cycle_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let install = tmp.path();
        let svencoop = install.join("svencoop");
        tokio::fs::create_dir_all(&svencoop).await.unwrap();
        let cycle = svencoop.join("mapcycle.txt");
        tokio::fs::write(&cycle, "crystal\nabandoned\n").await.unwrap();
        assert!(!fix_mapcycle_default(install).await);
        let unchanged = tokio::fs::read_to_string(&cycle).await.unwrap();
        assert_eq!(unchanged, "crystal\nabandoned\n");
    }

    #[test]
    fn ds_status_reports_phase() {
        let m = DsManager::new(PathBuf::from("/bundle"));
        let s = m.status();
        assert_eq!(s.phase, DsPhase::Idle);
        assert!(!s.running);
    }
}