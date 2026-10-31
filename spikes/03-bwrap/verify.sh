#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"

out="$(
  bwrap --unshare-all --die-with-parent --new-session \
    --ro-bind / / --dev /dev --proc /proc --tmpfs /tmp --tmpfs /sys \
    --chdir /tmp python3 - <<'PY'
import socket

try:
    socket.create_connection(("1.1.1.1", 443), 1)
    print("external=REACHABLE")
except OSError:
    print("external=BLOCKED")

srv = socket.socket()
srv.bind(("127.0.0.1", 0))
srv.listen(1)
port = srv.getsockname()[1]
client = socket.create_connection(("127.0.0.1", port), 2)
conn, _ = srv.accept()
conn.sendall(b"ok")
print("loopback=" + client.recv(2).decode())
client.close()
conn.close()
srv.close()
PY
)"

grep -qx 'external=BLOCKED' <<<"$out"
grep -qx 'loopback=ok' <<<"$out"

ifaces="$(
  bwrap --unshare-all --die-with-parent --new-session \
    --ro-bind / / --dev /dev --proc /proc --tmpfs /tmp --tmpfs /sys \
    --chdir /tmp sh -c 'ls /sys/class/net 2>/dev/null || true'
)"
test -z "$ifaces"

echo "ok: external network blocked, loopback works, host interfaces hidden"
