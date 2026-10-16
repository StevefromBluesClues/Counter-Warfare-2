#!/usr/bin/env bash
# Builds dist/counter-warfare-2-windows.zip from target/play/iw4l.exe.
# Usage: scripts/package-windows.sh [repo root]
set -euo pipefail
root="${1:-$(cd "$(dirname "$0")/.." && pwd)}"
out="$root/dist/counter-warfare-2-windows"
rm -rf "$out" "$root/dist/counter-warfare-2-windows.zip"
mkdir -p "$out/licenses"
cp "$root/target/play/iw4l.exe" "$out/counter-warfare2.exe"
cp "$root/LICENSE" "$root/NOTICE" "$root/THIRD-PARTY-LICENSES.txt" "$root/CHANGES.md" "$out/"
cp "$root/crates/ui/assets/OFL-Oxanium.txt" "$root/crates/console/assets/OFL-FiraMono.txt" "$out/licenses/"
cp "$root/docs/BOMB-DEFUSAL.md" "$out/"
cat > "$out/README.txt" <<'EOF'
Counter-Warfare 2 - Counter-Strike style Bomb Defusal for Modern Warfare 2 (2009)

1. You need your own installed copy of Call of Duty: Modern Warfare 2.
   This archive contains no game files.
2. Run counter-warfare2.exe. It finds a Steam install of MW2 on its own and
   creates a "Modern Warfare 2" shortcut next to itself. For an install
   outside Steam, make that shortcut to your MW2 folder yourself.
3. Pick COMPETITIVE or CASUAL BOMB DEFUSAL from the game modes. Press B to buy.

Rules: BOMB-DEFUSAL.md. Based on IW4L by vladtrc (https://github.com/vladtrc/iw4L).
Unofficial fan project, not affiliated with Activision, Infinity Ward or Valve.
EOF
(cd "$out" && "$SYSTEMROOT/System32/tar.exe" -a -cf ../counter-warfare-2-windows.zip *)
ls -la "$root/dist"
