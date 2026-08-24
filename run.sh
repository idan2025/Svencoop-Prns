#!/usr/bin/env bash
# sc-rns-bridge launcher for Linux/macOS.
# Run: ./run.sh   (or double-click in a file manager that executes scripts)
set -euo pipefail

cd "$(dirname "$0")"

BIN="./target/release/sc-rns-bridge"
if [ ! -x "$BIN" ]; then
    echo "Release binary not found at $BIN"
    echo "Building it now (first run takes a minute)..."
    cargo build --release
fi

# Try to locate the Sven Co-op dedicated server launcher.
find_svends() {
    local candidates=(
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
            -type f -name svends_run 2>/dev/null | head -1)
    [ -n "$found" ] && echo "$found" && return 0
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
        SVENDS=$(find_svends || true)
        if [ -z "$SVENDS" ]; then
            echo
            echo "Could not find svends_run (the Sven Co-op dedicated server)."
            echo "Looked in the usual Steam install paths. If Sven Co-op is"
            echo "installed elsewhere, pass the full path to svends_run as:"
            echo "  ./run.sh  (then choose 1)  -- not yet supported; run svends_run manually"
            echo
            read -rp "Enter full path to svends_run manually: " SVENDS
            if [ -z "$SVENDS" ] || [ ! -x "$SVENDS" ]; then
                echo "No valid path given. Aborting."
                exit 1
            fi
        fi
        SVENDS_DIR=$(dirname "$SVENDS")
        echo
        echo "Found dedicated server: $SVENDS"
        read -rp "UDP port [27015]: " sc_port
        sc_port="${sc_port:-27015}"
        read -rp "Max players [8]: " maxplayers
        maxplayers="${maxplayers:-8}"
        read -rp "Starting map [svencoop1]: " map
        map="${map:-svencoop1}"

        # Pre-create the soundcache file for the chosen map. The SC dedicated
        # server fails to generate these on-the-fly on Linux/macOS, causing
        # "failed to transmit file" errors that disconnect clients. Creating
        # an empty file lets the server send it without error.
        SOUNDCACHE_DIR="$SVENDS_DIR/svencoop/maps/soundcache"
        mkdir -p "$SOUNDCACHE_DIR" 2>/dev/null
        if [ ! -f "$SOUNDCACHE_DIR/${map}.txt" ]; then
            : > "$SOUNDCACHE_DIR/${map}.txt"
            echo "Pre-created empty soundcache: $SOUNDCACHE_DIR/${map}.txt"
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