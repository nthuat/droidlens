#!/usr/bin/env bash
# Download the integration-test fixtures into .fixtures/ and verify their SHA-256.
# Files already present with the right hash are kept.
set -euo pipefail

dir="$(cd "$(dirname "$0")/.." && pwd)/.fixtures"
mkdir -p "$dir"

sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

fetch() {
  local name=$1 url=$2 want=$3 path="$dir/$1"
  if [[ -f $path && $(sha256 "$path") == "$want" ]]; then
    echo "ok       $name"
    return
  fi
  echo "download $name"
  curl -fsSL --retry 3 -o "$path.part" "$url"
  local got
  got=$(sha256 "$path.part")
  if [[ $got != "$want" ]]; then
    echo "sha256 mismatch for $name: got $got, want $want" >&2
    rm -f "$path.part"
    exit 1
  fi
  mv "$path.part" "$path"
}

fetch Telegram-X-0.29.1.1817.apk \
  https://github.com/TGX-Android/Telegram-X/releases/download/v0.29.1.1817/Telegram-X-0.29.1.1817.apk \
  28b0cf4008329355f271e4780cbabde30b41938d97df800f1cd0c62226c4f56c
fetch NewPipe_v0.29.1.apk \
  https://github.com/TeamNewPipe/NewPipe/releases/download/v0.29.1/NewPipe_v0.29.1.apk \
  18447bfb1e06d113edc88df93f471827280de06f6f3d4dc42f56f28b9c1bab79
fetch jadx-1.5.6.zip \
  https://github.com/skylot/jadx/releases/download/v1.5.6/jadx-1.5.6.zip \
  545ea2be9c242511bc145755cf4bda2485ade42966e096f8b4d3da2a230e8974

if [[ ! -f $dir/jadx-1.5.6-all.jar ]]; then
  unzip -p "$dir/jadx-1.5.6.zip" lib/jadx-1.5.6-all.jar > "$dir/jadx-1.5.6-all.jar.part"
  mv "$dir/jadx-1.5.6-all.jar.part" "$dir/jadx-1.5.6-all.jar"
fi
echo "fixtures ready in $dir"
