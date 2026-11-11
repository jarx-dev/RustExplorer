#!/usr/bin/env bash
# Publishes every new commit on main for the app's self-updater, so a change reaches the
# installed copies without a manual build. Run every minute by the systemd user timer
# rustexplorer-autopublish: it checks out the local main branch (no remote needed) into a worktree
# of its own (~/rustexplorer-publish, so it never touches the tree being worked in) and, when that
# commit is not the build in latest.json yet, builds and publishes it with scripts/publish.sh.
# A commit whose build failed is not tried again (the next commit on main is).
#
#   RUSTEXPLORER_UPDATE_BASE=... RUSTEXPLORER_UPDATE_DIR=... scripts/auto-publish.sh --install
#   journalctl --user -u rustexplorer-autopublish -f
#
# --install creates the worktree and the timer, and writes the publish variables set at that
# moment (see scripts/publish.sh) into the service, so the timer publishes where you expect.
# Env: RUSTEXPLORER_PUBLISH_TREE (default ~/rustexplorer-publish), RUSTEXPLORER_UPDATE_DIR and
# RUSTEXPLORER_UPDATE_BASE (as publish.sh), GIT (git binary, e.g. to bypass a shell wrapper).
set -euo pipefail

crate=$(cd "$(dirname "$0")/.." && pwd)
repo=$(git -C "$crate" rev-parse --show-toplevel)
# where the crate sits in the repo ("app/" in a larger repo, "" when it is the repo itself)
rel=$(git -C "$crate" rev-parse --show-prefix)
tree=${RUSTEXPLORER_PUBLISH_TREE:-$HOME/rustexplorer-publish}
out=${RUSTEXPLORER_UPDATE_DIR:-$HOME/updates/rustexplorer}
failed=${XDG_STATE_HOME:-$HOME/.local/state}/rustexplorer-autopublish-failed

if [[ ${1:-} == --install ]]; then
  [[ -d $tree ]] || "${GIT:-git}" -C "$repo" worktree add -q --detach "$tree" main
  env_lines=""
  for v in RUSTEXPLORER_UPDATE_BASE RUSTEXPLORER_UPDATE_DIR RUSTEXPLORER_PUBLISH_TREE; do
    [[ -n ${!v:-} ]] && env_lines+="Environment=$v=${!v}"$'\n'
  done
  units=$HOME/.config/systemd/user
  mkdir -p "$units"
  cat > "$units/rustexplorer-autopublish.service" <<EOF
[Unit]
Description=RustExplorer: build and publish new commits on main (scripts/auto-publish.sh)
After=network-online.target

[Service]
Type=oneshot
Environment=PATH=%h/.cargo/bin:/usr/local/bin:/usr/bin:/bin
${env_lines}ExecStart=$tree/${rel}scripts/auto-publish.sh
Nice=10
EOF
  cat > "$units/rustexplorer-autopublish.timer" <<'EOF'
[Unit]
Description=RustExplorer: check main for a new build every minute

[Timer]
OnBootSec=2min
OnUnitInactiveSec=1min

[Install]
WantedBy=timers.target
EOF
  export XDG_RUNTIME_DIR=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
  systemctl --user daemon-reload
  systemctl --user enable --now rustexplorer-autopublish.timer
  echo "zainstalowano: $tree, rustexplorer-autopublish.timer"
  # a user timer stops with the user's last session unless lingering is on (headless servers)
  [[ $(loginctl show-user "$USER" -p Linger 2>/dev/null) == Linger=yes ]] ||
    echo "uwaga: timer użytkownika zatrzyma się po wylogowaniu; włącz: loginctl enable-linger $USER" >&2
  exit 0
fi

cd "$tree"
sha=$(git rev-parse --short main)
published=$(jq -r '.build' "$out/latest.json" 2>/dev/null | cut -d- -f1 || true)
[[ $sha == "$published" ]] && exit 0
[[ -f $failed && $(cat "$failed") == "$sha" ]] && exit 0
git checkout -q --detach main
echo "nowy commit na main: $sha (opublikowany: ${published:-brak}), buduję"
if ! "${rel}scripts/publish.sh"; then
  mkdir -p "$(dirname "$failed")"
  echo "$sha" > "$failed"
  echo "build $sha nieudany, nie będzie ponawiany" >&2
  exit 1
fi
