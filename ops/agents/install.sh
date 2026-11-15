#!/bin/sh
# Sets up the agent host: three Claude sessions, one per perf track, running on oci-v3 under an
# unprivileged user, benchmarking on s5 as an unprivileged user, watched and logged into from
# https://graph.tharax.me/lab/control.html. Run from the oxide checkout on the laptop:
#
#   ops/agents/install.sh <your GitHub account email>
#
# Needs: ssh hosts oci-v3 (sudo) and s5 (root), and gh logged in with admin on dronzer-tb/oxide.
# Safe to run again.
set -eu
cd "$(dirname "$0")/../.."
owner="${1:?usage: install.sh <github email>}"
ssh="ssh -o BatchMode=yes"

echo "== oci-v3: user, ttyd, units, nginx"
$ssh oci-v3 sudo sh -s "$owner" <ops/agents/setup-oci.sh

echo "== oci-v3: agents user"
rsync -a -e "$ssh" ops/agents/bin/ oci-v3:agents-bin/
$ssh oci-v3 'sudo install -d -o agents -g agents /home/agents/bin && sudo install -o agents -g agents -m 755 ~/agents-bin/* /home/agents/bin/ && rm -rf ~/agents-bin'
key="$($ssh oci-v3 sudo -iu agents sh -s "$owner" <ops/agents/setup-agent-user.sh | sed -n '/--- public key ---/{n;p}')"
[ -n "$key" ] || { echo "no public key from oci-v3" >&2; exit 1; }

echo "== github: deploy key (write, this repo only)"
gh repo deploy-key list -R dronzer-tb/oxide | grep -qF "$(echo "$key" | cut -d' ' -f2 | cut -c1-20)" ||
    { echo "$key" >/tmp/oxide-agents.pub && gh repo deploy-key add /tmp/oxide-agents.pub -R dronzer-tb/oxide -w -t oxide-agents-oci-v3 && rm /tmp/oxide-agents.pub; }

echo "== s5: bench user"
$ssh s5 sh -s "$key" <ops/agents/setup-s5.sh

echo "== oci-v3: repo and worktrees"
$ssh oci-v3 sudo -iu agents sh -s <ops/agents/setup-repo.sh
if [ -d graphify-out ]; then
    rsync -a -e "$ssh" graphify-out/ oci-v3:agents-graph/
    $ssh oci-v3 'for t in noise datapath pipeline; do sudo rsync -a --chown=agents:agents ~/agents-graph/ /home/agents/oxide-wt/$t/graphify-out/; done; rm -rf ~/agents-graph'
fi

echo "== site"
rsync -a --delete --exclude data-local --exclude agents -e "$ssh" website/lab/ oci-v3:oxide-lab/
$ssh oci-v3 'sudo rsync -a --delete --exclude agents ~/oxide-lab/ /srv/oxide-lab/ && sudo chmod -R a+rX /srv/oxide-lab'

echo "== start"
$ssh oci-v3 'sudo systemctl enable --now oxide-agents.service oxide-agents-term.service && sleep 3 && systemctl is-active oxide-agents oxide-agents-term'
echo "Done. Open https://graph.tharax.me/lab/control.html and log each terminal into one account."
