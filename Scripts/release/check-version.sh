#!/bin/sh
# Usage: check-version.sh 0.1.0
# Fails unless the release version is the one in the workspace Cargo.toml.
set -eu

want="$1"
have="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
if [ "$have" != "$want" ]; then
  echo "The tag is $want but Cargo.toml says $have" >&2
  exit 1
fi
