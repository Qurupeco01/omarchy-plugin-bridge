#!/bin/sh
# opb installer — downloads the latest release binary from GitHub and puts it
# in ~/.local/bin (override with OPB_INSTALL_DIR).
#
#   curl -fsSL https://raw.githubusercontent.com/Qurupeco01/omarchy-plugin-bridge/main/install.sh | sh
#   VERSION=v0.1.0 sh install.sh          # pin a version
#
# Verifies the sha256 checksum before installing. Needs: curl, tar, sha256sum.
set -eu

REPO="Qurupeco01/omarchy-plugin-bridge"
TARGET="x86_64-unknown-linux-gnu"
INSTALL_DIR="${OPB_INSTALL_DIR:-$HOME/.local/bin}"

[ "$(uname -s)" = "Linux" ] || { echo "error: prebuilt binaries are Linux-only; build from source (see README)" >&2; exit 1; }
[ "$(uname -m)" = "x86_64" ] || { echo "error: only x86_64 binaries are published; build from source (see README)" >&2; exit 1; }

if [ -n "${VERSION:-}" ]; then
    tag="$VERSION"
else
    echo "fetching the latest release tag…"
    tag=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
        | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p')
    [ -n "$tag" ] || { echo "error: could not determine the latest release" >&2; exit 1; }
fi
echo "installing opb $tag"

version="${tag#v}"
asset="omarchy-plugin-bridge-${version}-${TARGET}.tar.gz"
base_url="https://github.com/$REPO/releases/download/$tag"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

curl -fsSL -o "$tmp/$asset" "$base_url/$asset"
curl -fsSL -o "$tmp/$asset.sha256" "$base_url/$asset.sha256"

(cd "$tmp" && sha256sum -c "$asset.sha256") \
    || { echo "error: checksum mismatch — aborting" >&2; exit 1; }

mkdir -p "$INSTALL_DIR"
tar -xzf "$tmp/$asset" -C "$tmp"
mv "$tmp/omarchy-plugin-bridge-${version}-${TARGET}/opb" "$INSTALL_DIR/opb"

case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *) echo "note: $INSTALL_DIR is not on your PATH — add it to your shell profile:" 
       echo "      export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac

echo "installed: $INSTALL_DIR/opb — run \`opb bootstrap\` to get started"
