#!/bin/sh
set -eu

manifest="$(mktemp)"
trap 'rm -f "$manifest"' EXIT HUP INT TERM

cargo package --list --allow-dirty >"$manifest"

require() {
  if ! grep -Fqx "$1" "$manifest"; then
    echo "package is missing required path: $1" >&2
    exit 1
  fi
}

require Cargo.toml
require Cargo.lock
require rust-toolchain.toml
require README.md
require LICENSE
require src/main.rs
require src/lib.rs
require tests/fixtures/graphite/supported-schema.sql
require tests/fixtures/github/pr-list.json
require benches/responsiveness.rs
require benches/fixture_builder.rs

if grep -Eq '(^|/)(PLAN\.md|memory\.md|changelog\.md|docs/plans/|\.context/|target/)' "$manifest"; then
  echo "package contains an internal or generated path" >&2
  grep -E '(^|/)(PLAN\.md|memory\.md|changelog\.md|docs/plans/|\.context/|target/)' "$manifest" >&2
  exit 1
fi

echo "package contents match the reviewed allowlist"
