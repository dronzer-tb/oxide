#!/bin/sh
# The `agents` user's part on oci-v3. Run by install.sh as: ssh oci-v3 sudo -iu agents sh -s
# Idempotent. Installs Claude Code and Rust in the user's home, makes the ssh key used for the
# repo deploy key and the bench user on s5, and clones the repo with one worktree per track.
set -eu
mkdir -p ~/bin ~/.ssh ~/.local
chmod 700 ~/.ssh
[ -f ~/.ssh/id_ed25519 ] || ssh-keygen -q -t ed25519 -N "" -C "oxide-agents@oci-v3" -f ~/.ssh/id_ed25519
cat >~/.ssh/config <<CFG
Host github.com
    IdentityFile ~/.ssh/id_ed25519
    IdentitiesOnly yes
Host bench-s5
    HostName 62.169.17.163
    User bench
    IdentityFile ~/.ssh/id_ed25519
    IdentitiesOnly yes
CFG
[ -x ~/.local/bin/claude ] || npm install -g --prefix ~/.local @anthropic-ai/claude-code >/dev/null 2>&1
[ -x ~/.cargo/bin/cargo ] || curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal -c rustfmt,clippy >/dev/null 2>&1
[ -f ~/agents.env ] || cat >~/agents.env <<ENV
AGENT_MODEL=claude-fable-5-2
AGENT_SUBAGENT_MODEL=claude-opus-5-5
NUDGE_MIN=20
ENV
git config --global user.name Kasniya
git config --global user.email "$1"
echo "--- public key ---"
cat ~/.ssh/id_ed25519.pub
