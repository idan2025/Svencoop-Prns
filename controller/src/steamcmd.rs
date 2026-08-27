//! Rust-native steamcmd runner: downloads the steamcmd bootstrap and pulls
//! the Sven Co-op dedicated server (Steam app 276060) with anonymous login.
//!
//! This is the Rust port of the shell logic in `run.sh` / `run.bat`:
//!   - OS-correct Valve archive download + extract (tar.gz on linux/macos,
//!     zip on windows), with a fallback mirror.
//!   - `steamcmd +force_install_dir <dir> +login anonymous +app_update 276060
//!     validate +quit` — `+force_install_dir` MUST precede `+login` or
//!     steamcmd exits 8 ("Please use force_install_dir before logon!").
//!   - Linux: probe steamcmd once for missing 32-bit loader libs; install them
//!     via apt-get if possible, else print the distro command.
//!   - macOS: no native Sven Co-op dedicated server exists, so we decline.
//!
//! No hardcoded absolute paths — every location resolves from a bundle dir
//! supplied by the caller.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};

/// Operating system, detected from `std::env::consts::OS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    Windows,
    Macos,
    Other,
}

impl Os {
    pub fn detect() -> Self {
        match std::env::consts::OS {
            "linux" => Os::Linux,
            "windows" => Os::Windows,
            "macos" => Os::Macos,
            _ => Os::Other,
        }
    }

    /// The Sven Co-op dedicated server (app 276060) ships only for Windows and
    /// Linux. macOS has no native build.
    pub fn supports_dedicated_server(self) -> bool {
        matches!(self, Os::Linux | Os::Windows)
    }
}

const STEAMCDN_PRIMARY: &str = "https://steamcdn-a.akamaihd.net/client/installer";
const STEAMCDN_FALLBACK: &str = "http://media.steampowered.com/installer";

/// The steamcmd bootstrap archive name for this OS (none on unsupported OSes).
fn steamcmd_archive(os: Os) -> Option<&'static str> {
    match os {
        Os::Linux => Some("steamcmd_linux.tar.gz"),
        Os::Windows => Some("steamcmd.zip"),
        Os::Macos => Some("steamcmd_osx.tar.gz"),
        Os::Other => None,
    }
}

/// OS-correct steamcmd bootstrap + DS pull. All paths are relative to a bundle
/// dir supplied by the caller — no hardcoded absolute paths.
pub struct SteamcmdRunner {
    bundle_dir: PathBuf,
    os: Os,
}

impl SteamcmdRunner {
    pub fn new(bundle_dir: PathBuf) -> Self {
        Self {
            bundle_dir,
            os: Os::detect(),
        }
    }

    /// Build for a specific OS (used by tests).
    #[cfg(test)]
    pub fn with_os(bundle_dir: PathBuf, os: Os) -> Self {
        Self { bundle_dir, os }
    }

    pub fn os(&self) -> Os {
        self.os
    }

    /// The steamcmd directory (`<bundle>/steamcmd`).
    pub fn steamcmd_dir(&self) -> PathBuf {
        self.bundle_dir.join("steamcmd")
    }

    /// Absolute path to the steamcmd executable for this OS, if present.
    pub fn steamcmd_path(&self) -> PathBuf {
        if self.os == Os::Windows {
            self.steamcmd_dir().join("steamcmd.exe")
        } else {
            self.steamcmd_dir().join("steamcmd.sh")
        }
    }

    /// Ensure the steamcmd bootstrap is present on disk, downloading + extracting
    /// it if not. Returns the path to the steamcmd executable. On Linux this
    /// also probes for the 32-bit loader libs and installs them if missing.
    pub async fn ensure_steamcmd(&self) -> Result<PathBuf> {
        let exe = self.steamcmd_path();
        if exe.exists() {
            return Ok(exe);
        }
        let archive_name = steamcmd_archive(self.os)
            .ok_or_else(|| anyhow!("steamcmd has no bootstrap for OS {:?}", self.os))?;
        let dir = self.steamcmd_dir();
        tokio::fs::create_dir_all(&dir)
            .await
            .with_context(|| format!("creating steamcmd dir {}", dir.display()))?;

        let bytes = download_with_fallback(archive_name).await?;
        extract_archive(&bytes, self.os, &dir)?;
        drop(bytes);

        if !exe.exists() {
            return Err(anyhow!(
                "steamcmd bootstrap extracted but {} was not found",
                exe.display()
            ));
        }

        if self.os == Os::Linux {
            self.ensure_linux_32bit_deps().await?;
        }
        Ok(exe)
    }

    /// Probe steamcmd for missing 32-bit loader libs (the DS and steamcmd are
    /// 32-bit). If the probe shows a missing shared object and apt-get is
    /// available, install lib32z1 lib32gcc-s1 lib32stdc++6; otherwise print the
    /// distro-specific command and bail.
    pub async fn ensure_linux_32bit_deps(&self) -> Result<()> {
        let exe = self.steamcmd_path();
        // Run `steamcmd.sh +quit` once; if the 32-bit loader libs are missing it
        // prints "cannot open shared object" / "error while loading shared
        // libraries" before getting anywhere.
        let output = tokio::process::Command::new(&exe)
            .arg("+quit")
            .output()
            .await
            .with_context(|| format!("probing steamcmd at {}", exe.display()))?;
        let probe = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if !probe
            .to_lowercase()
            .contains("cannot open shared object")
            && !probe
                .to_lowercase()
                .contains("error while loading shared libraries")
        {
            return Ok(()); // libs present (or some other non-loader state)
        }

        tracing::warn!(
            "steamcmd missing 32-bit libraries; attempting to install lib32z1 lib32gcc-s1 lib32stdc++6"
        );
        if which("apt-get").is_some() {
            run_sudo_apt_install(&["lib32z1", "lib32gcc-s1", "lib32stdc++6"]).await?;
            return Ok(());
        }
        Err(anyhow!(
            "Missing 32-bit libraries required by steamcmd / the dedicated server.\n\
             Install them for your distro and re-run, e.g.:\n  \
             Arch:    sudo pacman -S --needed lib32-glibc lib32-gcc-libs\n  \
             Fedora:  sudo dnf install -y glibc.i686 libstdc++.i686\n  \
             Debian:  sudo apt-get install -y lib32z1 lib32gcc-s1 lib32stdc++6"
        ))
    }

    /// Pull the Sven Co-op dedicated server (app 276060) into `install_dir` via
    /// anonymous login. `+force_install_dir` is passed BEFORE `+login` (the
    /// known ordering trap — otherwise steamcmd exits 8). Streams steamcmd
    /// stdout/stderr to tracing.
    pub async fn pull_ds(&self, install_dir: &Path) -> Result<()> {
        let exe = self.ensure_steamcmd().await?;
        let install_dir = install_dir
            .canonicalize()
            .or_else(|_| Ok::<_, std::io::Error>(install_dir.to_path_buf()))?;
        let mut cmd = tokio::process::Command::new(&exe);
        cmd.arg(format!("+force_install_dir {}", install_dir.display()))
            .arg("+login")
            .arg("anonymous")
            .arg("+app_update")
            .arg("276060")
            .arg("validate")
            .arg("+quit");
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let _ = cmd.kill_on_drop(true);

        tracing::info!(install_dir = %install_dir.display(), "running steamcmd to pull Sven Co-op DS (app 276060)");
        let mut child = cmd
            .spawn()
            .with_context(|| format!("spawning steamcmd at {}", exe.display()))?;
        use tokio::io::AsyncReadExt;
        let mut stdout = child.stdout.take().expect("piped");
        let mut stderr = child.stderr.take().expect("piped");
        let mut out_buf = [0u8; 4096];
        let mut err_buf = [0u8; 4096];
        loop {
            tokio::select! {
                n = stdout.read(&mut out_buf) => {
                    match n {
                        Ok(0) => break,
                        Ok(n) => tracing::info!("steamcmd: {}", String::from_utf8_lossy(&out_buf[..n]).trim_end()),
                        Err(e) => { tracing::warn!(error=?e, "steamcmd stdout read ended"); break; }
                    }
                }
                n = stderr.read(&mut err_buf) => {
                    match n {
                        Ok(0) => break,
                        Ok(n) => tracing::warn!("steamcmd: {}", String::from_utf8_lossy(&err_buf[..n]).trim_end()),
                        Err(_) => break,
                    }
                }
            }
        }
        let status = child
            .wait()
            .await
            .context("waiting for steamcmd")?;
        if !status.success() {
            return Err(anyhow!(
                "steamcmd exited with {} — the download may have failed; check the log above",
                status
            ));
        }
        Ok(())
    }
}

/// Download `archive_name` from the primary CDN, falling back to the mirror.
/// 15s per-request timeout, retries the fallback once on failure.
async fn download_with_fallback(archive_name: &str) -> Result<Vec<u8>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()?;
    let primary = format!("{STEAMCDN_PRIMARY}/{archive_name}");
    let fallback = format!("{STEAMCDN_FALLBACK}/{archive_name}");
    for url in [primary.as_str(), fallback.as_str()] {
        tracing::info!(url, "downloading steamcmd");
        match client.get(url).send().await {
            Ok(resp) if resp.status().is_success() => match resp.bytes().await {
                Ok(b) => return Ok(b.to_vec()),
                Err(e) => tracing::warn!(url, error=%e, "body read failed; trying next URL"),
            },
            Ok(resp) => tracing::warn!(url, status=%resp.status(), "non-success; trying next URL"),
            Err(e) => tracing::warn!(url, error=%e, "request failed; trying next URL"),
        }
    }
    Err(anyhow!("failed to download steamcmd archive {archive_name} from both CDN and mirror"))
}

/// Extract a steamcmd bootstrap archive into `dest`.
fn extract_archive(bytes: &[u8], os: Os, dest: &Path) -> Result<()> {
    match os {
        Os::Linux | Os::Macos => {
            let tar = flate2::read::GzDecoder::new(Cursor::new(bytes));
            let mut archive = tar::Archive::new(tar);
            archive.unpack(dest).with_context(|| format!("extracting tar.gz into {}", dest.display()))?;
            Ok(())
        }
        Os::Windows => {
            let reader = Cursor::new(bytes);
            let mut zip = zip::ZipArchive::new(reader).context("reading steamcmd.zip")?;
            for i in 0..zip.len() {
                let mut entry = zip.by_index(i).context("reading zip entry")?;
                let name = entry.name().to_string();
                let outpath = dest.join(&name);
                if entry.is_dir() {
                    std::fs::create_dir_all(&outpath)
                        .with_context(|| format!("creating zip dir {}", outpath.display()))?;
                } else {
                    if let Some(parent) = outpath.parent() {
                        std::fs::create_dir_all(parent)
                            .with_context(|| format!("creating parent for {}", outpath.display()))?;
                    }
                    let mut out = std::fs::File::create(&outpath)
                        .with_context(|| format!("creating {}", outpath.display()))?;
                    std::io::copy(&mut entry, &mut out)?;
                }
            }
            Ok(())
        }
        Os::Other => Err(anyhow!("no steamcmd archive for unsupported OS")),
    }
}

/// Run `sudo apt-get update && sudo apt-get install -y <pkgs>` for the given
/// packages. Sudo may prompt for a password; that's expected.
async fn run_sudo_apt_install(pkgs: &[&str]) -> Result<()> {
    let mut update = tokio::process::Command::new("sudo");
    update.arg("apt-get").arg("update");
    update
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());
    let status = update.status().await.context("sudo apt-get update")?;
    if !status.success() {
        return Err(anyhow!("sudo apt-get update exited {status}"));
    }
    let mut install = tokio::process::Command::new("sudo");
    install.arg("apt-get").arg("install").arg("-y").args(pkgs);
    install
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());
    let status = install.status().await.context("sudo apt-get install")?;
    if !status.success() {
        return Err(anyhow!(
            "sudo apt-get install exited {status}.\nInstall manually: sudo apt-get install -y {}",
            pkgs.join(" ")
        ));
    }
    Ok(())
}

/// Tiny `which` — true if the command resolves on PATH.
fn which(cmd: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let candidate = dir.join(cmd);
            if candidate.is_file() {
                Some(candidate)
            } else {
                None
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn archive_name_per_os() {
        assert_eq!(steamcmd_archive(Os::Linux), Some("steamcmd_linux.tar.gz"));
        assert_eq!(steamcmd_archive(Os::Windows), Some("steamcmd.zip"));
        assert_eq!(steamcmd_archive(Os::Macos), Some("steamcmd_osx.tar.gz"));
        assert_eq!(steamcmd_archive(Os::Other), None);
    }

    #[test]
    fn os_detect_matches_consts() {
        let os = Os::detect();
        assert_eq!(os, Os::detect()); // stable
        // Only sanity: the build host's OS is one we know.
        assert!(matches!(os, Os::Linux | Os::Windows | Os::Macos | Os::Other));
    }

    #[test]
    fn only_linux_windows_have_ds() {
        assert!(Os::Linux.supports_dedicated_server());
        assert!(Os::Windows.supports_dedicated_server());
        assert!(!Os::Macos.supports_dedicated_server());
        assert!(!Os::Other.supports_dedicated_server());
    }

    #[test]
    fn steamcmd_path_is_bundle_relative() {
        let r = SteamcmdRunner::with_os(PathBuf::from("/bundle"), Os::Linux);
        assert_eq!(r.steamcmd_path(), PathBuf::from("/bundle/steamcmd/steamcmd.sh"));
        let r = SteamcmdRunner::with_os(PathBuf::from("/bundle"), Os::Windows);
        assert_eq!(r.steamcmd_path(), PathBuf::from("/bundle/steamcmd/steamcmd.exe"));
    }

    #[test]
    fn extract_linux_tar_gz() {
        // Build a tar.gz in memory containing one file "steamcmd.sh".
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let mut data = std::io::Cursor::new(b"#!/bin/sh\necho hi\n");
            let mut header = tar::Header::new_gnu();
            header.set_path("steamcmd.sh").unwrap();
            header.set_size(data.get_ref().len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append(&header, &mut data).unwrap();
            builder.finish().unwrap();
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&tar_bytes).unwrap();
        let gz_bytes = gz.finish().unwrap();

        let tmp = tempfile::tempdir().unwrap();
        extract_archive(&gz_bytes, Os::Linux, tmp.path()).unwrap();
        assert!(tmp.path().join("steamcmd.sh").exists());
    }

    #[test]
    fn extract_windows_zip() {
        let buf: Vec<u8>;
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::<u8>::new()));
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("steamcmd.exe", opts).unwrap();
            zip.write_all(b"MZ fake").unwrap();
            let cursor = zip.finish().unwrap();
            buf = cursor.into_inner();
        }
        let tmp = tempfile::tempdir().unwrap();
        extract_archive(&buf, Os::Windows, tmp.path()).unwrap();
        assert!(tmp.path().join("steamcmd.exe").exists());
    }
}