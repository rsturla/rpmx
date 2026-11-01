# Build sandboxing: contract, backends and threats

## 1. Why this is the next layer

The source and SRPM model decides *what* gets built. The sandbox decides *what the build can do while it runs*. A malicious recipe, a compromised dependency, or a malicious generator gets arbitrary code execution before the package exists. Namespaces are not a security policy by themselves; the policy is the combination of filesystem, network, user, syscall, resource and lifetime controls.

The design should start from a contract, then pick an implementation.

## 2. Sandbox contract

Every native build must satisfy these properties:

| Property | Requirement |
|---|---|
| **Fresh root** | The build starts from a verified build root, not the host filesystem. |
| **Read-only toolchain** | `/usr`, `/lib`, `/bin` are read-only; only declared writable areas exist. |
| **No host home** | No `$HOME`, SSH agent, keyring, browser profile or user config. |
| **No network** | No interfaces except loopback; DNS disabled; fetch happens earlier. |
| **No credentials** | No registry tokens, signing keys, cloud credentials or proxy secrets. |
| **No host devices** | Only minimal `/dev` nodes (`null`, `zero`, `random`, `urandom`, `tty` if needed). |
| **No host IPC** | Private PID, IPC, UTS, mount and network namespaces. |
| **No privilege escalation** | `no_new_privs`; all capabilities dropped; no setuid. |
| **Resource limits** | CPU, memory, pids, open files, file size, disk/tmpfs, wall time. |
| **Deterministic environment** | Fixed `PATH`, `HOME`, `LC_ALL`, `TZ`, `umask`, `SOURCE_DATE_EPOCH`; no hostname. |
| **Writable only where intended** | Build tree, staging root and a size-limited `/tmp`. |
| **Auditable configuration** | The sandbox config digest goes into the provenance attestation. |
| **Fail closed** | If the sandbox cannot be established, the build fails; `--no-sandbox` is explicit and recorded. |

The QEMU backend is the one deliberate exception to the interface wording in
the network row: it has a single host-only NIC so the harness can reach SSH.
`restrict=on` gives the guest no route to the host network or the internet.

## 3. Backends

| Backend | Isolation | Rootless | Resource limits | OCI rootfs | Complexity | Good for |
|---|---|---|---|---|---|---|
| **bubblewrap** | namespaces, mounts, seccomp, `no_new_privs` | Yes (user namespaces) | No native cgroups; pair with systemd-run/cgroup v2 | Yes, via a prepared rootfs | Low | Default local/CI single-tenant builds; melange/Flatpak model |
| **nsjail** | namespaces, cgroups, rlimits, seccomp/Kafel | Yes/partly | Strong, built in | Via chroot/bind mounts | Medium | Security-focused CI, fuzzing, multi-tenant |
| **rootless Podman** | OCI runtime, namespaces, seccomp, cgroups | Yes | Yes (cgroups v2; limitations on v1) | Native | Medium | CI, OCI build roots, teams already using containers |
| **systemd-nspawn** | full OS container, namespaces | With `--private-users` | cgroups | Yes | Medium | Distro-like build roots; systemd-centric infra |
| **mock/chroot** | chroot + namespaces (version-dependent) | No/limited | cgroups via systemd | No | Low | Compatibility fallback only; not a security boundary |
| **gVisor (`runsc`)** | user-space application kernel | Yes | OCI runtime limits | Yes | High | Untrusted multi-tenant builds, strong isolation |
| **Kata/Firecracker** | VM-level | Varies | VM limits | Yes | High | Highest-assurance multi-tenant |
| **Landlock/seccomp only** | filesystem/syscall filters | Yes | No | No | Low | Defense in depth, not a full sandbox |

### 3.1 Reference: how melange does it

Melange is the closest working reference. Its build path is:

1. **Assemble a guest rootfs with apko.**
   `environment.contents` lists APK repositories, keyrings and packages. Melange asks apko for a tar stream of that package set and lays it out into a temporary **guest directory**. This is the future `/` of the build.
2. **Create a workspace.**
   The source tree is copied to a separate **workspace directory**, then bind-mounted into the guest at `/home/build`. The original source directory is not the build root.
3. **Run each pipeline step inside the guest.**
   `runs:` steps execute inside a bubblewrap container whose root filesystem is the guest directory. `uses:` steps expand to other pipelines. A step can declare `needs.packages` (added to the build environment) and `needs.capabilities` (added to the runner).
4. **Use the runner abstraction.**
   `melange build --runner` supports `bubblewrap`, `docker` and `qemu`; the default depends on the platform. Bubblewrap is the native Linux path. QEMU gives a VM boundary and uses binfmt for foreign architectures.
5. **Use binfmt/QEMU for other architectures.**
   Native-architecture builds run directly; foreign-architecture builds use `binfmt_misc` user-mode emulation (or the QEMU runner).
6. **Mount a build cache.**
   `--cache-dir` mounts a host directory at `/var/cache/melange`. Docker and bubblewrap mount it read-write; the default QEMU runner uses a read-only 9p mount plus an overlay, with virtiofs available for persistence.
7. **Test in a fresh guest.**
   `melange test` builds a fresh apko guest containing the package under test plus declared test dependencies, then runs the test pipeline. Capabilities declared by a test are scoped to that test’s runner, not the build runner.

What is good:

- the build root is a **constructed package set**, not the host;
- the source tree is copied, not built in place;
- bwrap provides user/mount/PID/IPC/UTS/network namespaces and `no_new_privs`;
- QEMU provides a stronger boundary for untrusted or foreign-arch builds;
- tests run in a fresh guest, not the build tree;
- the guest is disposable.

What is weaker than the design here:

- **Network is not inherently disabled.** The pipeline can call `curl` or a language package manager; melange relies on policy and the fetch steps, not a network namespace.
- **Recipes can request capabilities.** `needs.capabilities.add: CAP_SYS_ADMIN` is additive and runner-wide for the build. A malicious recipe can ask for more privilege; that is a policy decision.
- **The cache is a writable bind mount** for Docker/bubblewrap. A malicious build can poison the cache for later builds unless the cache is treated as untrusted.
- **The workspace is writable and bind-mounted.** If configured to point at the source directory, build changes persist; that is convenient for development but not hermetic.
- **`environment.contents` is a package list, not a content-addressed lock.** apko resolves it; reproducibility depends on repository state.
- **Bubblewrap is not a complete sandbox by itself.** Melange’s container image is often run with elevated privileges because bwrap needs them in that environment; the host deployment matters.

The design here keeps the good parts — apko-style rootfs, bwrap, fresh guest tests — and moves capabilities, cache trust and network policy into the profile rather than the recipe.

## 4. Recommendation

Use a **layered profile**, not one backend forever:

1. **Default local and single-tenant CI: bubblewrap + cgroups v2 + seccomp.**
   - Bubblewrap is small, unprivileged and proven in Flatpak/melange.
   - It gives mount, PID, IPC, UTS, user and network namespaces.
   - It sets `no_new_privs`; use `--new-session` to mitigate TIOCSTI.
   - It does not provide cgroup limits by itself; run it under `systemd-run --user --scope` or an equivalent cgroup v2 wrapper.
   - Bubblewrap explicitly says it is not a complete sandbox; the arguments are the policy.

2. **CI with OCI build roots: rootless Podman (or another rootless OCI runtime).**
   - Native OCI image handling, cgroups v2, seccomp, `--network none`, `--read-only`, `--cap-drop all`, `--security-opt no-new-privileges`.
   - Good when the build root is already an OCI image digest.
   - Rootless resource limits depend on cgroups v2; document the fallback.

3. **Untrusted multi-tenant or high-assurance: gVisor or a microVM.**
   - gVisor’s user-space kernel reduces host-kernel attack surface and is a real security boundary, at the cost of syscall compatibility and performance.
   - Kata/Firecracker give VM isolation when the threat model includes kernel exploits or hostile tenants.
   - Do not make this the default; it is slower and more operationally complex.

4. **Compatibility only: mock/chroot.**
   - Useful for legacy builds that cannot run in the native sandbox.
   - Not a security boundary; chroot escapes and shared kernel are real.
   - Mark the package/provenance as `sandbox = compatibility`.

5. **Unsafe: `--no-sandbox`.**
   - Allowed only for bootstrap/debugging.
   - Recorded in provenance; profiles can forbid it.

### 4.1 Default shape: melange-like, with network off

The default build shape should be directly modeled on melange:

| Concern | Melange | `rpmx` default |
|---|---|---|
| Build root | apko-composed guest directory | OCI image or resolved RPM package set, laid out as a guest root |
| Source | copied to a workspace, bind-mounted at `/home/build` | copied from the verified source snapshot into `/build` |
| Runner | bubblewrap / Docker / QEMU | bubblewrap by default; rootless Podman for OCI-root CI; QEMU/gVisor for high assurance |
| Network | available to pipeline steps | **network namespace with no external interfaces by default** |
| Capabilities | recipe can request `needs.capabilities` | profile allowlist only; recipes cannot request capabilities |
| Cache | `--cache-dir` read-write for bwrap/Docker | read-only by default; writable only for explicitly trusted caches |
| Tests | fresh apko guest per test | fresh minimal root per installed test |

A representative bwrap invocation:

```sh
bwrap \
  --unshare-all \
  --die-with-parent \
  --new-session \
  --ro-bind "$ROOTFS" / \
  --dev /dev \
  --proc /proc \
  --tmpfs /tmp \
  --bind "$BUILD" /build \
  --bind "$STAGE" /stage \
  --chdir /build \
  --clearenv \
  --setenv PATH /usr/bin:/bin \
  --setenv HOME /build/home \
  --setenv LC_ALL C.UTF-8 \
  --setenv TZ UTC \
  --setenv SOURCE_DATE_EPOCH "$SOURCE_DATE_EPOCH" \
  -- /bin/sh -c "$COMMAND"
```

`--unshare-all` includes the network namespace. **Never add `--share-net` for source assembly or build.** Do not mount host `/etc/resolv.conf`, do not pass proxy variables, and do not expose a host network interface.

The guest rootfs is read-only. `/build` (source copy), `/stage` (package root) and `/tmp` (size-limited tmpfs) are the only writable locations.

**Current prototype:** the runner binds the host root read-only and mounts the
workspace and stage under `/tmp/build` and `/tmp/stage`. That is enough to run a
fixture, but it is not the final guest-root model: the host toolchain is visible,
and the build root is not resolved or digest-pinned yet.

`/sys` must be a fresh or empty mount, not the host's. `--unshare-all` unshares
the network namespace, but a host `/sys` bind still exposes the host's interface
names (confirmed in [`spikes/03-bwrap`](../spikes/03-bwrap/README.md)). Mount
`--tmpfs /sys` or use a guest root that already has its own. Loopback remains
available and reachable, which installed tests need; external egress is blocked.

## 5. What goes in the sandbox config

### Filesystem

```
/                 read-only build root
/usr, /lib, /bin  read-only toolchain
/build            writable build directory
/stage            writable staging root
/tmp              tmpfs, size-limited
/proc             private procfs, hidepid where possible
/dev              minimal device set
```

- No `/home`, `/root`, `/run`, `/var/run`, Docker socket, SSH agent, D-Bus, or host `/etc`.
- Bind mounts are `nodev`, `nosuid`, `noexec` where appropriate.
- Prefer a prepared rootfs/OCI image over bind-mounting host `/usr`.
- Use idmapped mounts or user namespace mapping for ownership, not host root.

### Network

**External network is off by default.** The build and source-assembly sandboxes get a new network namespace with only loopback.

- `bwrap`: use `--unshare-all` (or at least `--unshare-net`); never `--share-net`.
- `podman`: `--network none`.
- QEMU: one host-only NIC with `restrict=on` and a hostfwd rule for port 22.
  It exists solely so the harness can drive the guest. It provides no route to
  the host network or the internet. The no-network test therefore checks
  reachability, not interface count.
- No DNS; do not mount host `/etc/resolv.conf`.
- No proxy environment variables in the build.
- Fetch is a separate phase and may reach only the acquisition proxy.
- Source assembly and build phases are networkless.
- Tests are networkless by default; a profile may opt into a separate network-enabled test runner with explicit policy.

Loopback is allowed for tests and build steps that need a local service. External interfaces are not.

### Users and capabilities

- Run as an unprivileged UID inside the namespace; map to the invoking user outside.
- Drop all capabilities (`--cap-drop ALL`).
- `no_new_privs`; no setuid binaries.
- No host root, ever.

### Syscalls

- Use the container runtime’s default seccomp profile as a baseline.
- Add a small deny list for high-risk syscalls (`ptrace`, `bpf`, `perf_event_open`, `kexec`, `mount`/`umount` where not needed, `userfaultfd`, `keyctl`, `add_key`).
- Do not attempt a default-deny allowlist for arbitrary builds; it will break real toolchains and produce pressure to disable seccomp.
- Test the profile against the reference package corpus.

### Resources

- cgroups v2: `MemoryMax`, `PidsMax`, `CPUQuota`, `IOWeight`.
- rlimits: `NOFILE`, `FSIZE`, `CORE=0`, `NPROC`.
- Wall-clock timeout; kill the whole process group.
- Size-limited `/tmp`; disk quota for build/stage.
- OOM behavior: fail the build, do not let the kernel kill the host.

### Environment

```
PATH=/usr/bin:/bin
HOME=/build/home
LC_ALL=C.UTF-8
TZ=UTC
SOURCE_DATE_EPOCH=<from lock>
umask=022
```

- Clear inherited environment.
- No hostname; set a fixed UTS hostname.
- No `RPM_*` macro soup; only documented variables.
- Fixed working directory (`/build`).
- No random or host-specific values.

## 5.1 Runner interface

The sandbox expands templates and builds the deterministic environment; the
runner only executes the resulting command. `SandboxConfig` carries the policy
and the runner choice; the runner receives only `RunnerConfig`. The interface
is:

```rust
pub struct SandboxConfig<'a> {
    pub runner: RunnerKind,
    pub host: Arch,
    pub target: Arch,
    pub workspace: &'a Path,
    pub stage: &'a Path,
    pub root: Option<&'a Path>,
    pub image: Option<&'a str>,
    pub source_date_epoch: u32,
    pub jobs: u32,
    pub timeout: Option<u64>,
    pub debuginfo: bool,
}

pub struct RunnerConfig<'a> {
    pub host: Arch,
    pub target: Arch,
    pub workspace: &'a Path,
    pub stage: &'a Path,
    pub jobs: u32,
    pub timeout: Option<u64>,
}

pub struct RunnerRequest<'a> {
    pub command: &'a str,
    pub args: &'a [String],
    pub cwd: &'a str,
    pub env: &'a BTreeMap<String, String>,
    pub label: &'a str,
}

pub trait Runner {
    fn name(&self) -> &'static str;
    fn supports(&self, host: Arch, target: Arch) -> bool;
    fn start(&self, config: &RunnerConfig<'_>) -> Result<Box<dyn Session>>;
}

pub trait Session {
    fn run(&mut self, request: &RunnerRequest<'_>) -> Result<()>;
    fn sync_out(&mut self) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
}
```

`source_date_epoch` and `debuginfo` are policy fields: the sandbox applies them
to the environment and never passes them to a backend. `Sandbox::new` resolves
`RunnerKind` (`auto`, `bwrap` or `qemu`) into a runner and rejects contradictory
selections eagerly (bubblewrap with a foreign target and an assembled root, or
qemu without a root). The `supports` check itself is deferred to the first
session start, so `Sandbox::with_runner` cannot fail on the runner and
constructing a sandbox that never runs a step cannot fail that check. The
concrete implementations (`BwrapBackend`, `PodmanBackend`, `QemuBackend`) live
under `runner/backend/`; `sandbox` holds policy only.

`bubblewrap` and `podman` support native execution only. `qemu-system` starts
a full VM for a foreign target with a target kernel, initramfs and a writable
copy of the assembled root disk. Nothing is shared with the host: the
workspace and stage are copied in over SSH, and outputs are tarred back over
the same connection. The guest has one host-only NIC (`restrict=on`) used only
for the SSH port forward; it cannot reach the host network or the internet.
The sandbox checks `supports` before starting anything, so an unsupported
combination is `RPMX-RUNNER-002`, not a mysterious target binary failure.
Root assembly and test-root preparation use podman's `--arch` and the host
binfmt handler; build steps do not.

A bootable QEMU root needs a kernel, its modules, `dracut`,
`systemd-networkd`, `openssh-server` and `tar`. Because root assembly runs package
scriptlets, the kernel transaction generates `modules.dep` and the initramfs.
`rpmx` injects the SSH key, a DHCP `.network` unit and the SSH config into the
disk image, then runs every step through `ssh` and copies the workspace and
stage back with `tar`.

## 6. Stage-by-stage coverage

| Phase | Sandbox? | Network | Rootfs | Writable | What runs |
|---|---|---|---|---|---|
| **Fetch** | Yes, dedicated | Proxy only | Minimal | CAS/download dir | Network client, signature/digest verification. Credentials allowed here only. |
| **Source assembly** | Yes, separate | None | Generator/tool root | Source tree, snapshot | Vendoring, Git archive, deterministic generation, license collection. |
| **SRPM assembly/verification** | Tool sandbox | None | Minimal | Temp extraction dir | `rpmx`/librpm parses and verifies; no arbitrary build code. |
| **Prep** | Yes, build sandbox | None | Build root | `/build` | Unpack source snapshot, apply patches, `%prep`. |
| **Build/install/check** | Yes, build sandbox | None | Build root | `/build`, `/stage`, `/tmp` | Configure, compile, install, `%check` tests. |
| **Installed tests** | Yes, test sandbox | None by default | Fresh test root with produced RPMs | Test root, temp dirs | Install/upgrade/erase, scriptlets/actions, smoke/ABI tests. |
| **Package tests** | Yes, artifact sandbox | None | Minimal | Temp dir | Inspect RPMs/SRPMs, manifests, signatures, reproducibility. |
| **Signing** | No, trusted signing service | No | Host service | Signing key | Policy checks, signing, attestation. Keys never enter a build sandbox. |

### 6.1 Is `%prep` sandboxed?

Yes. `%prep` runs inside the build sandbox, with:

- the verified source snapshot copied into `/build`;
- the build root read-only except `/build`;
- no network;
- no credentials;
- a timeout and resource limits.

`%prep` is untrusted code from a security perspective. It unpacks archives and applies patches. A malicious patch can try path traversal, symlink tricks, absolute paths or writing outside the source tree. The sandbox contains that, but the unpacker/patcher must also be safe:

- extract into a fresh directory;
- reject absolute paths and `..`;
- reject escaping symlinks and device files;
- apply patches with a fixed strip level and no fuzz by default;
- keep the source snapshot immutable; patch a copy in `/build`.

`%prep` is post-SRPM. It is distinct from **source assembly**, which happens before the SRPM and handles vendoring/generated source. Both are sandboxed; they use different roots and different writable areas.

### 6.2 Are tests sandboxed?

Yes, but there are three kinds:

1. **Build tests** (`stage: build`): run inside the build sandbox after `build` or `install`. No network, no credentials, same limits as the build.
2. **Installed tests** (`stage: installed`): install the produced RPMs into a **fresh disposable test root**, then run the test in a separate sandbox. This is where scriptlets and typed actions execute. It must never run against the host root.
   - The test root contains only the package under test, its runtime dependencies, and declared test dependencies.
   - No network by default; loopback only.
   - Scriptlets run in the disposable root under the test sandbox, not on the host.
   - Test the install, upgrade and erase paths, not just the happy path.
3. **Package tests** (`stage: package`): inspect the produced RPMs/SRPMs, manifest, signatures and reproducibility. Run in a minimal artifact sandbox; no network.

Tests are untrusted code. They can fork bombs, exhaust memory, read whatever is mounted, or exploit the kernel. Apply the same sandbox contract as the build, with fresh roots and resource limits.

**Network for tests** is off by default. If a test genuinely needs external network, a profile can opt into a separate network-enabled test runner. That runner is not the build sandbox, and the opt-in is recorded in provenance.

The source-assembly sandbox is not optional. It is the boundary between “we fetched/generated source” and “we compiled it.” `%prep`, build and tests are not optional sandboxes either; they are where untrusted code actually runs.

## 7. Cross-compilation and special cases

- Cross builds use a target sysroot in the build root and `qemu-user`/binfmt only if needed.
- Native builders are preferred for performance and to avoid emulation bugs.
- Kernel modules, bootloaders, setuid binaries and FUSE may need a **privileged profile**:
  - separate VM or dedicated builder;
  - never host root;
  - explicit policy allowlist;
  - full audit and provenance.
- GPU builds need device passthrough; treat as a privileged profile, not a default.

## 8. Verification and testing

Every sandbox backend must pass a common test suite:

1. **Network isolation:** `curl`/`ping`/DNS fail; only loopback exists.
2. **Filesystem isolation:** host `/etc/shadow`, `/home`, `/run`, SSH sockets are absent.
3. **Process isolation:** host PIDs are invisible; no `ptrace` of host processes.
4. **Capability isolation:** `capsh --print` shows no effective capabilities; setuid fails.
5. **Resource limits:** memory, pids, file size and wall time are enforced.
6. **Write restrictions:** `/usr` is read-only; only `/build`, `/stage`, `/tmp` are writable.
7. **Determinism:** two runs produce the same output given the same inputs.
8. **Escape tests:** known container-escape patterns fail; keep the test suite current.
9. **Reproducibility across backends:** bwrap and Podman produce identical unsigned outputs for the same package.

## 9. Implementation notes

- Every step starts from a cleared environment. The sandbox sets only `PATH`,
  `HOME=/tmp/build`, `TMPDIR=/tmp`, `LANG`/`LC_ALL=C.UTF-8`, `TZ=UTC`,
  `SOURCE_DATE_EPOCH`, `SRC`, `BUILD`, `STAGE` and `JOBS`. A step may add
  toolchain variables but cannot set the reserved contract variables
  (`SOURCE_DATE_EPOCH`, `SRC`, `BUILD`, `STAGE`, `JOBS`).
- bwrap runs with `--unshare-all --unshare-user --disable-userns`, so a step
  cannot create a nested user namespace. This needs bubblewrap >= 0.9; the
  sandbox fails closed if bwrap is absent or too old.
- Every step has a wall-clock limit (`--step-timeout`, default 3600 seconds,
  `0` disables it). GNU `timeout` kills the bwrap process group; the container
  backend also records a cidfile so a timed-out container is killed explicitly.
  A timeout is `RPMX-SANDBOX-004`. Memory, CPU and PID limits beyond the
  container's `--pids-limit` are still missing.
- `profile.sandbox` selects the backend and limits; the recipe cannot request privileges.
- `rpmx sandbox check` prints what the host supports (user namespaces, cgroups v2, Landlock, bwrap, Podman, gVisor).
- `rpmx shell --stage build` enters the sandbox for debugging; it is interactive and not used in CI.
- The sandbox configuration digest is recorded in `RPMX-MANIFEST.json` and provenance.
- `--no-sandbox` is refused by default in profiles that require isolation.
- Build logs record the backend, kernel, bwrap/runtime version and config digest.

## 10. What to research and benchmark next

- **bubblewrap + systemd-run** vs **rootless Podman** on the reference packages: wall time, memory, I/O, compatibility.
- **cgroups v2 delegation** in rootless environments across Fedora, Debian/Ubuntu and RHEL.
- **User namespace restrictions** (Ubuntu AppArmor, RHEL sysctls) and fallbacks.
- **seccomp profiles** for real toolchains (GCC, Clang, Rust, Go, Java, Python, Node).
- **gVisor compatibility** with build tools and syscall-heavy language runtimes.
- **Privileged builds** (kernel modules, bootloaders) in microVMs.
- **Reproducibility across sandbox backends.**
- **Attack surface** of bind mounts, `/proc`, `/sys`, `/dev` and idmapped mounts.

## 11. Open questions

1. Default backend: bubblewrap or rootless Podman? (Current recommendation: bubblewrap for local/single-tenant, Podman for OCI-root CI.)
2. How much seccomp filtering can real builds tolerate?
3. Do we require cgroups v2, or support cgroups v1 with weaker limits?
4. Is `mock` compatibility worth supporting, or should legacy builds require `--no-sandbox`?
5. How do we handle packages that genuinely need privileged operations without making the default unsafe?
6. Do we need a microVM for all CI, or only multi-tenant/high-assurance?
7. How do we verify the sandbox itself (escape tests, kernel version, runtime CVEs)?
8. What is the performance budget for a sandboxed build versus a bare build?
