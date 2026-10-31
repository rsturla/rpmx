#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

cargo run --release -- demo.src.rpm >/dev/null

test "$(rpm -qp --qf '%{SOURCEPACKAGE}' demo.src.rpm)" = "1"
test -z "$(rpm -qp --qf '[%{SOURCERPM}]' demo.src.rpm)"
test "$(rpm -qp --qf '%{PAYLOADFORMAT}' demo.src.rpm)" = "cpio"

rpm -Kv demo.src.rpm | grep -q 'digest: OK'

files="$(rpm2cpio demo.src.rpm | cpio -it 2>/dev/null | sort)"
printf '%s\n' "$files" | grep -qx './package.yaml'
printf '%s\n' "$files" | grep -qx './package.lock.json'
printf '%s\n' "$files" | grep -qx './RPMX-MANIFEST.json'

cargo run --release -- a.src.rpm >/dev/null
cargo run --release -- b.src.rpm >/dev/null
cmp a.src.rpm b.src.rpm

echo "ok: source package is readable, digest-valid and deterministic"
