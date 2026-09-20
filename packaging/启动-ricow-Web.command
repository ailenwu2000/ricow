#!/bin/sh
# ===========================================================
#  ricow - macOS Web UI double-click entry (025 FR-033)
#
#  Finder runs a ".command" file in Terminal when you double-click it.
#  This is a shim so the launcher logic lives in exactly one place:
#  启动-ricow-Web.sh, shipped in the same archive. That script keeps
#  the window open at the end (it waits for Enter on a tty).
#
#  The file has to be executable for Finder to run it. The archive is
#  built from a git checkout, so if the bit was lost, restore it once:
#      chmod +x 启动-ricow-Web.sh 启动-ricow-Web.command
#
#  FR-058 (macOS quarantine) is handled inside 启动-ricow-Web.sh.
# ===========================================================

set -u

dir=$(CDPATH= cd "$(dirname "$0")" && pwd)
launcher="$dir/启动-ricow-Web.sh"

if [ ! -f "$launcher" ]; then
  printf '%s\n' "[ricow] Error: launcher 启动-ricow-Web.sh is missing next to this file."
  printf '%s\n' "[ricow] Extract the whole archive again."
  printf '%s' "[ricow] press Enter to close... "
  read -r _
  exit 1
fi

exec "$launcher"
