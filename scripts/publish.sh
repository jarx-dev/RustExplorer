#!/usr/bin/env bash
# Builds the Windows EXE on Linux (cargo-xwin) and publishes it for the app's self-updater
# (crates/rx-update): <dir>/RustExplorer-<build>.exe + <dir>/latest.json. Serve <dir> with any
# static file server on a Tailscale network (plain HTTP is accepted there) or over HTTPS, so that
# <base>/latest.json is reachable; running copies pick a new build up within about a minute and
# swap it in when the app is minimized, behind other windows for 2 minutes or untouched for 10.
#
#   scripts/publish.sh
#
# Env:
#   RUSTEXPLORER_UPDATE_BASE  URL where <dir> is served (required; built into the EXE)
#   RUSTEXPLORER_UPDATE_DIR   where builds go (default ~/updates/rustexplorer)
#   KEEP                      how many builds to keep (default 5)
# Settings that are this machine's own (addresses in the private network) live outside the
# repository, in ~/.config/rustexplorer/build.env (shell variables; RUSTEXPLORER_CONF points
# elsewhere). GitHub releases (.github/workflows/release.yml) are built separately.
# Needs cargo-xwin and jq.
# scripts/auto-publish.sh runs this for every new commit on main.
set -euo pipefail

for tool in cargo-xwin jq; do
  command -v "$tool" >/dev/null || { echo "publish.sh: brak narzędzia $tool" >&2; exit 1; }
done

crate=$(cd "$(dirname "$0")/.." && pwd)
conf=${RUSTEXPLORER_CONF:-${XDG_CONFIG_HOME:-$HOME/.config}/rustexplorer/build.env}
if [[ -f $conf ]]; then set -a; . "$conf"; set +a; fi
out=${RUSTEXPLORER_UPDATE_DIR:-$HOME/updates/rustexplorer}
keep=${KEEP:-5}
target=x86_64-pc-windows-msvc

# where the published EXE looks for newer builds (a build without it never updates itself)
url_base=${RUSTEXPLORER_UPDATE_BASE:?brak RUSTEXPLORER_UPDATE_BASE (plik $conf)}
# the updater (crates/rx-update) accepts plain HTTP only from Tailscale or localhost; anything else
# would ship an EXE that never updates itself
tailnet='^http://(localhost|127\.[0-9.]+|100\.(6[4-9]|[7-9][0-9]|1[01][0-9]|12[0-7])\.[0-9]+\.[0-9]+|[^/:]+\.ts\.net|\[fd7a:115c:a1e0:[^]]*\])(:[0-9]+)?(/|$)'
[[ $url_base =~ ^https:// || $url_base =~ $tailnet ]] || {
  echo "publish.sh: $url_base — zwykły http:// działa tylko w sieci Tailscale lub na localhost; użyj https://" >&2
  exit 1
}

cd "$crate"
built_at=$(date +%s)
sha=$(git rev-parse --short HEAD)
git diff --quiet HEAD || sha="$sha-dirty"
export RUSTEXPLORER_BUILT_AT=$built_at RUSTEXPLORER_BUILD="$sha-$built_at"
export RUSTEXPLORER_UPDATE_URL="$url_base/latest.json"
notes=$(git log -1 --format=%s)

nice -n 10 cargo xwin build --release --target "$target"

# cargo may put the output elsewhere (CARGO_TARGET_DIR, build.target-dir)
exe="$(cargo metadata --format-version 1 --no-deps | jq -r .target_directory)/$target/release/rust-explorer.exe"
[[ -f $exe ]] || { echo "brak $exe" >&2; exit 1; }

mkdir -p "$out"
name="RustExplorer-$RUSTEXPLORER_BUILD.exe"
cp "$exe" "$out/$name.part" && mv "$out/$name.part" "$out/$name"
sum=$(sha256sum "$out/$name" | cut -d' ' -f1)
size=$(stat -c %s "$out/$name")
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | tr -d '\r' | head -1)
jq -n --arg version "$version" --arg build "$RUSTEXPLORER_BUILD" --argjson built_at "$built_at" \
  --arg sha256 "$sum" --arg url "$name" --argjson size "$size" --arg notes "$notes" \
  '{version:$version, build:$build, built_at:$built_at, sha256:$sha256, url:$url, size:$size, notes:$notes}' \
  > "$out/latest.json.part"
mv "$out/latest.json.part" "$out/latest.json"

# keep the newest $keep builds
ls -1t "$out"/RustExplorer-*.exe 2>/dev/null | tail -n +"$((keep + 1))" | while read -r old; do rm -f -- "$old"; done

echo "opublikowano $name ($((size / 1024)) KiB) → $url_base/$name"
