# Spike 02: a native binary RPM without `rpmbuild`

## Question

Can `rpmx` assemble an installable binary RPM with no `rpmbuild` and no
generated spec?

This is the release gate. Spike 01 proved the source package; this one tests
the binary path.

## Environment

| Tool | Version |
|---|---|
| RPM | 6.0.2 (host), Fedora 44 container (install test) |
| `rpmbuild` | not installed |
| `rpm` crate | 0.30.2 |

`rpm --root` is blocked in this environment (`Unable to change root directory:
Operation not permitted`), so installation was tested inside a Fedora 44
container. That is also closer to how the design expects packages to be tested.

## What was built

`src/main.rs` writes a package with:

- `/usr/bin/rpmx-demo-bin` (mode 0755, a shell script);
- `/usr/share/doc/rpmx-demo-bin/README` (mode 0644).

No spec, no `rpmbuild`.

## Results

```
$ cargo run --release -- rpmx-demo-bin-1.0.0-1.x86_64.rpm
$ rpm -qpl rpmx-demo-bin-1.0.0-1.x86_64.rpm
/usr/bin/rpmx-demo-bin
/usr/share/doc/rpmx-demo-bin/README

$ rpm -Kv rpmx-demo-bin-1.0.0-1.x86_64.rpm
    Header SHA256 digest: OK
    Payload SHA256 digest: OK
```

Inside Fedora 44:

```
$ rpm -i --nodeps /work/rpmx-demo-bin-1.0.0-1.x86_64.rpm
$ rpm -V rpmx-demo-bin            # no output: all attributes match
$ stat -c "%a %U:%G %n" /usr/bin/rpmx-demo-bin
755 root:root /usr/bin/rpmx-demo-bin
$ rpm -q --qf "%{NAME} %{VERSION}-%{RELEASE} %{ARCH}\n" rpmx-demo-bin
rpmx-demo-bin 1.0.0-1 x86_64
$ rpm -e rpmx-demo-bin
$ rpm -q rpmx-demo-bin
package rpmx-demo-bin is not installed
```

The installed script runs:

```
$ /usr/bin/rpmx-demo-bin
rpmx-demo-bin 1.0.0
```

Determinism: two builds are byte-identical.

```
769cd0bc...  a.rpm
769cd0bc...  b.rpm
```

## Findings

| Question | Result |
|---|---|
| Can a binary RPM be written without a spec? | Yes. |
| Does stock RPM install it? | Yes, including file modes and ownership. |
| Does `rpm -V` pass? | Yes. |
| Is it deterministic? | Yes, with a fixed `source_date` and sorted inputs. |
| Are dependencies generated? | No. The recipe must declare them, or a later generator must add them. |

## Gaps this exposes

1. **`SOURCERPM` is hardcoded to `(none)`.** A real binary RPM records
   `name-version-release.src.rpm`. This matters for provenance, `rpm -qi` and
   rebuild traceability. The writer must set the actual SRPM NEVRA.
2. **The payload is zstd by default.** Valid, but the distro convention should
   be a policy choice, not a crate default.
3. **No dependency generation.** ELF `SONAME`/`NEEDED`, Python `Provides` and
   the rest are not automatic. This is design-consistent but must be built.
4. **Scriptlets, triggers and actions were not exercised.** The crate exposes
   them; the typed-action emitters still need implementation and tests.
5. **Debuginfo is absent.** Correct for opt-in debuginfo, but the `debuginfo`
   step still needs a `debugedit`-equivalent implementation.

## Verdict

The native binary writer is feasible now using `rpm-rs`, with the same caveat as
the source writer: a small patch or a thin layer that sets the tags the crate
does not expose. Nothing here requires `rpmbuild`.

The remaining risk moved from "can we write a binary RPM" to "can we generate
correct dependencies, scriptlets and actions". That is normal application work,
not an existential format risk.

## Reproduce

```sh
cd spikes/02-native-binary
cargo run --release -- rpmx-demo-bin-1.0.0-1.x86_64.rpm
./verify.sh
```

`verify.sh` needs `podman` and pulls `registry.fedoraproject.org/fedora:44`.
