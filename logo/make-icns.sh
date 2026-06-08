#!/usr/bin/env bash
# Regenerate logo/chatbotchat.icns from the master icon PNG (macOS only).
#
#   ./logo/make-icns.sh
#
# Source is logo/chatbotchat-logo-icon-transparent.png (currently 228x228), so
# the 512/1024 slices are upscaled and will look soft — drop in a >=1024px
# master and rerun for crisp large sizes.
set -euo pipefail

cd "$(dirname "$0")/.."
SRC="logo/chatbotchat-logo-icon-transparent.png"
OUT="logo/chatbotchat.icns"
ICONSET="$(mktemp -d)/chatbotchat.iconset"
mkdir -p "$ICONSET"

# size  iconset-name   (macOS standard set; @2x is twice the base dimension)
specs=(
  "16   icon_16x16"
  "32   icon_16x16@2x"
  "32   icon_32x32"
  "64   icon_32x32@2x"
  "128  icon_128x128"
  "256  icon_128x128@2x"
  "256  icon_256x256"
  "512  icon_256x256@2x"
  "512  icon_512x512"
  "1024 icon_512x512@2x"
)
for spec in "${specs[@]}"; do
  read -r px name <<<"$spec"
  sips -z "$px" "$px" "$SRC" --out "$ICONSET/$name.png" >/dev/null
done

iconutil -c icns "$ICONSET" -o "$OUT"
echo "wrote $OUT"
