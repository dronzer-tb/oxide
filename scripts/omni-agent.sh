#!/bin/sh
# Runs a headless Claude Code worker on an OmniRoute model, in the current directory, and
# prints its final answer. Unlike omni-ask.sh this one has tools: it reads files, runs
# commands and edits code. It spends no Claude quota.
#
#   scripts/omni-agent.sh <model> "<task>"
#   e.g. scripts/omni-agent.sh kimi-web/k3 "Profile notes are in /tmp/p.txt. Which ..."
#
# Give it its own tree, never yours: `git worktree add /tmp/omni-<name> HEAD`, run it there,
# then read its diff and take what is good. It runs with permission checks off and its work
# is not trusted: nothing it writes lands without the bench and the digest.
# Free models are slower and weaker at tool use than Claude; give it one narrow job.
#
# Key: $OMNI_KEY, or the first line of ~/.config/omniroute/key. Host: $OMNI_URL.
# Turn limit: $OMNI_MAX_TURNS (default 40).
set -eu
model="${1:?usage: omni-agent.sh <model> \"<task>\"}"
task="${2:?usage: omni-agent.sh <model> \"<task>\"}"
key="${OMNI_KEY:-$(head -n 1 "$HOME/.config/omniroute/key" 2>/dev/null || true)}"
if [ -z "$key" ]; then
    echo "omni-agent: no key; set OMNI_KEY or write it to ~/.config/omniroute/key" >&2
    exit 2
fi
# Its own config dir, so it never touches the account login of the session that started it.
export CLAUDE_CONFIG_DIR="$HOME/.claude-omni"
export ANTHROPIC_BASE_URL="${OMNI_URL:-https://omni.tharax.me}"
export ANTHROPIC_AUTH_TOKEN="$key"
export ANTHROPIC_DEFAULT_HAIKU_MODEL="$model"
unset ANTHROPIC_API_KEY
exec claude -p --model "$model" --dangerously-skip-permissions \
    --max-turns "${OMNI_MAX_TURNS:-40}" "$task"
