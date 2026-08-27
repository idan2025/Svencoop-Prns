#!/usr/bin/env bash
# sc-rns-bridge launcher for Linux/macOS.
# Run: ./run.sh   (or double-click in a file manager that executes scripts)
set -euo pipefail

# Bundle dir = where this script lives (resolved absolute, never hardcoded).
BUNDLE_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$BUNDLE_DIR"

BIN="$BUNDLE_DIR/target/release/sc-rns-bridge"
if [ ! -x "$BIN" ]; then
    echo "Release binary not found at $BIN"
    echo "Building it now (first run takes a minute)..."
    cargo build --release
fi

# The Sven Co-op dedicated server (Steam app 276060) has a build only for
# Windows and Linux. There is no native macOS dedicated server, so on macOS
# we never try to download one.
detect_os() {
    case "$(uname -s)" in
        Linux)  echo linux ;;
        Darwin) echo macos ;;
        *)      echo other ;;
    esac
}

# Try to locate the Sven Co-op dedicated server launcher.
# Order: bundle-local ./svends, last-used path in ./.svends_path, Steam install
# paths, then a broad search. No hardcoded absolute paths.
find_svends() {
    local candidates=(
        "$BUNDLE_DIR/svends/svends_run"
    )
    # Reuse a previously chosen install path, if any.
    if [ -f "$BUNDLE_DIR/.svends_path" ]; then
        local prev
        prev="$(cat "$BUNDLE_DIR/.svends_path" 2>/dev/null || true)"
        [ -n "$prev" ] && candidates+=("$prev/svends_run")
    fi
    candidates+=(
        "$HOME/.local/share/Steam/steamapps/common/Sven Co-op/svends_run"
        "$HOME/.steam/steam/steamapps/common/Sven Co-op/svends_run"
        "$HOME/.var/app/com.valvesoftware.Steam/data/Steam/steamapps/common/Sven Co-op/svends_run"
    )
    for c in "${candidates[@]}"; do
        if [ -x "$c" ]; then echo "$c"; return 0; fi
    done
    # Last resort: search broadly (slow, but thorough).
    local found
    found=$(find "$HOME/.local/share/Steam" "$HOME/.steam" "$HOME/.var/app/com.valvesoftware.Steam" \
            -type f -name svends_run 2>/dev/null | head -1 || true)
    [ -n "$found" ] && echo "$found" && return 0
    return 1
}

# Download + extract steamcmd into $BUNDLE_DIR/steamcmd if not already present.
ensure_steamcmd() {
    local sc="$BUNDLE_DIR/steamcmd/steamcmd.sh"
    if [ -x "$sc" ]; then return 0; fi
    mkdir -p "$BUNDLE_DIR/steamcmd"
    local tgz="$BUNDLE_DIR/steamcmd_linux.tar.gz"
    local url="https://steamcdn-a.akamaihd.net/client/installer/steamcmd_linux.tar.gz"
    local alt="http://media.steampowered.com/installer/steamcmd_linux.tar.gz"
    echo "Downloading steamcmd..."
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$url" -o "$tgz" || curl -fsSL "$alt" -o "$tgz"
    elif command -v wget >/dev/null 2>&1; then
        wget -q "$url" -O "$tgz" || wget -q "$alt" -O "$tgz"
    else
        echo "Need curl or wget to download steamcmd. Install one and re-run."
        return 1
    fi
    if ! tar -xzf "$tgz" -C "$BUNDLE_DIR/steamcmd"; then
        echo "Failed to extract steamcmd archive."
        rm -f "$tgz"
        return 1
    fi
    rm -f "$tgz"
    if [ ! -x "$sc" ]; then
        echo "steamcmd.sh not found after extraction."
        return 1
    fi
    return 0
}

# steamcmd and the Sven Co-op dedicated server are 32-bit. Probe whether the
# 32-bit C runtime is present by launching steamcmd once; if the loader is
# missing libs it prints "cannot open shared object" / "error while loading
# shared libraries" and we offer to install them.
ensure_linux_32bit_deps() {
    local probe
    probe=$(timeout 60 "$BUNDLE_DIR/steamcmd/steamcmd.sh" +quit 2>&1 || true)
    if ! printf '%s' "$probe" | grep -qiE "cannot open shared object|error while loading shared libraries"; then
        return 0  # steamcmd started (libs present) or some other non-loader state.
    fi
    echo
    echo "Missing 32-bit libraries required by steamcmd / the dedicated server."
    if command -v apt-get >/dev/null 2>&1; then
        echo "Installing lib32z1 lib32gcc-s1 lib32stdc++6 via apt-get"
        echo "(this may ask for your password)..."
        if sudo apt-get update && sudo apt-get install -y lib32z1 lib32gcc-s1 lib32stdc++6; then
            return 0
        fi
        echo
        echo "apt-get install failed. Install them manually, then re-run:"
        echo "  sudo apt-get install -y lib32z1 lib32gcc-s1 lib32stdc++6"
        return 1
    fi
    echo "Install the 32-bit C runtime for your distro, then re-run, e.g.:"
    echo "  Arch:    sudo pacman -S --needed lib32-glibc lib32-gcc-libs"
    echo "  Fedora:  sudo dnf install -y glibc.i686 libstdc++.i686"
    echo "  Debian:  sudo apt-get install -y lib32z1 lib32gcc-s1 lib32stdc++6"
    return 1
}

echo "=================================="
echo "  Sven Co-op over Reticulum"
echo "=================================="
echo " 1) Start Sven Co-op dedicated server"
echo " 2) Bridge server  (relays SC server traffic over Reticulum)"
echo " 3) Bridge client  (you are a player; connects to a bridge server)"
echo " 4) Build only"
echo
read -rp "Choose [1-4]: " choice

case "$choice" in
    1)
        OS="$(detect_os)"

        if [ "$OS" != "linux" ]; then
            # No native dedicated server on macOS / unsupported OS.
            echo
            echo "The Sven Co-op dedicated server (Steam app 276060) has no native"
            echo "build for $(uname -s) — it ships only for Windows and Linux."
            echo
            echo "To host, run the dedicated server + bridge server (menus 1 and 2)"
            echo "on a Linux machine, VM, or Docker container. This machine can still"
            echo "run the bridge client (menu 3) to play."
            echo
            echo "If you have a svends_run reachable (e.g. via Wine or a shared VM"
            echo "mount), enter its full path below; otherwise just abort."
            echo
            read -rp "Full path to svends_run (blank to abort): " SVENDS
            if [ -z "$SVENDS" ] || [ ! -x "$SVENDS" ]; then
                echo "Aborting."
                exit 1
            fi
            SVENDS_DIR="$(dirname "$SVENDS")"
        else
            # Linux: find an existing DS, or pull one via steamcmd.
            SVENDS="$(find_svends || true)"
            if [ -z "$SVENDS" ]; then
                echo
                echo "No Sven Co-op dedicated server found."
                read -rp "Download it via steamcmd now? [y/N]: " dl
                dl="${dl:-N}"
                if [ "$dl" != "y" ] && [ "$dl" != "Y" ]; then
                    echo "Aborting."
                    exit 1
                fi

                ensure_steamcmd || exit 1
                ensure_linux_32bit_deps || exit 1

                default_install="$BUNDLE_DIR/svends"
                read -rp "Install path for the dedicated server [$default_install]: " install_dir
                install_dir="${install_dir:-$default_install}"

                echo
                echo "Downloading Sven Co-op dedicated server (app 276060) into:"
                echo "  $install_dir"
                echo "This is ~2.7 GB. Please wait..."
                echo
                set +e
                "$BUNDLE_DIR/steamcmd/steamcmd.sh" +force_install_dir "$install_dir" \
                    +login anonymous +app_update 276060 validate +quit
                rc=$?
                set -e
                if [ "$rc" -ne 0 ]; then
                    echo
                    echo "steamcmd exited with code $rc. The download may have failed."
                    echo "Check the output above and re-run."
                    exit 1
                fi
                SVENDS="$install_dir/svends_run"
                if [ ! -x "$SVENDS" ]; then
                    echo "Download finished but $SVENDS was not found / not executable."
                    exit 1
                fi
                echo "$install_dir" > "$BUNDLE_DIR/.svends_path"
            fi
            SVENDS_DIR="$(dirname "$SVENDS")"
        fi

        echo
        echo "Found dedicated server: $SVENDS"
        read -rp "UDP port [27015]: " sc_port
        sc_port="${sc_port:-27015}"
        read -rp "Max players [8]: " maxplayers
        maxplayers="${maxplayers:-8}"
        read -rp "Starting map [svencoop1]: " map
        map="${map:-svencoop1}"

        # Pre-create soundcache files for ALL maps in the maps directory.
        # The SC dedicated server fails to generate these on-the-fly on
        # Linux/macOS, causing "failed to transmit file" errors that
        # disconnect clients. Creating empty files for every .bsp means
        # map changes mid-game won't break either.
        SOUNDCACHE_DIR="$SVENDS_DIR/svencoop/maps/soundcache"
        mkdir -p "$SOUNDCACHE_DIR" 2>/dev/null
        created=0
        for bsp in "$SVENDS_DIR"/svencoop/maps/*.bsp; do
            [ -f "$bsp" ] || continue
            mapname=$(basename "$bsp" .bsp)
            if [ ! -f "$SOUNDCACHE_DIR/${mapname}.txt" ]; then
                : > "$SOUNDCACHE_DIR/${mapname}.txt"
                created=$((created + 1))
            fi
        done
        if [ "$created" -gt 0 ]; then
            echo "Pre-created $created empty soundcache file(s) in $SOUNDCACHE_DIR"
        fi

        echo
        echo "Starting Sven Co-op dedicated server on port $sc_port..."
        echo "Map: $map   Max players: $maxplayers"
        echo "Press Ctrl-C to stop."
        echo
        cd "$SVENDS_DIR"
        exec ./svends_run -port "$sc_port" +maxplayers "$maxplayers" +map "$map"
        ;;
    2)
        read -rp "Sven Co-op server host [127.0.0.1]: " sc_host
        sc_host="${sc_host:-127.0.0.1}"
        read -rp "Sven Co-op server UDP port [27015]: " sc_port
        sc_port="${sc_port:-27015}"
        echo
        echo "Interface: how should this node reach other nodes?"
        echo "  a) TCP server (bind a public relay, e.g. 0.0.0.0:4234)"
        echo "  b) Wi-Fi/LAN auto-discovery (no internet needed)"
        echo "  c) Both"
        read -rp "Choose [a/b/c]: " iface
        tcp_flag=""; auto_flag=""
        case "$iface" in
            a|A) read -rp "TCP bind address [0.0.0.0:4234]: " tcp; tcp="${tcp:-0.0.0.0:4234}"; tcp_flag="--tcp $tcp" ;;
            b|B) auto_flag="--auto" ;;
            c|C) read -rp "TCP bind address [0.0.0.0:4234]: " tcp; tcp="${tcp:-0.0.0.0:4234}"; tcp_flag="--tcp $tcp"; auto_flag="--auto" ;;
            *) echo "invalid"; exit 1 ;;
        esac
        read -rp "Announce interval seconds [15]: " ann
        ann="${ann:-15}"
        echo
        echo "Starting bridge server. Press Ctrl-C to stop."
        echo "Players use the printed server_hash with --server-hash, or just run a client."
        echo
        exec "$BIN" server --sc-host "$sc_host" --sc-port "$sc_port" $tcp_flag $auto_flag --announce-interval "$ann"
        ;;
    3)
        read -rp "Local UDP port for GoldSrc client to connect to [27015]: " listen
        listen="${listen:-27015}"
        echo
        echo "Interface: how should this node reach the bridge server?"
        echo "  a) TCP client (connect to a public relay, e.g. example.com:4234)"
        echo "  b) Wi-Fi/LAN auto-discovery (no internet needed)"
        read -rp "Choose [a/b]: " iface
        tcp_flag=""; auto_flag=""
        case "$iface" in
            a|A) read -rp "Bridge server host:port (e.g. 1.2.3.4:4234): " tcp; tcp_flag="--tcp $tcp" ;;
            b|B) auto_flag="--auto" ;;
            *) echo "invalid"; exit 1 ;;
        esac
        read -rp "Server destination hash (32 hex chars, blank to auto-discover): " hash
        hash_flag=""
        if [ -n "$hash" ]; then hash_flag="--server-hash $hash"; fi
        echo
        echo "Starting bridge client. Point your Sven Co-op client at localhost:$listen"
        echo "Press Ctrl-C to stop."
        echo
        exec "$BIN" client --listen-port "$listen" $tcp_flag $auto_flag $hash_flag
        ;;
    4)
        cargo build --release
        ;;
    *)
        echo "Invalid choice"
        exit 1
        ;;
esac