Sven Co-op over Reticulum — Portable Edition
============================================

This is a self-contained portable build. All mutable state — settings,
RNS identities, the steamcmd bootstrap, and the pulled Sven Co-op
dedicated server (~2.74 GB) — is stored in the `sc-rns-data/` folder
next to the executable, NOT in any system location.

How to use
----------
1. Extract this archive anywhere you like (Desktop, USB stick, etc.).
2. Run the executable inside. The `sc-rns-data/` folder is created
   automatically on first run (with a PORTABLE.txt marker).
3. To "uninstall", just delete the folder. Nothing is written outside it.

To host a server: open the DS tab → "Start / pull" to download the
Sven Co-op dedicated server (~2.74 GB) into `sc-rns-data/svends/`.
To play: start a bridge client and connect to a host.

Moving the install
------------------
To move to another machine, copy the whole folder (including
`sc-rns-data/`). The downloaded server, your identity, and settings
travel with it.

Fallback
--------
If the executable's directory isn't writable (e.g. extracted to
`/usr/bin` or `C:\Program Files` without admin rights), the app falls
back to the OS per-app data dir:
  Linux:   ~/.local/share/org.svencoop.rns.gui/
  Windows: %APPDATA%\org.svencoop.rns.gui\
  macOS:   ~/Library/Application Support/org.svencoop.rns.gui/