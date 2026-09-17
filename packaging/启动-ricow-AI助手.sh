#!/bin/sh
# ===========================================================
#  ricow - Linux / macOS launcher (FR-039)
#
#  Usage, from a terminal:
#      ./启动-ricow-AI助手.sh
#  On macOS you can also double-click the sibling file
#  启动-ricow-AI助手.command, which just runs this script.
#
#  The body is deliberately ASCII-only and does NOT touch the locale:
#  every Chinese line the user reads is printed by ricow itself, and a
#  UTF-8 locale is the Unix default anyway. (The Windows entry runs
#  "chcp 65001" because the legacy console code page really is GBK/936
#  by default; there is nothing like that to fix here, and forcing a
#  locale onto a console that cannot render it would only make it worse.)
#
#  Run it with NO arguments (the bare entry, commands/chat.rs). The bare
#  entry asks for whatever is still missing -- provider, API key, Binance
#  demo credentials -- in a first-run wizard, saves them into ricow.toml
#  itself, and only then opens the chat. That is the whole point of this
#  launcher: the user has to remember nothing.
#  "ricow ai" is deliberately NOT used here: it skips the wizard and fails
#  outright with "auth error: [ai].api_key is not set yet".
#
#  Which binary to run, in order:
#    1. ./ricow            - a release archive. This file is shipped inside
#                            every Unix archive next to the binary (see the
#                            "include" key in dist-workspace.toml). The msi
#                            and the Homebrew formula carry the binary only.
#    2. the build output in ../target - a source checkout. Both
#                            target/release/ricow ("cargo build -p ricow
#                            --release") and target/debug/ricow ("cargo build
#                            -p ricow") may be present, and they go stale
#                            independently, so we run whichever was built last.
#    3. ricow on PATH      - a normal install.
#  With (2) we also cd to the repository root: ricow takes its data
#  directory from the current directory when that directory holds
#  ricow.db or strategies/, and a checkout keeps both at the root.
#  Running from packaging/ instead would silently use (or create) a
#  second, empty data directory.
#
#  FR-058: on macOS a binary extracted from a browser download carries the
#  "com.apple.quarantine" attribute, and Gatekeeper may then refuse to
#  start it ("cannot be opened because the developer cannot be verified").
#  We check for the attribute and print the way out, instead of letting the
#  user stare at a silent failure. Plain tar.xz on Linux has no such mark.
# ===========================================================

set -u

dir=$(CDPATH= cd "$(dirname "$0")" && pwd)

not_executable() {
  printf '%s\n' "[ricow] Error: $1 exists but is not executable."
  printf '%s\n' "[ricow] Archives keep the mode; if it was lost on the way, restore it once:"
  printf '%s\n' "[ricow]   chmod +x \"$1\""
  exit 1
}

ricow_bin=
run_dir=$dir

if [ -f "$dir/ricow" ]; then
  [ -x "$dir/ricow" ] || not_executable "$dir/ricow"
  ricow_bin="$dir/ricow"
elif [ -f "$dir/../Cargo.toml" ]; then
  run_dir=$(CDPATH= cd "$dir/.." && pwd)
  rel=$run_dir/target/release/ricow
  dbg=$run_dir/target/debug/ricow
  # A checkout can hold both builds and they go stale independently, so take
  # whichever is newer: a release binary from before today's source would
  # start and then fail on a config key it has not learned yet.
  if [ -f "$rel" ]; then
    ricow_bin=$rel
  fi
  if [ -f "$dbg" ] && { [ -z "$ricow_bin" ] || [ "$dbg" -nt "$rel" ]; }; then
    ricow_bin=$dbg
  fi
  if [ -n "$ricow_bin" ]; then
    [ -x "$ricow_bin" ] || not_executable "$ricow_bin"
  fi
fi

if [ -z "$ricow_bin" ] && command -v ricow >/dev/null 2>&1; then
  ricow_bin=$(command -v ricow)
fi

if [ -z "$ricow_bin" ]; then
  printf '%s\n' "[ricow] Error: no ricow binary found."
  printf '%s\n' "[ricow] Looked: next to this script, ../target/release and ../target/debug,"
  printf '%s\n' "[ricow] and every ricow on PATH."
  printf '%s\n' "[ricow] From a source checkout, build it once and run this script again:"
  printf '%s\n' "[ricow]   cargo build -p ricow"
  exit 1
fi

cd "$run_dir" || {
  printf '%s\n' "[ricow] Error: cannot enter $run_dir"
  exit 1
}

if command -v xattr >/dev/null 2>&1 &&
  xattr "$ricow_bin" 2>/dev/null | grep -q 'com\.apple\.quarantine'; then
  printf '%s\n' "[ricow] Note: ricow still carries the \"com.apple.quarantine\" attribute."
  printf '%s\n' "[ricow] If macOS says \"cannot be opened because the developer cannot be verified\","
  printf '%s\n' "[ricow] clear the attribute once for the folder holding the binary:"
  printf '%s\n' "[ricow]   xattr -dr com.apple.quarantine \"$(dirname "$ricow_bin")\""
  printf '\n'
fi

# Bare entry: no arguments -> first-run wizard (if needed) -> chat.
"$ricow_bin"

printf '\n%s\n' "[ricow] session ended."
# A Terminal window opened by a double-click closes as soon as the shell
# exits, so wait for the user. Interactive shells have a tty on stdin;
# piped runs do not and are left alone.
if [ -t 0 ]; then
  printf '%s' "[ricow] press Enter to close... "
  read -r _
fi
