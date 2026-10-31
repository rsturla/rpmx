#!/bin/sh
# Check that `rpmx root export` produces a document upstream libpkgmanifest
# accepts. Requires podman and network access to install python3-libpkgmanifest.
set -eu

lock=${1:?usage: check-pkgmanifest-conformance.sh ROOT_LOCK [ARCH...]}
shift

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
rpmx=${RPMX:-$root/target/debug/rpmx}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

"$rpmx" root export "$lock" --output "$work/manifest.yaml"

podman run --rm \
    -v "$work:/w:Z" \
    -v "$root/scripts:/s:Z" \
    registry.fedoraproject.org/fedora:44 \
    sh -c 'dnf -q -y install python3-libpkgmanifest >/dev/null && python3 /s/pkgmanifest-conformance.py /w/manifest.yaml "$@"' \
    sh "$@"
