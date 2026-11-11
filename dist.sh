#!/usr/bin/env bash
# Builds the release archive: a statically linked program that runs on any
# x86_64 Linux distribution, plus the installer and the Dolphin plugin source.
# Needs: rustup target add x86_64-unknown-linux-musl, and musl-gcc.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
cd "$here"
target=x86_64-unknown-linux-musl
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
name="kdlite-$version-x86_64-linux"
out="$here/dist"

# Keep the build machine's home path out of the program.
RUSTFLAGS="--remap-path-prefix=$HOME=~ ${RUSTFLAGS:-}" cargo build --release --target "$target"
rm -rf "$out/$name" && mkdir -p "$out/$name/dolphin"
cp "target/$target/release/kdlite" "$out/$name/kdlite"
strip "$out/$name/kdlite"
cp install.sh README.md README.tr.md LICENSE "$out/$name/"
cp dolphin/CMakeLists.txt dolphin/*.cpp dolphin/*.json "$out/$name/dolphin/"
tar -C "$out" --owner=0 --group=0 -czf "$out/$name.tar.gz" "$name"
(cd "$out" && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256")
echo "Built: $out/$name.tar.gz"
