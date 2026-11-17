#!/bin/sh
# Builds ccs and the notch app into ./build
set -e
cd "$(dirname "$0")"
cargo build --locked --release -q
mkdir -p build
cp target/release/ccs notch/*.applescript build/
cp notch/ProviderIcons-LICENSE build/
swiftc -target "$(uname -m)-apple-macosx14.0" -O -parse-as-library notch/*.swift -o build/notch
echo "built: build/ccs build/notch"
