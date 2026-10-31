#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

cargo run --release -- rpmx-demo-bin-1.0.0-1.x86_64.rpm >/dev/null

test "$(rpm -qp --qf '%{NAME}' rpmx-demo-bin-1.0.0-1.x86_64.rpm)" = "rpmx-demo-bin"
test "$(rpm -qp --qf '%{ARCH}' rpmx-demo-bin-1.0.0-1.x86_64.rpm)" = "x86_64"
rpm -Kv rpmx-demo-bin-1.0.0-1.x86_64.rpm | grep -q 'digest: OK'
rpm -qpl rpmx-demo-bin-1.0.0-1.x86_64.rpm | grep -qx '/usr/bin/rpmx-demo-bin'

cargo run --release -- a.rpm >/dev/null
cargo run --release -- b.rpm >/dev/null
cmp a.rpm b.rpm

podman run --rm -v "$PWD:/work:Z" registry.fedoraproject.org/fedora:44 sh -c '
set -e
rpm -i --nodeps /work/rpmx-demo-bin-1.0.0-1.x86_64.rpm
rpm -V rpmx-demo-bin
/usr/bin/rpmx-demo-bin | grep -q "rpmx-demo-bin 1.0.0"
rpm -e rpmx-demo-bin
! rpm -q rpmx-demo-bin
'

echo "ok: binary package installs, verifies, runs and erases"
