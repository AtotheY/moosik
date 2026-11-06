#!/usr/bin/env bash
# Install (or remove) a systemd user timer that runs `just api` daily, so YouTube breakage
# pages us before users report it. See M14-plan.md ("Upstream posture").
set -euo pipefail

unit_dir="$HOME/.config/systemd/user"
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
    echo "usage: canary-install.sh [install|uninstall]" >&2
    exit 1
}

install_units() {
    just_bin="$(command -v just)" || {
        echo "just is not installed; cannot schedule the canary" >&2
        exit 1
    }
    mkdir -p "$unit_dir"
    cat > "$unit_dir/moosik-api.service" <<EOF
[Unit]
Description=moosik API canary (search, mix, library, home)
After=network-online.target
Wants=network-online.target

[Service]
Type=oneshot
WorkingDirectory=$root
ExecStart=$just_bin api
EOF
    cat > "$unit_dir/moosik-api.timer" <<EOF
[Unit]
Description=Run the moosik API canary daily

[Timer]
OnCalendar=daily
RandomizedDelaySec=1h
Persistent=true

[Install]
WantedBy=timers.target
EOF
    systemctl --user daemon-reload
    systemctl --user enable --now moosik-api.timer
    echo "canary scheduled daily (moosik-api.timer)"
}

uninstall_units() {
    systemctl --user disable --now moosik-api.timer 2>/dev/null || true
    rm -f "$unit_dir/moosik-api.timer" "$unit_dir/moosik-api.service"
    systemctl --user daemon-reload
    echo "canary removed"
}

case "${1:-install}" in
    install) install_units ;;
    uninstall) uninstall_units ;;
    *) usage ;;
esac
