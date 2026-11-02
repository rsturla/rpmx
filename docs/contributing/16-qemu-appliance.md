# QEMU transport appliance: separating the runner from the build root

Status: implemented, 2026-11-01. The lock, resolution, assembly, cache,
`rpmx runner` prebuild/inspect command, `--runner-asset-root` overrides,
two-disk boot and two-channel session are in place. A QEMU build and an offline
SRPM rebuild have been exercised end to end on x86_64.

**Update 2026-11-02:** the appliance is a sidecar, not a section in
`root.lock.yaml`. The root lock describes only the build root; the appliance
lock lives at `appliance.lock.yaml` beside the cached root and is resolved on
demand for `--runner qemu`. The `runner-assets` section, `--appliance` on
`rpmx root resolve`, and `--appliance-root`/`--no-appliance` on
`rpmx root assemble` have been removed. `rpmx runner resolve|assemble|inspect`
is the explicit appliance interface. The historical sections below that
describe embedding are superseded by this note.

Related documents:

- [`09-sandboxing.md`](09-sandboxing.md) — the sandbox contract and the current
  QEMU exception.
- [`14-build-root.md`](14-build-root.md) — root resolution, assembly, locking
  and the root digest.
- [`08-threat-model.md`](08-threat-model.md) — trust boundaries.
- [`07-srpm-first.md`](07-srpm-first.md) — what an SRPM must be able to rebuild.

## 1. Summary

Today a QEMU build boots the build root itself. To do that, the build root must
also be a bootable appliance: a kernel, an initramfs, systemd-networkd,
`openssh-server`, `tar` and friends. Those packages are therefore part of the
build environment, and the profile has to name them.

That is the wrong seam. It makes the build environment depend on the runner,
lets transport packages satisfy build dependencies by accident, and writes
transport packages into the SRPM's embedded root lock.

The target state is:

> The build root is a function of the recipe, the profile and the target
> architecture. The runner changes how steps execute, never what they see.
> QEMU boots a separate, runner-owned **transport appliance**, attaches the
> build root as a second disk, and runs steps chrooted into it.

The appliance is an RPM root too, with its own closure and lock, but it is not
part of the build root, not part of `closure_digest`, and not mounted into the
build root's filesystem view.

Chainguard's melange already works this way: its QEMU runner boots a separate
`microvm-init` initramfs and attaches an apko-built guest rootfs as a block
device (Section 9). That is the main external evidence that the split is
practical.

### 1.1 Decisions at a glance

| # | Decision |
|---|---|
| **D1** | The appliance is a separate root, not a package set inside the build root. |
| **D2** | Its package list is owned by rpmx and resolved from the profile's repositories, releasever and builder image. Profiles do not mention it. |
| **D3** | Its lock is a sidecar `appliance.lock.yaml` beside the cached build root, outside the build `closure_digest` and outside `root.lock.yaml`. |
| **D4** | Only an explicit `--runner qemu` needs it. `root resolve` and automatic roots describe the build root only; they never embed an appliance. |
| **D5** | It is identified by a closure digest, schema and builder digest, not by a tree digest. Its initramfs is not a build input. |
| **D6** | QEMU boots the appliance from `/dev/vda` and the build root from `/dev/vdb`; sshd and tar live in the appliance; steps run on a chrooted build channel. |
| **D7** | The VM remains the security boundary. chroot provides dependency isolation, not a security boundary. |
| **D8** | No 9p or virtiofs. The build root stays on a block device. |
| **D9** | A prebuilt, signed appliance image is a future optimization behind the same interface, not part of this plan. |
| **D10** | Adopt melange's two-channel model: a control channel outside the chroot for mount, copy and sync, and a build channel chrooted into the build root for steps. |
| **D11** | Adopt melange's per-session host-key verification and secure deletion of key material and the appliance copy. |
| **D12** | Expose the appliance as an explicit artifact (`rpmx runner`) so it can be prebuilt, cached and inspected like melange's `melange initramfs`. |

## 2. Current behaviour (as implemented)

The relevant path is:

1. `autoroot::requirement` merges `profile.packages.install` with every
   dependency the plan declares (`crates/rpmx-cli/src/commands/autoroot.rs:106`).
2. `root::resolve_profile` asks dnf for that closure inside the profile's
   builder image and records it in `root.lock.yaml`
   (`crates/rpmx-root/src/root/resolve.rs`).
3. `root::assemble` installs the locked payloads offline into an empty
   `--installroot` (`crates/rpmx-root/src/root/assemble.rs`).
4. For a foreign target, `QemuBackend` takes that same root, finds
   `/boot/vmlinuz-*` and `/boot/initramfs-*.img` in it, copies it to an ext4
   image, injects an SSH key, an sshd config and a DHCP network unit, boots it,
   and copies the workspace and stage in over SSH
   (`crates/rpmx-runner/src/runner/backend/qemu/`).
5. Installed tests take a different path and add the boot set themselves when
   the target is foreign (`crates/rpmx-root/src/test_root.rs:56`).
6. `root_digest` walks the whole assembled tree, including `/boot`
   (`crates/rpmx-store/src/tree.rs`).
7. The root lock is embedded in the SRPM as `root.lock.json`
   (`crates/rpmx-build/src/source_package.rs:64`).

The profile therefore has to contain:

```yaml
packages:
  install: [bash, coreutils, gcc, make,
            kernel-core, kernel-modules-core, kernel-modules,
            dracut, kmod, util-linux, systemd-networkd,
            openssh-server, tar]
```

and README tells the user exactly that (`README.md:337`).

### 2.1 Three layers in one directory

| Layer | What it is | What it should be |
|---|---|---|
| Build environment | The RPM closure the recipe compiles against | A function of recipe + profile + target |
| Target distro | releasever, repositories, arch | Part of the build environment |
| Transport | kernel, initramfs, networkd, sshd, tar | Runner-owned, outside the build environment |

The current design makes the third layer part of the first.

## 3. Why this needs changing

### 3.1 The profile is runner-aware

A profile is documented as "root policy, not a recipe" (`docs/usage/profiles-and-roots.md:5`).
It now has to carry a kernel and an SSH server because one runner needs a
bootable disk. A native build using the same profile installs both and never
uses them.

### 3.2 Build dependencies are satisfied by transport packages

A recipe with a missing `BuildRequires: tar`, `util-linux` or `openssh-server`
builds successfully under QEMU and looks correct. The root is not wrong; it is
simply larger than the environment the recipe declared. For a system whose
selling point is a locked, hermetic closure, that is a false negative in the
most important check.

### 3.3 Transport ends up in the SRPM

`rpmx srpm --root-lock` embeds the full root lock as `root.lock.json`
(`crates/rpmx-build/src/source_package.rs:64`). A foreign build therefore
records `kernel-core`, `dracut`, `openssh-server` and `systemd-networkd` as part
of the package's build environment. A later native rebuild is expected to
assemble a bootable root even though it will never boot it.

### 3.4 Build and test roots disagree

`test_root.rs` adds the boot set automatically for foreign targets. The build
root makes the user write it in the profile. Same requirement, two mechanisms,
opposite defaults. A test-root change can silently diverge from a build-root
change.

### 3.5 The root digest is exposed to boot-time generation

`tree_digest` covers `/boot` with no exclusions. Kernel scriptlets run `depmod`
and `dracut` during assembly. If dracut writes any build-time data into the
initramfs, two assemblies of the same lock produce different `root_digest`
values, and every `root assemble` plus verification becomes slower and less
trustworthy. Document 14 says two assemblies matched in the Fedora 44 fixture,
but that fixture does not contain a kernel, so it does not prove the bootable
case.

### 3.6 Transport lives inside the untrusted tree

The host copies outputs by running the guest's `tar` and trusting the stream
(`crates/rpmx-runner/src/runner/backend/qemu/ssh.rs`). `tar_guard` validates
paths, which is good, but a build step can still replace `tar`, `sshd` or
`mount` before sync-out. The QEMU boundary contains the damage; it does not
make the transport trustworthy.

### 3.7 What is not broken

- Offline assembly from a pinned CAS closure.
- Failing loudly when the root is not bootable.
- Reusing one root across runners.
- No 9p, no virtiofs, no host path shared with the guest.

Those properties stay.

## 4. Target contract

### 4.1 Invariants

| ID | Invariant |
|---|---|
| **I1** | The build root depends only on the recipe, the profile and the target architecture. It never depends on the host architecture, the runner, or the transport. |
| **I2** | The same build closure produces the same `root_digest` under bubblewrap, podman and QEMU. |
| **I3** | The SRPM's `root.lock.json` describes the build environment only. Transport is a runner artifact. |
| **I4** | A transport package cannot satisfy a build dependency, by declaration or by accident. |
| **I5** | A runner changes how a step executes, not what the step can see. |
| **I6** | An offline rebuild works from the lock, the CAS and, when a VM is needed, the locked appliance. |

### 4.2 Definitions

- **Build root** — the RPM closure from the profile and the plan. Mounted (or
  chrooted to) as `/` for every step.
- **Appliance root** — a small, target-architecture RPM root containing the
  kernel, initramfs, init system, network and SSH transport. It is never the
  build environment.
- **Transport** — moving the workspace and stage in and out, running steps and
  collecting output.

### 4.3 Non-goals

- Cross-compilation. Foreign builds still execute target binaries in a VM.
- 9p or virtiofs. The build root stays on a block device.
- Making chroot a security boundary. The VM is the boundary. Section 5.5
  explains the delta.
- Prebuilt appliance images. Section 10 keeps that as a future optimization
  behind the same interface.

## 5. Architecture

### 5.1 Shape

```
profile + plan
      |
      v
 root requirement
      |
      +--> build closure  --> build root  (identical for all runners)
      |
      +--> appliance closure --> appliance root (only when a VM is needed)
                                        |
                                        v
                             +----------------------+
                             |  qemu-system         |
                             |  /dev/vda appliance  |  kernel + initrd from
                             |  /dev/vdb build      |  the appliance root
                             |  sshd control +      |
                             |  chrooted build port |
                             |  steps see the build |
                             |  root as /           |
                             +----------------------+
```

The appliance is resolved from the same profile repositories, releasever and
builder image, with a package list owned by rpmx. It is cached by closure, not
by recipe.

### 5.2 Appliance root

Initial package set (`APPLIANCE_PACKAGES`, versioned by `APPLIANCE_SCHEMA`):

| Package | Why |
|---|---|
| `kernel-core` | The target-architecture kernel image. |
| `kernel-modules-core`, `kernel-modules` | virtio-blk, virtio-net, ext4 and the rest of the boot path. |
| `dracut`, `kmod` | initramfs and `depmod` during assembly. |
| `systemd` | PID 1 and unit ordering. |
| `systemd-networkd` | DHCP on the host-only NIC. |
| `openssh-server` | The control channel. |
| `bash`, `coreutils` | `/bin/sh`, `env`, `mkdir`, `true`, `sleep`. |
| `util-linux` | `mount`, `chroot`, `findmnt`. |
| `tar` | Workspace and stage transfer. |

The list is a constant in `rpmx-root`, not a profile field. It may be trimmed
later (`kernel-modules-core` plus explicit dracut modules instead of the full
module set), but that is a schema change, not a user-visible one.

The appliance root is assembled with the same offline path as a build root:

1. Synthesize a profile from the build profile: same `releasever`, `builder`,
   `repositories`; `options.weak_deps = false`, `options.docs = false`;
   `debuginfo = false`, `sbom = false`; `packages.install = APPLIANCE_PACKAGES`.
2. Resolve it with dnf into an `ApplianceLock`.
3. Assemble it offline from the CAS and verify the installed NEVRA set.
4. Cache it under `$XDG_CACHE_HOME/rpmx/appliances/<identity>/root`.

`identity` is a digest over the appliance schema, the appliance closure digest
and the builder image digest. It is independent of the recipe and of the host.

The appliance root gets no `root_digest` in the lock. Its identity is the
closure, not the tree: the initramfs is generated by a scriptlet and is not a
build input. The assembled package set is verified against the lock, as it is
for a build root. If a tree check is wanted later, it must exclude `/boot`.

### 5.3 Boot protocol

Two disks, labels instead of device ordering:

| Disk | Label | Contents |
|---|---|---|
| `/dev/vda` | `rpmx-appliance` | The appliance root, with injected SSH key, sshd config, DHCP unit and mount unit. |
| `/dev/vdb` | `rpmx-build` | A disposable writable copy of the build root. |

Kernel command line:

```
console=ttyS0 root=LABEL=rpmx-appliance rw
```

`-kernel` and `-initrd` come from the appliance root's `/boot`, not from the
build root. The kernel/initrd pair is selected by matching version, not by
taking the first entry in `/boot`.

Injected into the appliance disk:

- `/root/.ssh/authorized_keys` and the per-session SSH host key.
- `/etc/ssh/sshd_config` with two ports and a chrooted build port. The control
  port (22) is root outside any chroot; the build port (2222) is chrooted into
  the build root:

```sshconfig
Port 22
Port 2222
PermitRootLogin prohibit-password
PasswordAuthentication no
PubkeyAuthentication yes
UsePAM no
HostKey /etc/ssh/ssh_host_ed25519_key

Match LocalPort 2222
    ChrootDirectory /run/rpmx/root
```

  `ChrootDirectory` requires every path component to be root-owned and not
  group- or world-writable. `/run/rpmx` and `/run/rpmx/root` are created by the
  mount unit with mode 0755, so that holds by construction. The runner checks
  it before starting and fails with `RPMX-RUNNER-004` if a profile produced an
  unusable root directory mode.
- `/etc/systemd/network/20-rpmx.network` (DHCP).
- `/etc/systemd/system/rpmx-buildroot.service`:

```ini
[Unit]
Description=Mount the rpmx build root
After=local-fs.target
Before=sshd.service

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/bin/sh -c '\
  set -e; \
  mkdir -p /run/rpmx/root; \
  mount -L rpmx-build /run/rpmx/root; \
  mkdir -p /run/rpmx/root/proc /run/rpmx/root/dev /run/rpmx/root/sys \
           /run/rpmx/root/tmp/build /run/rpmx/root/tmp/stage; \
  mount -t proc proc /run/rpmx/root/proc; \
  mount --rbind /dev /run/rpmx/root/dev; \
  mount --rbind /sys /run/rpmx/root/sys; \
  mount --make-rslave /run/rpmx/root/dev; \
  mount --make-rslave /run/rpmx/root/sys'

[Install]
WantedBy=multi-user.target
```

It is enabled, and `sshd.service` is made to require it (`sshd.service.requires`
symlink) so SSH cannot come up before the build root is mounted. The unit also
waits for the build disk's systemd device unit, so `mount -L` does not race
udev. Because the chrooted build port needs the mount, a failed unit also fails
the build port instead of leaving it in a half-configured state.

QEMU forwards two host ports: the control port and the build port. The guest
still has only the one host-only NIC (`restrict=on`).

After the control port is up, the session probes the build port with `true`.
A failed probe is `RPMX-RUNNER-004`, with the QEMU log tail, not a mysterious
step failure.

### 5.4 Step execution and transfers

The session has two channels, mirroring melange:

| Channel | Port | Root | Used for |
|---|---|---|---|
| Control | 22 | Appliance root (unchrooted) | Mount setup, `copy_in`, `sync_out`, lifecycle probes. |
| Build | 2222 | Build root (`ChrootDirectory`) | Pipeline steps. |

A step is a normal `sh -s` on the build channel:

```
sh -s
```

- `sshd` applies `ChrootDirectory /run/rpmx/root` before the shell starts, so
  `/` is the build root. No per-step `chroot` wrapper is needed, and the build
  root does not need `openssh-server`.
- The build root needs only a shell (`/bin/sh`), which any shell step already
  implies. The profile must provide `bash` or another `/bin/sh`.
- sshd chdirs to the login user's home (`/root` for root). The runner creates
  that directory on the disposable disk copy if it is missing; the cached root
  is not modified.
- `request.cwd` is a path inside the build root (`/tmp/build` by default).
- The step script exports the sandbox environment. The channel inherits sshd's
  small environment (`HOME`, `USER`, `SHELL`, `PATH`, `SSH_*`); the script
  overwrites every contract variable. It is not a fully cleared environment
  like bubblewrap's `--clearenv`. Clearing it would require `env` inside the
  build root, which is an implicit build dependency; this is a documented gap,
  not a hidden one. A future option is a tiny `ForceCommand` wrapper shipped in
  the appliance and bind-mounted read-only into the build root, which is also a
  transport artifact and therefore not free.

Transfers stay on the control channel, outside the chroot:

```
mkdir -p /run/rpmx/root/tmp/build && tar -C /run/rpmx/root/tmp/build -xf -
tar -C /run/rpmx/root/tmp/build -cf - .
```

The paths are built from one constant, `BUILD_ROOT_MOUNT = "/run/rpmx/root"`,
plus the existing `rpmx_base::guest::BUILD` and `STAGE`. The host-side tar
guard, the 8 GiB cap, the streaming behavior and the bounded stderr tail are
unchanged.

The transfer must preserve modes and ownership of the build tree. Extended
attributes and ACLs are not part of rpmx's current package contract; if they
become one, the transfer needs `tar --xattrs --acls` on both sides, as melange
does. Do not silently claim xattr support without that.

### 5.5 Threat-model delta

- The appliance is trusted infrastructure. It is built from the same locked
  repositories as the build root, and its closure is recorded. It is not signed
  separately.
- The build root remains untrusted. Steps still run as root inside it.
- chroot is **not** a security boundary. A root process that escapes it is
  still inside the VM, and the VM is the boundary. The benefit is dependency
  isolation: an ordinary build cannot see or replace the transport packages.
  A hostile build can still attempt a chroot escape; that does not change the
  host's trust boundary.
- Transport now runs from the appliance's `tar` and `sshd`, not from binaries
  the build root can overwrite. Host-side validation of the tar stream stays.
- The build channel is chrooted by sshd; the control channel is the only one
  that can see the appliance. A step therefore does not normally get a path to
  the transport binaries.
- The guest still has one host-only NIC (`restrict=on`) used only for the two
  SSH port forwards.

A future hardening step is to run steps as an unprivileged UID and to give each
step a private PID/mount namespace. That is out of scope here because it
changes what builds are allowed to do.

### 5.6 Performance

- One extra resolve and assembly per (repositories, releasever, arch, schema),
  cached. It is not per recipe and not per build.
- The appliance disk image is created once per cached appliance and copied per
  session (`cp --reflink=auto` where available). The build disk is created per
  session as today.
- `/boot` disappears from every build root, so root assembly, digesting and
  storage get cheaper.
- VM boot time is essentially unchanged; the second disk is a local virtio
  device.

## 6. Data model and interfaces

### 6.1 Appliance lock

`RootLock` gains an optional `runner-assets` section. It is serialized with the
root lock, so the existing SRPM embedding carries it without a second file.
The section is generic: `qemu_appliance` is the first asset kind, and a future
runner adds another named asset without changing the build closure.
`rpmx-runner`'s `RunnerDescriptor` registry declares which asset kinds each
runner needs; `rpmx doctor` iterates that registry.

```yaml
runner-assets:
  qemu_appliance:
    schema: 1
    profile: fedora-44-qemu-appliance
    archs: [aarch64]
    releasever: "44"
    builder:
      image: registry.fedoraproject.org/fedora:44
      image_digest: sha256:...
    repositories:
      - id: fedora
        metalink: https://mirrors.fedoraproject.org/metalink?repo=fedora-$releasever&arch=$basearch
    packages:
      aarch64:
        - name: kernel-core
          repo_id: fedora
          evr: 6.16.3-200.fc44
          checksum: sha512:...
          size: 123456
          srpm: kernel-6.16.3-200.fc44.src.rpm
      noarch:
        - name: systemd-networkd
          ...
    closure_digest: sha512:...
```

Type:

```rust
pub struct ApplianceLock {
    pub schema: u32,
    pub profile: String,
    pub archs: Vec<String>,
    pub releasever: String,
    pub builder: LockedBuilder,
    pub repositories: Vec<Repository>,
    pub packages: BTreeMap<String, Vec<LockedPackage>>,
    pub closure_digest: String,
}
```

Rules:

- The section is parsed strictly (`deny_unknown_fields`), like the rest of the
  lock. A producer that adds a field older binaries do not know is a hard
  failure, not a silent downgrade.
- `schema` is bumped whenever `APPLIANCE_PACKAGES` or the boot protocol
  changes. A lock with an old schema is rejected with `RPMX-ROOT-021` and a
  re-resolve hint; silently booting an old appliance is worse.
- `closure_digest` is computed with the existing `closure_digest()` function.
  `load_lock` recomputes it. `RPMX-ROOT-021` on mismatch.
- The appliance is deliberately outside the build `closure_digest` and outside
  `root_digest`. The SRPM's `root.lock.json` digest covers the whole document,
  including the appliance section.
- `RootLock.appliance` is `Option<ApplianceLock>`. A lock without it still
  loads. A QEMU build with such a lock fails with `RPMX-ROOT-020` and tells the
  user how to resolve one.
- `root export`/`root import` keep exporting only the build closure. The
  appliance is runner metadata, not part of the libpkgmanifest contract.

### 6.2 Root resolution policy

Add an appliance policy to the requirement:

```rust
pub enum AppliancePolicy { Auto, Always, Never }
```

- Only an explicit `--runner qemu` needs an appliance. `auto` and `--runner
  bwrap` use host binfmt for foreign targets and never resolve one.
- `--runner-asset-root DIR` uses a preassembled appliance instead of the cache.
- If an appliance is requested and its packages cannot be resolved, the error
  comes from the resolver and names the missing package. The user must add a
  target-distribution repository that carries `kernel-core` and
  `openssh-server`; rpmx does not silently fall back to the host or a
  different distribution.

The build-root requirement digest is runner-independent: it covers the
profile and target only. The appliance has its own identity and cache, so the
same build root serves `bwrap`, `binfmt` and `qemu`.

### 6.3 CLI surface

New and changed flags:

| Command | Flag | Meaning |
|---|---|---|
| `rpmx build`, `test`, `source`, `rebuild` | `--runner-asset-root PATH` | Use a pre-assembled appliance instead of the cache. |
| `rpmx runner` | `resolve\|assemble\|inspect` | Build, cache or inspect the appliance independently, analogous to `melange initramfs`. |
| `rpmx doctor` | — | Report cached appliance entries alongside qemu-system, binfmt-adjacent tools and the cache. |

An appliance lock is a standalone YAML file managed by `rpmx runner`.

`--offline` never resolves. If a VM is needed and no appliance sidecar or
`--runner-asset-root` is available, fail with `RPMX-ROOT-020`.

### 6.4 Runner interface

The smallest change that keeps the `Runner` trait honest:

- `SandboxConfig` and `RunnerConfig` carry `RunnerAssets`, a runner-owned set of locked assets. QEMU reads its `QemuAppliance` entry.
- `QemuBackend` keeps `{ root }`; `start` reads `config.assets.get(QemuAppliance)` and fails with `RPMX-RUNNER-004` if it is absent.
- `Sandbox::new` rejects assets without `root`, and still rejects `root` plus `image`.
- `RunnerKind::choose`/`resolve` do not change. This avoids touching the
  selection tests and keeps the appliance a runner asset rather than a second
  root-selection axis.

`QemuSession` keeps two ports and two SSH argument sets: `control` and `build`.
`run` uses the build channel, `copy_in`/`sync_out` and the readiness probes use
the control channel. The generated host key is written to a temporary
`known_hosts` file and the client uses `StrictHostKeyChecking=yes` with that
file, instead of today's `StrictHostKeyChecking=no` and
`UserKnownHostsFile=/dev/null`. The client key and host key are zeroed and the
appliance copy is removed on drop.

All `SandboxConfig` literals need the new field. Current call sites:

- `crates/rpmx-build/src/source.rs`
- `crates/rpmx-cli/src/commands/build.rs` (through the pipeline)
- `crates/rpmx-cli/src/commands/rebuild/mod.rs`
- `crates/rpmx-cli/src/commands/testcmd.rs`
- `crates/rpmx-runner/src/test_runner.rs`
- the tests in `crates/rpmx-core/tests/` and `crates/rpmx-runner/src/sandbox/mod.rs`

`autoroot::ensure` also gains the runner kind (or an explicit appliance
boolean) so it can apply the policy. `autoroot::AutomaticRoot` gains
`appliance: Option<PathBuf>`, and `autoroot::ensure_from_lock` returns both
roots. `crates/rpmx-cli/src/commands/shell.rs` calls `ensure` but uses only the
build root; it passes the policy and ignores the appliance. The pipeline
carries the appliance alongside `root` exactly as it carries the build root
today.

`TestRootOptions` loses its `runner` field after the boot-set block is removed
from `test_root.rs`; the only remaining use of `runner` there is deciding which
packages to install, and that decision moves to the appliance.

### 6.5 Error codes

| Code | Meaning |
|---|---|
| `RPMX-ROOT-020` | A VM build needs an appliance and the active lock has none. |
| `RPMX-ROOT-021` | The appliance section is malformed, has an unsupported schema, or its closure digest does not match. |
| `RPMX-RUNNER-004` | The appliance failed to boot, mount the build root, or enter the chroot. |

Existing codes keep their meaning. `RPMX-RUNNER-002` stays "this runner cannot
execute this target".

## 7. Implementation plan

Each phase is independently shippable. The project is pre-1.0, so Phase 3 may
remove code rather than carry a compatibility shim indefinitely.

### Phase 0 — measure and guard (no behaviour change)

Goal: prove or disprove the `/boot` determinism concern and make the current
failure mode clear.

- Add a test that assembles a *bootable* root twice from the same lock and
  compares `root_digest`. If it differs, add `/boot` to the digest exclusions
  in `rpmx-store/src/tree.rs` (and document it in 14), or record the result as
  an open question. Do not change the build-root contract yet.
- Add a test that `required_packages(plan)` never adds boot packages. It
  currently does not; the test prevents a future "just add it here" fix.
- Make the current boot requirements fail earlier. `QemuBackend::prepare`
  already checks for `sshd`; add a check for the kernel and initramfs at the
  same point, with one message that names all missing items. Keep the codes.
- Document in `test_root.rs` that its boot list is temporary and removed in
  Phase 2.

Exit criteria:

- The determinism question has a test and an answer.
- A missing boot package fails with a single actionable message before a VM is
  started.

Files: `crates/rpmx-store/src/tree.rs`, `crates/rpmx-runner/src/runner/backend/qemu/`,
`crates/rpmx-root/src/test_root.rs`, `docs/contributing/14-build-root.md`.

### Phase 1 — appliance lock and assembly (no boot change)

Goal: produce a locked, assembled appliance without changing how QEMU boots.

- Add `crates/rpmx-root/src/appliance.rs`:
  - `APPLIANCE_SCHEMA`, `APPLIANCE_PACKAGES`.
  - `appliance_profile(&Profile, arch) -> Profile`.
  - `resolve_appliance(profile, cache, work) -> ApplianceLock`.
  - `assemble_appliance(lock, cache, work) -> PathBuf`.
  - `ApplianceLock::identity()` for the cache key.
- Add `ApplianceLock` and `RootLock.appliance` in
  `crates/rpmx-root/src/root/lock.rs`, with validation in `load_lock` and
  `write_lock`.
- Add `--appliance auto|always|never` to `rpmx root resolve` (default
  `always`), and `--appliance-root`/`--no-appliance` to `rpmx root assemble`.
- When `root assemble` sees an appliance, assemble both and print both paths.
- Add `rpmx runner resolve|assemble|inspect` (or `rpmx root appliance ...`)
  as the explicit prebuild/cache/inspect entry point, analogous to
  `melange initramfs`.
- Add unit tests: profile synthesis, package-list stability, round-trip,
  tampered closure, unsupported schema, lock without appliance.

Exit criteria:

- `rpmx root resolve --appliance always` writes a lock with an appliance.
- `rpmx root assemble` produces a build root and an appliance root.
- The appliance root contains a kernel, initramfs, `sshd` and `tar`; the build
  root contains neither unless the profile asked for them.
- Existing tests pass; no QEMU behaviour has changed.

Files: `crates/rpmx-root/src/{appliance.rs,root/lock.rs,root/resolve.rs,root/assemble.rs,lib.rs}`,
`crates/rpmx-cli/src/commands/{args.rs,root.rs,appliance.rs,dispatch.rs,cli.rs}`,
`docs/usage/profiles-and-roots.md`, `docs/contributing/14-build-root.md`.

### Phase 2 — two-disk QEMU backend

Goal: boot the appliance, attach the build root, run steps on the chrooted
build channel.

- Add `assets: RunnerAssets` to `SandboxConfig` and `RunnerConfig`.
- Implement the boot path in `crates/rpmx-runner/src/runner/backend/qemu/`:
  - `make_disk(root, disk, label)` with `mke2fs -L`.
  - `start_vm` with two drives, `root=LABEL=rpmx-appliance`, and two forwarded
    SSH ports.
  - `inject_sshd` writes a two-port config with `ChrootDirectory` on the build
    port; `inject_build_mount` writes and enables `rpmx-buildroot.service`.
  - `wait_for_ssh` on the control port, then a `true` probe on the build port.
  - `copy_in`/`sync_out` use the control channel and `BUILD_ROOT_MOUNT` plus
    the guest paths.
  - `run` sends `sh -s` on the build channel; no per-step `chroot` wrapper.
  - Host-key verification with a temporary `known_hosts`, SSH keepalives, and
    zeroing/removal of key material.
- Wire the `rpmx runner` API from Phase 1 into `autoroot::ensure` and
  `ensure_from_lock` so QEMU builds use the cached appliance or assemble it
  from the lock, returning it alongside the build root.
- Pass the appliance through all `SandboxConfig` call sites.
- Remove the boot package block from `test_root.rs` and drop the now-unused
  `runner` field.
- Keep the legacy boot-from-root path behind `RPMX_QEMU_LEGACY_ROOT=1` and
  print a deprecation warning when it is used. The fallback exists only so a
  failure in the new path does not block the rest of the work.

Exit criteria:

- A foreign build whose profile contains only `bash, coreutils, gcc, make`
  succeeds.
- The assembled build root has no `/boot`, no `sshd`, and no `tar` unless the
  recipe declared it.
- A recipe that uses `tar` without declaring it fails under QEMU.
- The build root's `root_digest` equals the digest of the same closure built
  natively.
- Two assemblies of the same build root produce the same digest.
- A build step that installs a fake `/usr/bin/tar` does not affect sync-out.
- A step cannot see `/run/rpmx`, `/etc/ssh/sshd_config` or the appliance
  binaries from the build channel.
- A tampered host key is rejected, and the per-session key material is gone
  after the session ends.
- Installed tests use a test root without boot packages and pass.
- An offline foreign rebuild from an SRPM works because the lock carries the
  appliance.

Files: `crates/rpmx-runner/src/runner/backend/qemu/*`,
`crates/rpmx-runner/src/{runner/mod.rs,sandbox/mod.rs,test_runner.rs}`,
`crates/rpmx-cli/src/commands/{autoroot.rs,build.rs,testcmd.rs,source.rs,shell.rs,rebuild/*.rs}`,
`crates/rpmx-root/src/test_root.rs`, and the QEMU paragraphs in
`docs/contributing/09-sandboxing.md` and `14-build-root.md`.

### Phase 3 — make it the default and remove the old seam

Goal: the appliance is the only QEMU path; the docs and examples no longer
describe a bootable build root.

- Make `auto` resolve the appliance whenever the runner is QEMU; remove
  `RPMX_QEMU_LEGACY_ROOT`.
- Delete the legacy code that finds a kernel/initrd in the build root and
  injects SSH into the build disk. Keep the functions, but point them at the
  appliance.
- Update `README.md`, `docs/usage/getting-started.md`, `reference.md`,
  `profiles-and-roots.md`, `troubleshooting.md`, `docs/contributing/09-sandboxing.md`
  and `docs/contributing/14-build-root.md`.
- Add `rpmx doctor` reporting for the appliance.
- Add a CHANGELOG entry that says the profile no longer lists boot packages and
  that foreign rebuilds of older SRPMs need `--appliance-lock` or an online
  appliance resolve.
- Add acceptance scripts under `scripts/` and document them in
  `CONTRIBUTING.md`.

Exit criteria:

- No user-facing document tells a user to add a kernel or SSH server to a
  profile.
- `grep -R 'kernel-core\|openssh-server\|systemd-networkd'` in profiles and
  usage docs only matches the appliance constant and the design document.
- Every example builds natively and at least one builds through QEMU.
- The old path is gone, not merely disabled.

## 8. Test plan

### 8.1 Unit tests

`rpmx-root`:

- `appliance_profile_inherits_repositories_builder_and_releasever`.
- `appliance_package_list_is_stable` — a schema + list snapshot, so an
  accidental package change forces a schema bump.
- `appliance_lock_round_trips_through_yaml`.
- `tampered_appliance_closure_is_rejected` (`RPMX-ROOT-021`).
- `root_lock_without_appliance_still_loads`.
- `unsupported_appliance_schema_is_rejected`.

`rpmx-cli`:

- `requirement_digest_changes_with_appliance_policy`.
- `foreign_requirement_requests_an_appliance`.
- `native_requirement_does_not_request_an_appliance`.
- `cached_lock_without_an_appliance_is_not_reused_for_qemu`.
- `offline_qemu_without_an_appliance_fails_with_the_resolve_hint`.

`rpmx-runner`:

- `qemu_args_use_the_appliance_kernel_and_two_labelled_disks`.
- `sshd_config_chroots_only_the_build_port`.
- `control_and_build_ports_are_both_forwarded`.
- `sync_out_uses_the_control_channel`.
- `host_key_is_verified_with_known_hosts`.
- `key_material_is_zeroed_and_temp_files_removed`.
- `remote_paths_are_under_the_build_root_mount`.
- `a_missing_appliance_is_a_runner_error`.
- `the_build_root_service_is_enabled_before_sshd`.

### 8.2 Integration tests (need podman, qemu-system and binfmt)

Add `scripts/qemu-appliance-build.sh`:

1. Resolve a minimal profile (bash, coreutils, gcc, make) for the foreign arch
   with `--appliance always`.
2. Assemble both roots.
3. Assert the build root has no `/boot`, `/usr/sbin/sshd` or `/usr/bin/tar`.
4. Run a step that lists `/etc/ssh` and `/run/rpmx`; assert it sees neither
   the control configuration nor the appliance mount. This proves the build
   channel is chrooted, not just running in the same namespace.
5. Build a fixture package through QEMU.
6. Record `root_digest`; assemble the same build root again; assert equality.
7. Build a native copy of the same closure; assert the same `root_digest`.
8. Assert a recipe that uses `tar` without declaring it fails.
9. Install a fake `tar` in the build root from a step; assert sync-out still
   works and the fake did not run.
10. Rebuild from the SRPM with `--offline` and the cached appliance; assert
    success.

Run the build leg on x86_64 → aarch64 with TCG and, in native aarch64 CI, with
KVM. The `virt` machine type and the disk labels must both be exercised; they
are where an architecture-specific assumption would hide.

Add `scripts/qemu-appliance-test.sh`:

1. Build a package with an installed-stage test.
2. Prepare the test root.
3. Assert the test root has no kernel or SSH server.
4. Run the test through QEMU; assert success.

Add `scripts/qemu-appliance-mask.sh`:

1. Build with the old boot packages in the profile and a missing `tar`
   dependency; record that it passes.
2. Build with the appliance; assert it fails.
This is the regression test for the central bug.

### 8.3 What would prove the change

| Claim | Proof |
|---|---|
| Runner independence | Same closure assembled natively and under QEMU has the same `root_digest`. |
| No masked dependencies | The tar-without-declaration test fails under the appliance and passed under the old root. |
| Transport isolation | A fake `tar` in the build root does not run; sync-out is normal, and a step cannot see `/run/rpmx` or the control config. |
| Channel split | A step runs with `/` equal to the build root; the control channel still sees `/run/rpmx/root` and the appliance. |
| Host authenticity | A tampered host key is rejected; the generated `known_hosts` is the only trust anchor. |
| Hermetic foreign rebuild | `rpmx rebuild --offline` from the SRPM succeeds with only the lock, CAS and cached appliance. |
| Boot determinism | Two assemblies of the same build root produce the same digest. |
| Test-root parity | Installed tests pass with no boot packages in the test root. |

## 9. Prior art: Chainguard melange

Reference: `chainguard-dev/melange` at `main`, fetched 2026-11-01, plus
`docs/md/melange_initramfs.md`. The UX audit in this repository already uses
melange v0.63.2 as its reference implementation. Melange is APK/apko rather
than RPM, but the runner architecture is directly relevant.

### 9.1 How melange's QEMU runner is put together

1. **The build root is an apko guest rootfs.** `buildGuest`
   (`pkg/build/build.go`) locks `environment.contents` with apko and builds an
   image layer. The QEMU loader uncompresses that layer to a
   `melange-guest-*.tar` file (`qemuOCILoader.LoadImage` in
   `pkg/container/qemu_runner.go`).
2. **The transport is a separate initramfs.** `GenerateBaseInitramfs` builds a
   CPIO initramfs with apko from `microvm-init` plus optional extra packages.
   `MicrovmConfig` carries the init package, repositories and extra packages.
   The result is cached under `kernel/<arch>/melange-cpio/` for 24 hours, can
   be supplied with `QEMU_BASE_INITRAMFS`, or can be prebuilt with
   `melange initramfs --arch ... --output ... --init-package ... --package ...`
   (`docs/md/melange_initramfs.md`).
3. **The kernel is the host kernel.** `getKernelPath` uses `/boot/vmlinuz` or
   `QEMU_KERNEL_IMAGE`. Melange does not take the kernel from the guest
   rootfs. Cross-architecture builds therefore need a matching kernel supplied
   by the environment.
4. **Per-session data is injected into the initramfs.** `injectRuntimeData`
   appends the VM's SSH host private key and optional kernel modules
   (`QEMU_KERNEL_MODULES`) to the cached initramfs. The per-session initramfs
   is securely deleted once the SSH clients are established.
5. **The guest rootfs is delivered as a block device.** QEMU attaches the
   apko rootfs tar as one virtio-blk device and a blank raw disk as another.
   `microvm-init` formats the scratch disk and extracts the rootfs onto it;
   the guest rootfs ends up at `/mount`. The build root is not booted.
6. **There are two SSH channels.** `setupSSHClients` connects a chrooted build
   channel on port 22 as the configured build user (`environment.accounts`,
   `run-as`) and an unchrooted root control channel on port 2223. The control
   channel mounts the cache and workspace share, copies files in and out, and
   performs lifecycle operations. The build channel runs pipeline steps
   inside `/mount`.
7. **Workspace and cache are shared, not copied through the session.** The host
   workspace is exposed with 9p (`defaultshare`), mounted read-only at
   `/mount/mnt`, and copied into `/home/build`. The cache uses 9p read-only
   with an overlay, or virtiofs when available. The code comments note that 9p
   is slow for many small files and that artifacts are therefore copied back
   at the end. `WorkspaceTar` retrieves the finished tree with tar.

The important structural point: **melange never boots the build root.** A small
transport initramfs runs the kernel, SSH and the tooling that populates and
enters the guest rootfs.

Comparison at a glance:

| Concern | melange | rpmx today | rpmx target |
|---|---|---|---|
| Build environment | apko guest rootfs from `environment.contents` | dnf root from profile + plan | Same; unchanged by the runner. |
| Transport | Separate initramfs (`microvm-init`) + host kernel | The build root itself | Separate appliance root + target kernel. |
| Build root bootable | No | Yes | No. |
| Kernel | Host `/boot/vmlinuz` or `QEMU_KERNEL_IMAGE` | Target kernel in the build root | Target kernel in the appliance lock. |
| Workspace | 9p read-only, copied in and out | Tar over SSH | Tar over SSH, no 9p. |
| Cache | 9p overlay or virtiofs | None | None. |
| Channels | Chrooted build + unchrooted control | One sshd, no chroot | Two ports: chrooted build + unchrooted control. |
| Appliance artifact | Cached initramfs, `melange initramfs` | Implicit in the build root | Locked appliance root, `rpmx runner`. |

### 9.2 What this validates

- The separation is practical and mature, not an exotic extra.
- The build environment can be a plain package closure; the transport does not
  need to appear in it.
- The appliance can be cached and prebuilt, with only per-session keys and
  modules injected.
- A control channel outside the chroot is useful: it can mount, copy and sync
  while the build channel is confined to the build root.
- Host-key verification against a generated `known_hosts`, secure deletion of
  key material, and SSH keepalives are all worth copying.

### 9.3 What rpmx should adopt

| Melange pattern | Adopt in rpmx |
|---|---|
| Transport separate from the build root | Already the core of this design. |
| Cached/prebuilt appliance plus per-session injection | Add an explicit `rpmx runner` build/inspect command and cache; keep the `ApplianceLock`. |
| Chrooted build channel plus unchrooted control channel | Adopted in D10 and section 5.4: two ports, `ChrootDirectory /run/rpmx/root` for the build port; transfers use the control port. |
| Host-key verification via `known_hosts` | Adopted: write the generated host public key to a temporary `known_hosts` and use `StrictHostKeyChecking=yes`, replacing today's `StrictHostKeyChecking=no`. |
| Secure deletion of the initramfs and key material | Adopted: zero the per-session keys and remove the appliance disk copy; never log the key. |
| SSH keepalives and configurable start timeout | Adopted: keepalives plus a configurable start timeout instead of a hard-coded 180 seconds. |
| Appliance package/repository overrides | Adopted through `--appliance-lock` and the future prebuilt-appliance image; not through the build profile. |

### 9.4 What rpmx should do differently

| Melange choice | rpmx choice | Why |
|---|---|---|
| Host kernel (`/boot/vmlinuz` or `QEMU_KERNEL_IMAGE`) | Target-architecture kernel pinned in the appliance lock | rpmx must build foreign architectures without requiring a matching host kernel. Melange needs an external kernel for cross-architecture; the lock makes rpmx self-contained and reproducible. |
| Initramfs-only appliance (`microvm-init` from an APK repository) | Full appliance root (systemd, sshd, tar) assembled through the existing RPM path | Reuses dnf, root assembly and systemd units, and needs no new architecture-specific init binary. An initramfs-only appliance is a valid later optimization. |
| 9p/virtiofs for workspace and cache | Tar over SSH, no host share | No host filesystem driver or shared host path in the guest. Melange's own code notes the 9p cost for many small files; its final artifact transfer is already tar. |
| apko rootfs tar extracted in-guest | Host-built ext4 image with `mke2fs -d` | Keeps the current disk path and host tooling. In-guest extraction is a possible simplification if `mke2fs`/`debugfs` become a burden. |
| Build user from `environment.accounts.run-as` | Steps run as root for now | rpmx recipes assume root; changing that is a separate sandbox design. The control/build split still applies. |
| `melange initramfs` prebuilds an APK initramfs | `rpmx runner` builds an RPM appliance lock/root | Same operational idea, different package format and a lock instead of a package list. |

### 9.5 Lessons for the plan

1. The two-channel split is not optional polish; it is how the build stays
   inside the build root while the control side still has the privileges it
   needs. Section 5.4 now reflects it.
2. The appliance must be a first-class, cacheable artifact with a prebuild
   command, not an implicit side effect of a build.
3. Per-session secrets belong in an injected copy, not in a cached appliance,
   and must be destroyed afterwards.
4. The host kernel shortcut is acceptable for native builds but not for
   rpmx's foreign-architecture contract. The appliance lock keeps the target
   kernel explicit.
5. The sync-out tar stream is attacker-controlled output. Melange has had a
   QEMU tar path-traversal advisory (`GHSA-qxx2-7h4c-83f4`) and an issue where
   its tar did not preserve setuid/setgid/xattrs. rpmx keeps `tar_guard` on the
   control channel, and must state explicitly whether xattrs/ACLs are
   preserved or unsupported.

## 10. Alternatives considered

| Alternative | Verdict |
|---|---|
| **Keep booting the build root** | Simplest; fails I1–I6. Keep only as the Phase 2 fallback. |
| **Add the boot set implicitly to every root** | Cleans the profile, but still masks dependencies, still bloats native roots, still puts dracut in `root_digest`. Reject. |
| **Add the boot set implicitly only for QEMU** | Forks the build environment by runner and breaks root reuse unless the requirement digest also forks. This is the current problem in a different place. Reject. |
| **Appliance + second disk + chroot** | Meets the contract with existing machinery. Recommended. |
| **Boot the build root, but take sshd/tar from an initramfs** | Still needs an appliance; building a custom initramfs is more code than booting a small RPM root. A valid implementation variant if the appliance disk turns out to be too slow. |
| **9p or virtiofs share for the build root** | No second disk, but shares host paths, contradicts the no-share design and adds a host filesystem driver to the attack surface. Reject. |
| **Prebuilt signed appliance image per arch** | Best UX and no per-profile resolve; adds a release artifact, signing and a distro-version policy. Keep as a future optimization behind the same `ApplianceLock` interface: replace `packages` with an image digest. |
| **Run the build root as a container inside the VM** | Adds podman to the appliance and a second layer; the chroot already gives the required filesystem view. Not now. |

## 11. Open questions

1. **Kernel provenance.** Should the appliance use the target distro's kernel
   from the profile repositories (recommended, and what this design does), or a
   kernel shipped with rpmx? The former keeps `releasever` semantics; the
   latter removes a resolve and makes the appliance a pure runner artifact.
2. **Appliance trimming.** `kernel-modules` is the easy path. A smaller set
   (`kernel-modules-core` plus explicit virtio and ext4 modules in dracut) is
   possible but needs its own boot fixture. Decide with measurements, not
   guesses.
3. **Tree digest.** This design records no appliance tree digest. If cache
   corruption is a concern, add a digest that excludes `/boot` and
   `var/lib/systemd`, and verify it only for explicit `--runner-asset-root`.
4. **Offline cross-arch rebuilds.** A native lock has no appliance. Producing
   an SRPM for another architecture should use `root resolve --appliance always`
   or `srpm --appliance-lock`. If this proves too easy to get wrong, make
   `rpmx srpm` always resolve/embed the appliance, at the cost of an extra
   resolve for native SRPMs.
5. **chroot escapes.** A root process can escape a chroot. The VM remains the
   boundary. If transport integrity against a hostile build becomes a goal,
   run steps as an unprivileged UID with a private PID/mount namespace. That is
   a separate design.
6. **Build-root writability.** QEMU still uses a disposable writable copy of
   the build root, so a step can modify `/usr`. The sandbox contract says the
   toolchain is read-only. Fixing this needs a separate writable disk or an
   overlay, and is out of scope here.
7. **SELinux and xattrs.** Not represented on the appliance disk or in the
   root digest today. The appliance does not change that.
8. **Prebuilt appliances.** If adopted, the `ApplianceLock` becomes one of two
   variants: a package closure or an image digest. Keep the boot protocol
   identical so the runner does not care which one it got.

## 12. Migration notes

- A profile that still lists the boot packages keeps working. The extra
  packages remain in the build root and are simply unused by the runner.
  Remove them from the examples and docs in Phase 3; do not add a warning that
  fails the build.
- A root lock without `appliance` still loads. A QEMU build with it fails with
  `RPMX-ROOT-020` and a hint to run `rpmx root resolve --appliance always` or
  pass `--appliance-lock`.
- An SRPM produced before this change has no appliance in `root.lock.json`.
  Rebuilding it for a foreign target needs an appliance from a repository or a
  supplied `--appliance-lock`; a native rebuild is unaffected.
- `root export` and `root import` keep their current shape. The appliance is
  not part of the libpkgmanifest boundary.
- The CAS gains the appliance payloads and the cache gains one appliance root
  per (repositories, releasever, arch, schema). `rpmx cache gc` should treat
  them like roots: referenced by a lock or a known appliance identity.
