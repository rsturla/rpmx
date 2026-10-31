# Spike 03: bubblewrap with network off

## Question

Does the default sandbox shape actually block external network while still
letting a build run and reach loopback?

The design says every untrusted phase runs in a network namespace with no
external interfaces, and that loopback is allowed so test suites can start local
servers.

## What was run

```sh
bwrap --unshare-all --die-with-parent --new-session \
  --ro-bind / / --dev /dev --proc /proc --tmpfs /tmp --tmpfs /sys \
  --chdir /tmp sh -c '...'
```

The root is the host root, read-only. This is only a mechanism test; the real
design uses a composed guest root, not the host.

## Results

| Check | Result |
|---|---|
| External TCP to `1.1.1.1:443` | blocked (`OSError`) |
| Loopback bind and connect | works |
| Write to `/tmp` | works |
| Write to `/usr` | denied (`Read-only file system`) |
| Run a host toolchain (`gcc`) | works |
| UID inside | unprivileged (`4241576`, no mapping to 0) |
| Host network interface names via `/sys/class/net` | **leaked** when host `/sys` is mounted |

The last row is the finding. `--unshare-all` unshares the network namespace, but
binding the host `/sys` still exposes the host's interface names. Adding
`--tmpfs /sys` removes them. The network is already unreachable; the leak is
information, not access, but a sandbox should not show the host's hardware
topology.

## Consequences for the design

- The sandbox must mount a fresh or empty `/sys`, not the host's, or use a guest
  root that already has its own.
- Loopback is available and reachable, which is what the installed-test model
  needs.
- `/proc` is a fresh procfs; `/dev` is the minimal device set from `--dev`.
- The root must be read-only except the declared writable mounts; this held.

## Reproduce

```sh
cd spikes/03-bwrap
./verify.sh
```

The script exits non-zero if external network succeeds or if host interface
names appear.
