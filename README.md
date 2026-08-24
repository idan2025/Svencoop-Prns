# Sven Co-op over Reticulum (Svencoop-Prns)

Play **Sven Co-op** with your friends over **Reticulum** — a secure, serverless
mesh-networking stack — using [Prns], the high-performance Rust implementation
of Reticulum. No central server, no port forwarding, no cloud relay, no
subscription. Reticulum handles end-to-end encryption, routing, and identity;
the bridge just shuffles GoldSrc UDP datagrams in and out of Reticulum Links.

> Bring your own network: two LoRa radios miles apart, a handful of laptops on
> a mountain, a phone hotspot in a building, or the ordinary internet — all
> just pipes to Reticulum. Players on a hike with no cell service can keep
> playing on the way home over Wi-Fi or TCP.

## What this is

A single cross-platform program that does **three jobs**, picked from a menu:

1. **Sven Co-op dedicated server** — bundled launcher that finds and starts
   your Sven Co-op dedicated server (`svends_run` on Linux/macOS,
   `svends.exe` on Windows) with the port, max players, and starting map you
   choose.
2. **Bridge server** — announces a Reticulum destination `sven-coop.server`
   and relays each accepted Reticulum Link to the Sven Co-op server's UDP
   port. This is the host side.
3. **Bridge client** — binds a local UDP port that the Sven Co-op *game
   client* connects to, and relays traffic over a Reticulum Link to the
   announced bridge server. This is the player side.

A single machine can run all three at once (host + play), or you can split
them across machines and networks. The bridge is symmetric: every player runs
the bridge client, the host runs the bridge server, and Reticulum meshes them
together over whatever links each side has.

### Features

- **Cross-platform** — Linux, macOS, and Windows, from one Rust codebase with
  no platform-specific code. Double-clickable launchers for all three.
- **Bundled Sven Co-op dedicated server launcher** — auto-detects your Steam
  install and starts the SC dedicated server with the port/map/players you
  pick, straight from the menu.
- **Interactive menu** — `run.sh` / `run.command` / `run.bat` walks you through
  every option (port, interface, map, server hash) without memorizing flags.
- **End-to-end encrypted** — Reticulum Links are encrypted by default; no
  device along the route can read the traffic, not even the source address.
- **No port forwarding on the game server** — the bridge server's Reticulum
  interface is the only thing that needs to be reachable; the Sven Co-op
  server stays on `127.0.0.1`.
- **Oversized-packet safe** — GoldSrc signon/world-state datagrams exceed
  Reticulum's link payload ceiling, so the bridge fragments and reassembles
  them transparently (length-prefix framing layer).
- **LAN or internet** — use Wi-Fi/LAN auto-discovery (`--auto`) for nearby
  peers with no internet, or a TCP interface (`--tcp`) for internet play.

## How it works

```
   GoldSrc client  --UDP-->  bridge client  --Reticulum Link-->  bridge server  --UDP-->  Sven Co-op server
        ^_________________________   E2E encrypted   ___________________________|
         (server replies flow back the same way)
```

The wire contract on every Reticulum Link is a length-prefix framing layer
over raw GoldSrc UDP datagrams: each datagram is split into link-MDU-sized
chunks (one byte of header + ≤384 bytes of payload), sent as separate link
packets, and reassembled on the far side before being handed to UDP. Reticulum
supplies encryption, ordering, integrity, and routing; the bridge is a thin,
reliable pump between a UDP socket and a Link.

- **Server side:** announces `sven-coop.server`, accepts Links, and for each
  Link opens a UDP socket to the local Sven Co-op server. Each player gets
  their own UDP socket and Link.
- **Client side:** binds `127.0.0.1:27015` (configurable). On the first packet
  from a GoldSrc client, it opens a Link to the announced server and relays
  both ways. Each distinct client source address gets its own Link; when a
  Link closes, the next packet re-establishes a fresh one.
- **Fire-and-forget link sends** — the bridge issues `SendToLink` commands
  without awaiting per-packet receipt settlement. The Reticulum engine still
  delivers reliably and retransmits as needed; not blocking on each ACK keeps
  latency low and prevents a slow ACK from stalling the UDP→Link pump.

## Build

Requires **Rust 1.90+** (stable).

```console
cargo build --release
```

The Prns engine is vendored under `vendor/` and built from source as part of
this crate, so the only external dependency is the Rust toolchain. No system
libraries are required for the TCP / Wi-Fi-auto features the bridge uses.

### Platform notes

| Platform | Launcher | SC dedicated server binary | Notes |
| --- | --- | --- | --- |
| **Linux** | `run.sh` | `svends_run` | Auto-detects Steam install paths. |
| **macOS** | `run.command` | `svends_run` | Double-click in Finder. Searches `~/Library/Application Support/Steam`. |
| **Windows** | `run.bat` | `svends.exe` | Double-click in Explorer. Searches `C:\Program Files (x86)\Steam`. |

On macOS, the first time you run `run.command` from Finder you may need to
right-click → Open to bypass Gatekeeper. On all platforms, the launcher
auto-builds the release binary on first run if it's missing.

## Run

Just run the launcher for your platform:

```bash
# Linux
./run.sh

# macOS
./run.command        # or double-click in Finder

# Windows
run.bat              # or double-click in Explorer
```

The menu:

```
==================================
  Sven Co-op over Reticulum
==================================
 1) Start Sven Co-op dedicated server
 2) Bridge server  (relays SC server traffic over Reticulum)
 3) Bridge client  (you are a player; connects to a bridge server)
 4) Build only
```

### Typical single-machine setup (host + play on one PC)

1. **Menu → 1** — starts the Sven Co-op dedicated server on port 27015.
2. **Menu → 2** — starts the bridge server, pointing at `127.0.0.1:27015`,
   with `--tcp 0.0.0.0:4234` (and/or `--auto` for LAN).
3. **Menu → 3** — starts the bridge client on a *different* local port
   (e.g. `27016`, to avoid clashing with the SC server on 27015) with
   `--tcp 127.0.0.1:4234`.
4. In the Sven Co-op game client console: `connect 127.0.0.1:27016`.

> Tip: if you're sitting next to the Sven Co-op server you don't need the
> bridge at all — just `connect localhost:27015` directly. The bridge earns
> its keep when players are on *other* machines or networks.

### Remote players

The host runs the SC dedicated server (menu 1) and the bridge server (menu 2)
with a publicly reachable interface — either `--tcp 0.0.0.0:4234` (forward
port 4234 on your router) or `--auto` for a LAN game.

Each remote player runs the bridge client (menu 3) with:

- `--tcp <host>:4234` for internet play (`<host>` = the host's public IP or
  hostname), or
- `--auto` for LAN/Wi-Fi play with no internet.

Then they connect their Sven Co-op client to `localhost:27015` (or whatever
`--listen-port` they chose). The bridge client auto-discovers the server via
the `sven-coop.server` announce, or the host can share the printed
`server_hash` for players to pass via `--server-hash <32-hex-chars>` to skip
discovery.

## CLI reference

All options are exposed via the launchers, but you can call the binary
directly too:

```
sc-rns-bridge server   --sc-host <host> --sc-port <port> [--tcp <addr>] [--auto] [--announce-interval <sec>]
sc-rns-bridge client   --listen-port <port> [--tcp <addr>] [--auto] [--server-hash <hex>]
```

`--tcp` convention:
- `0.0.0.0:PORT` (or `:PORT`) → bind a TCP server (use on the bridge server
  side for a public relay).
- `<host>:PORT` → connect as a TCP client (use on the bridge client side, or
  to chain across a transport node).

`--auto` enables Wi-Fi/LAN auto-discovery for nearby peers with no internet.
On a single machine, prefer `--tcp` — auto-discovery is for physical
LAN/Wi-Fi between separate machines.

## Testing

The loopback integration tests spin up a mock Sven Co-op echo server, a bridge
server, and a bridge client over a localhost TCP link, then verify UDP
datagrams round-trip through the whole bridge — including an oversized
1200-byte datagram that exercises the framing/reassembly layer.

```console
cargo test --test loopback -- --nocapture
```

## Troubleshooting

### "Error: server failed to transmit file 'maps/soundcache/\<map\>.txt'"

This is a **Sven Co-op server** issue, not a bridge issue. The SC dedicated
server fails to generate soundcache files on-the-fly on Linux/macOS, causing
client disconnects right after "STEAM USERID validated".

The launchers (menu option 1) **automatically pre-create an empty soundcache
file** for your chosen map before starting the server, which prevents this
error. If you start `svends_run` manually, create the file yourself first:

```bash
mkdir -p "<SC install>/svencoop/maps/soundcache"
touch "<SC install>/svencoop/maps/soundcache/<map>.txt"
```

Alternatively, launch the SC game client, start a local listen server on the
map, and the client will generate the soundcache file properly. Then the
dedicated server can use it.

### "Closed IP networking" / connection retries in a loop

Check that:
1. The SC dedicated server is actually running and listening on the port the
   bridge server points at (`--sc-port`, default 27015).
2. The bridge client used `--tcp <host:port>` (not just `--auto`) when
   connecting to a bridge server on the same machine. Auto-discovery is for
   physical LAN/Wi-Fi between separate machines.
3. The bridge client's `--listen-port` doesn't clash with the SC server's port
   if both run on the same machine (use e.g. `--listen-port 27016`).

### The SC client connects but hangs after "STEAM USERID validated"

This was a bridge issue caused by GoldSrc signon packets (~1400 bytes)
exceeding the Reticulum link payload ceiling. It's fixed in the current
version — the vendored Prns engine is sized for a 2048-byte link MTU so
GoldSrc datagrams fit in a single link packet. If you built from an older
commit, rebuild.

## Architecture notes

- **`src/framing.rs`** — length-prefix framing: splits each GoldSrc datagram
  into ≤384-byte chunks with a 1-byte final-chunk flag, reassembles on the
  far side. Without this, GoldSrc signon packets (~1400 bytes) exceed
  Reticulum's link MDU (~415 bytes at the default 500-byte `BROADCAST_MTU`).
- **`src/relay.rs`** — the bridge: an event router turns engine events into
  per-Link channel traffic, and per-Link relay tasks pump UDP↔Link both ways.
- **Per-Link UDP sockets** on the server side; **per-client-source-address
  Links** on the client side, so two players on one machine each get their
  own Link.
- **No persistence** — the bridge keeps no routing state across restarts;
  the Reticulum mesh re-learns announces within one announce interval.

## License

MIT OR Apache-2.0, matching Prns.

## Credits

- [Prns] — the high-performance Rust Reticulum engine this bridge is built on,
  by KenAKAFrosty.
- [Reticulum] / [RNS] — the reference Python implementation and the protocol
  specification, by markqvist.
- [Sven Co-op] — the game.

[Prns]: https://github.com/KenAKAFrosty/Prns
[Reticulum]: https://reticulum.rs
[RNS]: https://github.com/markqvist/Reticulum
[Sven Co-op]: https://www.svencoop.com/