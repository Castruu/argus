#!/bin/sh
# Builds ccs and the notch app into ./build
set -e
cd "$(dirname "$0")"
cargo build --release -q
mkdir -p build
cp target/release/ccs notch/focus.applescript build/
swiftc -O -parse-as-library notch/Notch.swift -o build/notch
echo "built: build/ccs build/notch"
