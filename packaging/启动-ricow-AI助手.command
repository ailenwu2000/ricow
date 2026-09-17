#!/bin/sh
# ===========================================================
#  ricow - macOS double-click entry (FR-039)
#
#  Finder runs a ".command" file in Terminal when you double-click it.
#  This is a shim so the launcher logic lives in exactly one place:
#  启动-ricow-AI助手.sh, shipped in the same archive. That script keeps
#  the window open at the end (it waits for Enter on a tty).
#
#  The file has to be executable for Finder to run it. The archive is
#  built from a git checkout, so if the bit was lost, restore it once:
#      chmod +x 启动-ricow-AI助手.sh 启动-ricow-AI助手.command
#
#  FR-058 (macOS quarantine) is handled inside 启动-ricow-AI助手.sh.
# ===========================================================

set -u

dir=$(CDPATH= cd "$(dirname "$0")" && pwd)
launcher="$dir/启动-ricow-AI助手.sh"

if [ ! -f "$launcher" ]; then
  printf '%s\n' "[ricow] Error: launcher 启动-ricow-AI助手.sh is missing next to this file."
  printf '%s\n' "[ricow] Extract the whole archive again."
  printf '%s' "[ricow] press Enter to close... "
  read -r _
  exit 1
fi

exec "$launcher"
