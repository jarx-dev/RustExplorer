# RustExplorer

A tabbed, dual-pane file manager for Windows 11 in Rust (eframe/egui 0.37). One EXE; English UI by default, Polish in Settings.

Every text the UI shows is a key in `crates/rx-core/locales/*.yml` (both `en` and `pl`), read with `t!`/`plural` (`rx_core::i18n`); `cargo test -p rx-core i18n` fails on a key used in the sources without a text. A layout stored without `lang=` (from before the choice) keeps Polish.

## Commands

- Build on Linux: `cargo xwin build --release --target x86_64-pc-windows-msvc`. Only the msvc target embeds the icon and manifest; the gnu target builds without them.
- Tests on Linux: `cargo test -p rx-core -p rx-update -p rx-archive -p rx-scan`. `rx-sys` and the bin test only on Windows (CI runs the whole workspace there).
- Screenshots: `rust-explorer.exe --screenshot out.png --shot-after <ms>` uses a throw-away layout, with no tray and no updates. Test on a real Windows machine through the `win-laptop-test` skill when it is available; those runs must stay invisible.
- README pictures (`docs/screenshots`) show demo data only: `--theme dark --no-network`, on a folder outside the user profile, so the path, the tree and the drives show nothing of the machine.

## Releases and updates

- Two update channels: GitHub releases (`.github/workflows/release.yml`, started by hand from `main`) for public copies, and `scripts/publish.sh` / `auto-publish.sh` for a private server. The private server's address is in `~/.config/rustexplorer/build.env` and never in the repo. A build without an update URL never updates itself again, so `publish.sh` refuses to build one; don't work around that.
- A build updates itself only when `RUSTEXPLORER_BUILD` was set at build time. A plain `cargo build` never does, so a local test never replaces the program under test.
- The updater (`crates/rx-update`) accepts plain HTTP only from Tailscale ranges and localhost, and follows redirects only from HTTPS to HTTPS. Don't loosen either: GitHub's `releases/latest/download` redirects twice.

## Never

- Never commit addresses of private machines or networks, user names or paths, keys or tokens. This repository is public.
- Never put the archive password on a command line. It travels in `RX_PW`.
- Never kill processes by name. The installed copy, its update guard (`--update-guard`) and the copy under test may all be running.
- Never call the installed Everything (SDK, IPC or `es.exe`) for search. "Search everywhere" uses its own index (`rx_core::index`), loaded only on demand and dropped from RAM after use.

## Owner habits (keep them through any UX cleanup)

- Files created or changed in the last 3 hours show green text (`RECENT_SECS`). A badge or a coloured date alone is not enough.
- The status bar always shows the number of hidden files, in red while some are hidden; one click toggles them.
- "Create .bak copy" stays in the right-click menu and on `Ctrl+B`, not on the toolbar.

## Traps

- Cloud-file attributes differ by reader. PowerShell reports flags (reparse 0x400, offline 0x1000) that the app's `std::fs` never sees. Measure with a Rust probe: a menu entry keyed on 0x400 shipped invisible.
