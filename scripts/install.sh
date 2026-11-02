#!/bin/sh
set -eu

repo="aabolfazl/adbtop"
dest="${ADBTOP_INSTALL_DIR:-$HOME/.local/bin}"

case "$(uname -s)" in
  Darwin) os="apple-darwin" ;;
  Linux) os="unknown-linux-musl" ;;
  *) echo "adbtop: $(uname -s) is not supported; build from source: cargo install adbtop" >&2; exit 1 ;;
esac

case "$(uname -m)" in
  arm64 | aarch64) arch="aarch64" ;;
  x86_64 | amd64) arch="x86_64" ;;
  *) echo "adbtop: $(uname -m) is not supported; build from source: cargo install adbtop" >&2; exit 1 ;;
esac

target="$arch-$os"
url="https://github.com/$repo/releases/latest/download/adbtop-$target.tar.gz"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "downloading $url"
curl -fsSL "$url" -o "$tmp/adbtop.tar.gz"
curl -fsSL "$url.sha256" -o "$tmp/adbtop.tar.gz.sha256"

expected="$(cut -d' ' -f1 < "$tmp/adbtop.tar.gz.sha256")"
if command -v sha256sum > /dev/null 2>&1; then
  actual="$(sha256sum "$tmp/adbtop.tar.gz" | cut -d' ' -f1)"
else
  actual="$(shasum -a 256 "$tmp/adbtop.tar.gz" | cut -d' ' -f1)"
fi
if [ "$expected" != "$actual" ]; then
  echo "adbtop: checksum mismatch, not installing" >&2
  exit 1
fi

tar xzf "$tmp/adbtop.tar.gz" -C "$tmp"
mkdir -p "$dest"
install -m 755 "$tmp/adbtop-$target/adbtop" "$dest/adbtop"
echo "installed $dest/adbtop"

case ":$PATH:" in
  *":$dest:"*) ;;
  *) echo "add $dest to your PATH to run adbtop" ;;
esac
