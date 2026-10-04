#!/usr/bin/env bash
# Build this checkout's Herdr and install it as `herdr` on PATH.
#
# Steps: build with `cargo build --release --locked`, copy the binary to
# $HERDR_INSTALL_DIR/herdr-<short sha> (default ~/.local/opt/herdr), hand a
# running server over to it with `server live-handoff`, then point
# $HERDR_BIN_DIR/herdr (default ~/.local/bin) at it. With no server running
# the handoff is skipped. A failed handoff leaves the old link in place.
#
# Takes no arguments. Exits 0 on success, non-zero with a one-line reason.
set -euo pipefail

die() {
  echo "install-local: $*" >&2
  exit 1
}

[ "$#" -eq 0 ] || die "takes no arguments"

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)
install_dir=${HERDR_INSTALL_DIR:-$HOME/.local/opt/herdr}
bin_dir=${HERDR_BIN_DIR:-$HOME/.local/bin}

command -v git >/dev/null 2>&1 || die "needs git"
sha=$(git -C "$repo" rev-parse --short=8 HEAD 2>/dev/null) || die "cannot read the commit of $repo"
name="herdr-$sha"
# A build from uncommitted changes is not the commit its sha names: give it its
# own name and never reuse an earlier copy for it.
if [ -n "$(git -C "$repo" status --porcelain --untracked-files=no)" ]; then
  name="$name-dirty"
fi
new_bin="$install_dir/$name"

if [ "${name%-dirty}" != "$name" ] || [ ! -x "$new_bin" ]; then
  command -v cargo >/dev/null 2>&1 || die "needs cargo (install Rust from https://rustup.rs)"
  # The vendored libghostty-vt build runs $ZIG or zig without checking its version
  # (crates/ghostty-vt/build.rs), and CI pins 0.16.0.
  zig=${ZIG:-zig}
  zig_version=$("$zig" version 2>/dev/null) || die "needs Zig 0.16.0 (install it, or set ZIG to its path)"
  [ "$zig_version" = "0.16.0" ] || die "needs Zig 0.16.0, found $zig_version ($zig)"

  build_log=$(mktemp "${TMPDIR:-/tmp}/herdr-install-build.XXXXXX")
  if ! (cd "$repo" && cargo build --release --locked) >"$build_log" 2>&1; then
    die "cargo build --release --locked failed, see $build_log"
  fi
  rm -f "$build_log"
  target_dir=${CARGO_TARGET_DIR:-target}
  case "$target_dir" in
    /*) built="$target_dir/release/herdr" ;;
    *) built="$repo/$target_dir/release/herdr" ;;
  esac
  [ -x "$built" ] || die "the build made no binary at $built"

  mkdir -p "$install_dir" || die "cannot create $install_dir"
  if ! { cp "$built" "$new_bin.tmp" && chmod 755 "$new_bin.tmp" && mv -f "$new_bin.tmp" "$new_bin"; }; then
    die "cannot copy the binary to $new_bin"
  fi
fi

mkdir -p "$bin_dir" || die "cannot create $bin_dir"
link="$bin_dir/herdr"
current=$(readlink "$link" 2>/dev/null || true)

# Ask the new binary whether a server answers on the socket it resolves; a stale
# socket file left by a crash reads as not running (`herdr status server`). A
# link already on this clean build means the server was handed to it before.
if [ "$current" != "$new_bin" ] || [ "${name%-dirty}" != "$name" ]; then
  if ! status=$(env -u HERDR_SOCKET_PATH -u HERDR_CLIENT_SOCKET_PATH "$new_bin" status server --json 2>&1); then
    die "cannot tell whether a Herdr server runs: $(printf '%s' "$status" | head -n 1)"
  fi
  case "$status" in
    *'"running":true'*)
      mkdir -p "$install_dir/logs" || die "cannot create $install_dir/logs"
      handoff_log="$install_dir/logs/handoff-$name.log"
      if ! env -u HERDR_SOCKET_PATH -u HERDR_CLIENT_SOCKET_PATH \
        "$new_bin" server live-handoff --import-exe "$new_bin" >>"$handoff_log" 2>&1; then
        die "live handoff failed, see $handoff_log; $link is unchanged"
      fi
      ;;
    *'"running":false'*) ;;
    *) die "cannot read the server status: $(printf '%s' "$status" | head -n 1)" ;;
  esac
fi

ln -sfn "$new_bin" "$link" || die "cannot link $link to $new_bin"
echo "installed $new_bin as $link"
