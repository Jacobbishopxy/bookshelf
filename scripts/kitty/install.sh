#!/usr/bin/env bash
set -euo pipefail

if ! command -v curl >/dev/null 2>&1; then
  echo "need curl" >&2
  exit 1
fi

# Official installer: https://sw.kovidgoyal.net/kitty/binary/
curl -L https://sw.kovidgoyal.net/kitty/installer.sh | sh /dev/stdin

mkdir -p "${HOME}/.local/bin"
ln -sf "${HOME}/.local/kitty.app/bin/kitty" "${HOME}/.local/bin/kitty"

cat <<'EOF'

Installed kitty under: ~/.local/kitty.app/

Symlinked kitty into: ~/.local/bin/kitty

Make sure ~/.local/bin is on PATH (bash/zsh):
  export PATH="$HOME/.local/bin:$PATH"
EOF
