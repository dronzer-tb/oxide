#!/bin/sh
# Root part of the agent host on oci-v3. Run by install.sh as: ssh oci-v3 sudo sh -s
# Idempotent. Creates the unprivileged user `agents` (no sudo), installs ttyd, two systemd
# units, and the nginx locations for the control page's terminals.
set -eu
TTYD_VERSION=1.7.7
OWNER_EMAIL="$1"

id agents >/dev/null 2>&1 || useradd -m -s /bin/bash agents
install -d -o agents -g agents /srv/oxide-lab/agents

if ! /usr/local/bin/ttyd --version 2>/dev/null | grep -q "$TTYD_VERSION"; then
    tmp="$(mktemp -d)"
    base="https://github.com/tsl0922/ttyd/releases/download/$TTYD_VERSION"
    curl -fsSL -o "$tmp/ttyd.aarch64" "$base/ttyd.aarch64"
    curl -fsSL -o "$tmp/SHA256SUMS" "$base/SHA256SUMS"
    (cd "$tmp" && grep ' ttyd.aarch64$' SHA256SUMS | sha256sum -c -)
    install -m 755 "$tmp/ttyd.aarch64" /usr/local/bin/ttyd
    rm -rf "$tmp"
fi

cat >/etc/systemd/system/oxide-agents.service <<UNIT
[Unit]
Description=Oxide perf track sessions (tmux, one per track)
After=network-online.target

[Service]
User=agents
ExecStart=/home/agents/bin/agent-supervisor
Restart=always
RestartSec=10
# Keep the graph portal and OmniRoute on this box alive whatever the builds do.
MemoryMax=7G
CPUWeight=50
KillMode=control-group

[Install]
WantedBy=multi-user.target
UNIT

cat >/etc/systemd/system/oxide-agents-term.service <<UNIT
[Unit]
Description=Web terminals for the Oxide perf tracks
After=oxide-agents.service

[Service]
User=agents
ExecStart=/usr/local/bin/ttyd -i lo -p 7681 -b /lab/term -W -a -t titleFixed=oxide /home/agents/bin/agent-term
Restart=always

[Install]
WantedBy=multi-user.target
UNIT

site=/etc/nginx/sites-available/graph.tharax.me
if ! grep -q "auth_owner" "$site"; then
    cp -p "$site" "/etc/nginx/graph.tharax.me.bak-agents"
    python3 - "$site" "$OWNER_EMAIL" <<'PY'
import sys
path, owner = sys.argv[1], sys.argv[2]
s = open(path).read()
mark = "    # Oxide lab (static): same GitHub login as the portal\n"
assert mark in s
block = f"""    # Web terminals into the agent sessions: one GitHub account only, not the whole org.
    location = /oauth2/auth_owner {{
        internal;
        proxy_pass http://127.0.0.1:4180/oauth2/auth?allowed_emails={owner};
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto https;
        proxy_set_header Content-Length "";
        proxy_pass_request_body off;
    }}
    location /lab/term/ {{
        auth_request /oauth2/auth_owner;
        error_page 401 = /oauth2/sign_in;
        proxy_pass http://127.0.0.1:7681;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_read_timeout 1d;
    }}

"""
open(path, "w").write(s.replace(mark, block + mark))
PY
fi
nginx -t
systemctl reload nginx
systemctl daemon-reload
