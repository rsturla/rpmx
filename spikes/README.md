# Spikes

Small, runnable experiments that test the riskiest design claims. Each one has
its own `README.md`, a `verify.sh`, and the exact commands it ran.

| Spike | Question | Result |
|---|---|---|
| [`01-native-srpm/`](01-native-srpm/) | Can a source RPM be written without a spec? | Yes, with a 12-line `rpm-rs` patch. RPM 6 reads it, digests verify, output is deterministic. |
| [`02-native-binary/`](02-native-binary/) | Can a binary RPM be assembled and installed without `rpmbuild`? | Yes. Installs, verifies, runs and erases in Fedora 44. `SOURCERPM` still needs setting. |
| [`03-bwrap/`](03-bwrap/) | Does the default sandbox block external network but keep loopback? | Yes. Host `/sys` leaks interface names unless a fresh `/sys` is mounted. |

These are prototypes, not the implementation. They use host tooling where the
design would use a composed guest root, and they do not exercise dependencies,
scriptlets, actions or debuginfo.

Run everything:

```sh
for d in spikes/*/; do [ -x "$d/verify.sh" ] && "$d/verify.sh"; done
```
