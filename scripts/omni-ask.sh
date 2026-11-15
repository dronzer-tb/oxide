#!/bin/sh
# Asks a model on OmniRoute one question and prints its answer: a second opinion or a review
# from outside the session, for the price of no Claude tokens.
#
#   scripts/omni-ask.sh <prompt-file> [model]
#   echo "question" | scripts/omni-ask.sh - [model]
#
# This is a consult, not an agent: the model sees only the text you send, cannot read the
# repository and cannot run anything. Paste the code it needs into the prompt. Never send
# anything from reference/ (game data) or any secret.
#
# Key: $OMNI_KEY, or the first line of ~/.config/omniroute/key. Host: $OMNI_URL.
# Models that answered real requests when last checked (2026-11-11): kimi-web/k3,
# deepseek-web/deepseek-v4-flash, gemini-pro-latest. Requests are streamed because several
# providers only answer that way.
set -eu
prompt="${1:?usage: omni-ask.sh <prompt-file|-> [model]}"
model="${2:-kimi-web/k3}"
url="${OMNI_URL:-https://omni.tharax.me}"
key="${OMNI_KEY:-$(head -n 1 "$HOME/.config/omniroute/key" 2>/dev/null || true)}"
if [ -z "$key" ]; then
    echo "omni-ask: no key; set OMNI_KEY or write it to ~/.config/omniroute/key" >&2
    exit 2
fi
if [ "$prompt" = "-" ]; then
    prompt=/dev/stdin
fi
node -e '
const fs = require("fs");
const [file, model] = process.argv.slice(1);
process.stdout.write(JSON.stringify({
  model, stream: true,
  messages: [{ role: "user", content: fs.readFileSync(file, "utf8") }],
}));' "$prompt" "$model" |
    curl -sS -N --fail-with-body --max-time 600 "$url/v1/chat/completions" \
        -H "Authorization: Bearer $key" -H "Content-Type: application/json" --data-binary @- |
    node -e '
let buffer = "", wrote = false;
process.stdin.on("data", chunk => {
  buffer += chunk;
  let at;
  while ((at = buffer.indexOf("\n")) >= 0) {
    const line = buffer.slice(0, at).trim();
    buffer = buffer.slice(at + 1);
    if (!line.startsWith("data:") || line.endsWith("[DONE]")) continue;
    try {
      const text = JSON.parse(line.slice(5)).choices?.[0]?.delta?.content;
      if (text) { process.stdout.write(text); wrote = true; }
    } catch {}
  }
});
process.stdin.on("end", () => {
  if (!wrote) { process.stderr.write("omni-ask: no answer: " + buffer.slice(0, 400) + "\n"); process.exit(1); }
  process.stdout.write("\n");
});'
