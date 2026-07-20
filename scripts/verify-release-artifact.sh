#!/bin/sh
set -eu

if [ "$#" -ne 3 ]; then
  echo "usage: $0 ARCHIVE VERSION {arm64|x86_64}" >&2
  exit 2
fi

archive="$1"
version="$2"
architecture="$3"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT HUP INT TERM

case "$archive" in
  *.tar.gz) ;;
  *) echo "release asset must be a .tar.gz archive" >&2; exit 1 ;;
esac

members="$(tar -tzf "$archive")"
expected="stackmap-$version-$architecture/stackmap
stackmap-$version-$architecture/README.md
stackmap-$version-$architecture/LICENSE"
if [ "$members" != "$expected" ]; then
  echo "unexpected archive membership:" >&2
  printf '%s\n' "$members" >&2
  exit 1
fi

tar -xzf "$archive" -C "$work"
root="$work/stackmap-$version-$architecture"
binary="$root/stackmap"
test -x "$binary"
test -s "$root/README.md"
test -s "$root/LICENSE"

case "$architecture" in
  arm64) file "$binary" | grep -Fq 'arm64' ;;
  x86_64) file "$binary" | grep -Fq 'x86_64' ;;
  *) echo "unsupported architecture: $architecture" >&2; exit 2 ;;
esac

"$binary" --version | grep -Fqx "stackmap $version"
otool -l "$binary" | awk '/LC_BUILD_VERSION/{seen=1} seen && /minos/{print $2; exit}' | grep -Eq '^15(\.0+)?$'

echo "verified $(basename "$archive")"
