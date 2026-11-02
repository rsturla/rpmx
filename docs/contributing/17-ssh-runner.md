# SSH runner: executing locked-root builds on a remote host

Status: design proposal, 2026-11-01. Nothing in this document is implemented
yet.

Related documents:

- [`09-sandboxing.md`](09-sandboxing.md) — the sandbox contract and the
  currently documented runner exceptions.
- [`14-build-root.md`](14-build-root.md) — resolution, assembly, locking and
  the root digest.
- [`16-qemu-appliance.md`](16-qemu-appliance.md) — the runner/build-root
  separation this document must not break.
- [`08-threat-model.md`](08-threat-model.md) — trust boundaries.

## 1. Summary

`rpmx` can already run steps in a local namespace (bubblewrap), a local
container (podman) or a locally spawned VM (qemu-system). This document adds a
fourth: **any host `rpmx` can reach over SSH**, whether that is a cloud VM, a
long-lived build machine, a CI-provisioned instance or a container with an SSH
server.

The host is treated as an execution and transfer endpoint, not as a build
environment. The build root stays a function of the recipe, the profile and the
target architecture; the runner changes how a step executes, not what the step
sees. That is the invariant from
[`16-qemu-appliance.md`](16-qemu-appliance.md).

Because SSH hosts differ in what they offer, the backend does not assume one
shape. It detects what the host can do and enters the uploaded root through one
of four **execution adapters**:

| Adapter | Requires on the host | Isolation | Root lock |
|---|---|---|---|
| `podman` | rootless or rootful podman | network, caps, PID, tmpfs, cgroups | honoured |
| `bwrap` | `bubblewrap`, unprivileged user namespaces | namespaces | honoured |
| `chroot` | root | filesystem only | honoured |
| `host` | nothing | none | **advisory** |

`auto` selects the best available adapter and refuses to fall back to `host`.
`host` is an explicit, recorded escape hatch for a machine that offers nothing
else, and in that mode the locked root is not used.

### 1.1 Decisions at a glance

| # | Decision |
|---|---|
| **S1** | SSH is a transport. The build root remains derived from recipe + profile + target; the runner never contributes packages to it. |
| **S2** | Steps are entered through an execution adapter (`podman`, `bwrap`, `chroot`, `host`). `auto` prefers `podman`, then `bwrap`, then `chroot`, and fails rather than silently degrading. |
| **S3** | The locked root is delivered per adapter: a container rootfs for `podman`, a directory for `bwrap` and `chroot`, and nothing for `host`. `podman import` versus an extracted `--rootfs` is decided by a spike (SSH-0), not by this document. |
| **S4** | Root **resolution and assembly stay on the control host** in Phase 1. They require local `podman`, `dnf5` and, for a foreign target, binfmt. Moving them to the remote is Phase 2 (SSH-12) and is the only way to drop the local binfmt requirement. |
| **S5** | The remote architecture is probed (`uname -m`) and must equal `--arch`. A mismatch fails with `RPMX-RUNNER-002` **before** any step. |
| **S6** | Connection settings come from flags, the operator's SSH config, or `RPMX_SSH_*` variables only. Recipes and profiles cannot set them, exactly as they cannot carry repository credentials. |
| **S7** | Host keys are verified by default against the operator's `known_hosts`. Accepting a key on first use is an explicit opt-in; pinning a file or a public key is supported. `StrictHostKeyChecking=no` is never used. |
| **S8** | A timed-out step kills the execution context and recreates it. Closing the SSH connection does not kill a `podman exec`. |
| **S9** | `sync_out` tars the remote workspace and stage back over the control connection, reusing the existing guard and 8 GiB cap. |
| **S10** | `--runner auto` never selects SSH. The endpoint is an explicit operator decision. |
| **S11** | One SSH channel. The remote shell only ever runs the tools listed in I4; build steps never execute in the remote user's shell (except in the `host` adapter, where that is the point). |
| **S12** | Delivered roots are cached on the host by `root_digest`. Ephemeral hosts discard it with the user or instance; long-lived hosts keep it until `rpmx ssh prune` or the operator's podman policy removes it. |
| **S13** | `--ssh-execution=host` is explicit, warns, and marks the build as not locked. It never happens through `auto`. |

## 2. Host shapes this must cover

The design is not tailored to one controller. These are the shapes it has to
serve; the acceptance plan exercises at least three of them.

| Shape | Typical offer | Adapter |
|---|---|---|
| Konflux `multi-platform-controller` | fresh unprivileged user per `TaskRun`, rootless podman installed, one-time SSH key, arbitrary architecture, per-user cgroup limits, user deleted at the end | `podman` (rootless) |
| Cloud VM from a CI template | Ubuntu/Fedora image, an SSH key, `sudo` or root, no podman | `chroot` or install `podman` |
| Long-lived build machine | a dedicated unprivileged user, podman or bwrap preinstalled, shared by many builds | `podman` or `bwrap` |
| Container with `sshd` | one user, no root, minimal image | `bwrap` if namespaces are allowed, otherwise `host` |
| Bare-metal runner with root | root over SSH, no daemon | `chroot` (or `podman`) |

Two facts about the Konflux controller are still worth recording because they
shape the `podman` adapter: its provisioning script creates the user with
`useradd -m`, which does **not** by itself create a `/etc/subuid` range, and its
secret carries a host name and a key but no host key. Neither is a global
assumption; both are preflight checks (§5.2) and a documented default (§9.2).

## 3. Execution adapters

### 3.1 The contract an adapter must satisfy

Given an uploaded root at a known directory, an adapter must:

1. enter that directory as `/` for a step;
2. see the workspace at `rpmx_base::guest::BUILD` and the stage at
   `rpmx_base::guest::STAGE`, writable;
3. preserve file modes and ownership as `tar` does;
4. kill every process from a timed-out step;
5. be removable without leaving host state behind, except the documented cache.

Whatever else an adapter adds is policy, and the delta is recorded per adapter.

### 3.2 `podman`

The preferred adapter. It works rootless or rootful, needs no host root, and
gives the properties the sandbox contract asks for. The local `PodmanBackend`
already encodes the intended flags; the remote variant reuses them and adjusts
two things by detection:

- `--userns=keep-id` only when the remote user is not root; rootful podman uses
  its default mapping.
- SELinux relabelling (`:Z`) only when the host reports SELinux enforcing; on
  an unlabelled or disabled host it is unnecessary and can fail.

```text
podman run -d --rm --name rpmx-<session>
  --network=none
  --read-only
  --cap-drop=all
  --security-opt=no-new-privileges
  --pids-limit=1024
  [--userns=keep-id]
  --tmpfs /tmp:rw,size=2g
  -v <session>/workspace:/tmp/build[:Z]
  -v <session>/stage:/tmp/stage[:Z]
  <root-image> /bin/sh -c 'trap : TERM INT; sleep infinity & wait'
```

`--pids-limit` and the tmpfs size need cgroup v2 delegation when rootless. If
the probe cannot confirm them, the runner reports the missing capability and
either degrades to `bwrap` (when `auto`) or fails; it does not silently drop the
limit.

### 3.3 `bwrap`

For hosts that allow unprivileged user namespaces and have `bubblewrap`. It is
the same argv the local `BwrapBackend` builds, with the uploaded root bound at
`/` and the remote workspace and stage bound at the guest paths. No daemon, no
cgroups, no network namespace.

It is the best choice when the host has no podman and no root but does allow
user namespaces, and it keeps the read-only toolchain that `chroot` cannot.

### 3.4 `chroot`

For hosts where `rpmx` connects as root and nothing else is installed. The
uploaded root is chrooted into, with `/proc`, `/dev` and `/sys` bind-mounted if
the host allows it. This gives dependency isolation, **not** a security
boundary: a root process can escape a chroot, and the network is not isolated.
The host itself is the boundary, exactly as
[`16-qemu-appliance.md`](16-qemu-appliance.md) §5.5 says for its build channel.

### 3.5 `host`

The escape hatch. Steps run in the remote user's own filesystem, in a session
directory, with no root delivery and no isolation. The locked root is ignored;
the recipe's build dependencies are whatever the host has, and
`check_build_tools` is the only thing that will notice a missing one.

It exists because some hosts (a minimal CI container with `sshd`, a vendor
appliance) offer nothing else, and refusing to use them forces the operator to
invent a private wrapper. It must be requested explicitly, it prints a warning,
and `--ssh-execution=host` is recorded in the build output so the provenance is
not silently unlocked.

### 3.6 Selection

`--ssh-execution auto|podman|bwrap|chroot|host`, default `auto`:

1. probe in the order `podman`, `bwrap`, `chroot`;
2. select the first that passes its precondition;
3. if none passes, fail with `RPMX-RUNNER-005` naming what was missing for each
   and suggesting `--ssh-execution=host` only as an explicit, documented
   choice;
4. an explicitly named adapter that fails its precondition is an error, never a
   fallback.

The probe result is cached for the invocation, not across runs.

## 4. Invariants

| ID | Invariant |
|---|---|
| **I1** | The build root depends only on the recipe, the profile and the target architecture. It never depends on the runner or the remote host. |
| **I2** | The same closure produces the same `root_digest` under bwrap, podman, QEMU and SSH adapters that use the root. |
| **I3** | The SRPM's `root.lock.json` never names remote-only packages. |
| **I4** | A remote host package cannot satisfy a build dependency of a locked build. The only host tools the runner uses are `sh`, `uname`, `nproc`, `df`, `mkdir`, `rm`, `tar`, `timeout`, plus whatever the selected adapter requires (`podman`, `bwrap`, `chroot`). |
| **I5** | A runner changes how a step executes, not what the step can see. `--ssh-execution=host` is the documented exception, and it is not a locked build. |
| **I6** | The control host keeps nothing new beyond its existing cache. A long-lived remote host may keep a tag-keyed root cache, bounded by `rpmx ssh prune`. |
| **I7** | A recipe, profile or lock cannot influence which host is used or how it is authenticated. |

## 5. Architecture

### 5.1 Shape

```
control host                                  remote host (SSH)
------------                                  -----------------
rpmx build
  | resolve + assemble root (local)            |
  | verified root_digest                       |
  |                                            |
  +--- ssh: probe arch, tools, adapter ------->|
  +--- ssh: mkdir <work>/<session> ----------->|
  +--- ssh: stream workspace + stage --------->|
  +--- ssh: deliver root by digest ----------->|  root cache / image store
  +--- ssh: start adapter context ------------>|  container | namespace | chroot | dir
  +--- ssh: run step ------------------------->|  step executes in the root
  +--- ssh: stream workspace + stage back ---->|
  +--- ssh: stop context; rm session dir ----->|
      (root cache stays until pruned)
```

There is one SSH channel. The SSH session is the control channel; the adapter
is the execution boundary.

### 5.2 Session lifecycle

`SshSession::prepare(target, config)`:

1. Open a connection with `BatchMode=yes`, no agent forwarding, no PTY,
   keepalives, and host-key policy from §9.2. Honour the operator's SSH config
   (`~/.ssh/config` or `--ssh-config`) so `Host` aliases, `ProxyJump`,
   `IdentityFile` and `IdentityAgent` work. Retry until the
   `--ssh-connect-timeout` window expires; each attempt uses a short TCP
   timeout, because a just-provisioned VM may still be finishing boot.
2. Probe and validate the host: `uname -m`, `nproc`, `df -Pk`, the
   `tar`/`timeout`/`sh` minimum, and then the adapter preconditions. For
   `podman` that means `podman --version`, `podman info` (where a missing
   subuid/subgid range surfaces), cgroup and SELinux state. For `bwrap`,
   `bwrap --version` and a `--unshare-user` smoke test. For `chroot`, the
   effective UID and write access to the root cache. Free space is checked in
   the session directory **and** in the root cache or image store, because
   rootless `vfs` storage can need two to three times the root size. One
   message names every missing item, not the first.
3. Create `<work-dir>/<session-id>/` containing `workspace/`, `stage/` and
   `logs/`. `<work-dir>` defaults to `$HOME/.rpmx` expanded on the remote, not a
   literal `~`. The session id combines the process id and a counter, and is
   unique on a shared host.
4. Copy the host workspace and stage in with the existing streaming tar
   transport.
5. Deliver the root (§6). Skipped on a cache hit.
6. Start the adapter context.

`Session::run` executes one step in the adapter context, with the step script
on stdin so quoting and argument limits stay identical to the QEMU transport.

`sync_out` tars `workspace/` and `stage/` from the remote session directory. The
context is disposable; the bind-mounted directories are the output.

`stop` stops the context and removes the session directory. `Drop` repeats both
best-effort so a panic or an early error does not leak a context. `Drop` does
**not** run on `SIGINT` or `SIGTERM`; a cancelled `rpmx` leaves the context for
the operator's or the controller's cleanup. The runner documentation must say
that, and `rpmx ssh prune` (§6.3) is the manual remedy.

Key material is only ever on the control side, in the local temporary
directory, and is removed when the session ends. The remote session directory
holds the workspace, the stage and transfer archives, never a key.

When a step fails and `--keep-workdir` is set, `stop` must keep the remote
session directory and print its path, because the host workspace is only
updated by `sync_out`, which a failed run never reaches. The existing
`workspace:`/`stage:` lines in the `Sandbox` error message are host paths and
must not be presented as the place to look for a remote failure.

### 5.3 Step command

Per adapter:

```
podman: podman exec -i -w <cwd> [ -e KEY=VALUE ... ] <container> /bin/sh -s   < script
bwrap:  bwrap <local-bwrap-args> -- /bin/sh -s                                < script
chroot: chroot <root> /bin/sh -s                                              < script
host:   sh -s                                                                 < script
```

The environment is passed explicitly for `podman`, `bwrap` and `chroot`
(`-e` / `--setenv` / a generated `env` prefix). Two gaps must be documented
rather than hidden:

- the container's or host's own environment is visible to a `podman`/`host`
  step; there is no `--clearenv` equivalent for `podman exec`. Importing the
  root without an `ENV` directive and starting the container with a minimal
  environment gets close, but is not the bwrap guarantee.
- `podman exec` does not run the container's entrypoint, so a root without
  `/bin/sh` fails the first step. `/bin/sh` is already implied by every shell
  step.

### 5.4 Timeouts and cancellation

The local `timeout` wrapper around `ssh` kills the *client*. It does not kill
the process inside the adapter context, and a build that ignores `TERM` can
outlive the run.

The session therefore:

1. runs the remote command under the remote `timeout --signal=TERM
   --kill-after=5`;
2. keeps the local `timeout` around `ssh` as a backstop;
3. on expiry, kills the context — `podman kill` (SIGKILL to container PID 1),
   or `bwrap --die-with-parent` plus killing the process group, or a chroot
   process-group kill — then recreates it with the same root and bind mounts.

The workspace survives because it lives in the session directory, not in the
context. The failure is `RPMX-SANDBOX-004`, as for the other runners.

### 5.5 Cache and pruning

Delivered roots are keyed by `root_digest`:

```
tag      = localhost/rpmx-root:<digest without the sha512: prefix>
cache dir= <work-dir>/roots/<digest without the prefix>
```

- The session checks the cache before uploading and skips the transfer on a
  hit.
- Ephemeral hosts (per-`TaskRun` users, disposable VMs) lose the cache with the
  host. Nothing to do.
- Long-lived hosts keep it. `rpmx ssh prune` connects, lists
  `localhost/rpmx-root:*` and the `roots/` directory, and removes entries older
  than a threshold or not referenced by the locks given on the command line.
  Without it, a shared build host accumulates one root per distinct closure,
  which is the same growth `rpmx cache gc` already manages on the control side.

## 6. Root delivery

The assembled root is content-addressed. `root_digest` is the cache key, but
the runner interface does not carry it today: `RunnerConfig` has only a `Path`.
Add `root_digest: Option<&str>` to `SandboxConfig` and `RunnerConfig` and let
the caller supply the digest it already trusts. The build pipeline passes
`verify_root: false` (`pipeline.rs:82,127`), so "the lock says X" is not enough
on its own: the digest used for the key must describe the tree actually
uploaded. Either reuse a `RootSelection` (which computed and compared the tree),
or compute `digest_root` once before the first session and cache it. A lock
without a digest is not an error; compute one.

Per adapter:

| Adapter | Delivery | Cache hit test |
|---|---|---|
| `podman` | stream the tar to `podman import`, or extract to a directory for `--rootfs` (SSH-0) | `podman image exists <tag>` |
| `bwrap` | stream the tar into `roots/<digest>` | directory exists and contains a `.digest` marker |
| `chroot` | stream the tar into `roots/<digest>` | same |
| `host` | none; the root is not used | n/a |

Order:

1. Obtain a digest that describes the tree on disk. Never key a cache entry
   with a digest the tree was not verified against.
2. Test the cache; on a hit, go to step 6.
3. Stream `tar -C <root> -cf - .` to the host.
4. Store it (image, or directory with a marker).
5. Verify the entry resolves and that the store has room for it.
6. Start the adapter context.

Costs and non-goals:

- The upload is the assembled tree, not the RPM payloads: a few hundred
  megabytes for a typical toolchain root. On a fresh host it happens once per
  command, because each command constructs its own `Sandbox`
  (`source::execute`, `rebuild::execute`, `test_runner`).
- The runner does **not** mount a control-host directory into the remote, and
  does **not** use NFS, 9p or virtiofs. There is no shared host path.
- The root must contain `/bin/sh` for every adapter except `host`. That is
  already implied by every shell step.

## 7. Root resolution and assembly (the part the runner does not move)

`rpmx build` is source assembly, then SRPM, then rebuild. Root resolution
(`dnf5` inside the profile's builder image) and root assembly (offline from the
CAS with `podman --arch`) happen before any step runs and are **not** part of
the runner.

Consequences the operator must know:

- `dnf download --resolve` is architecture-agnostic, so resolving a foreign
  lock on an x86_64 control host is fine.
- `root assemble` executes target-architecture scriptlets. On an x86_64
  control host, assembling an aarch64 root still needs `qemu-user` binfmt, as
  it does today (`crates/rpmx-root/src/root/assemble.rs`).
- Therefore **Phase 1 does not remove the local binfmt requirement for a
  foreign build.** It removes the local *step execution* requirement, and it
  replaces a slow TCG build with a native one.
- Phase 2 (SSH-12) moves resolution and assembly to the remote, where the
  architecture is native and `podman --arch` needs no emulator. That is the
  change that makes an x86_64 control plane plus a remote aarch64 host
  self-sufficient.
- `--ssh-execution=host` never uses the assembled root, so assembly is wasted
  work in that mode; the runner says so and `rpmx build --ssh-execution=host`
  may accept a recipe whose root was never assembled.

The installed-test root is a second root with its own digest and is delivered
the same way.

## 8. Interfaces

### 8.1 Rust

`RunnerKind` gains `Ssh`. `RunnerKind::choose` needs the SSH target, so
`SandboxConfig` gains fields — the same pattern, and nearly the same call
sites, as `appliance` in [`16-qemu-appliance.md`](16-qemu-appliance.md) §6.4.
The two changes should be coordinated so the literals are touched once.

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SshExecution {
    Auto,
    Podman,
    Bwrap,
    Chroot,
    Host,
}

pub struct SshTarget {
    pub host: String,
    pub port: u16,
    pub user: Option<String>,     // default: ssh's own choice ($USER)
    pub config: Option<PathBuf>,  // -F
    pub key: Option<PathBuf>,
    pub known_hosts: Option<PathBuf>,
    pub host_key: Option<String>, // public key or known_hosts line
    pub accept_new_host: bool,    // explicit opt-in
    pub options: Vec<(String, String)>, // -o KEY=VALUE
    pub work_dir: Option<String>,       // default: $HOME/.rpmx
    pub execution: SshExecution,
    pub connect_timeout: NonZeroU64,
}

pub struct RemoteInfo {
    pub arch: Arch,
    pub cpus: NonZeroU32,
    pub adapter: SshExecution,     // never Auto once probed
    pub selinux: bool,
    pub cgroup_v2: bool,
}

pub struct SandboxConfig<'a> {
    // ... existing fields ...
    pub ssh: Option<&'a SshTarget>,
    pub remote: Option<&'a RemoteInfo>, // probe result
    pub root_digest: Option<&'a str>,   // verified tree digest, for the cache key
}
```

`TestSandboxConfig` (`crates/rpmx-runner/src/test_runner.rs`) duplicates these
fields and needs the same additions, or it should embed `SandboxConfig` instead
of copying it.

`Sandbox::new` rejects `ssh` without `root` for every adapter except `host`, and
rejects `ssh` with `image`. There is no separate `unsafe_host_root` check: that
flag is expressed as `root: None`, so the first rule already covers it.

The architecture and CPU count are probed once, before the `SandboxConfig` is
constructed, and stored in `RemoteInfo`. This exists because `JOBS` and
`RPMX_HOST_ARCH` are built by `Sandbox` before the session starts, and a
default `--jobs` from the control host's CPU count would oversubscribe or
undersubscribe the host. `--jobs` and `--ssh-arch` remain explicit overrides.

### 8.2 CLI

| Flag | Meaning |
|---|---|
| `--runner ssh` | Select the SSH backend. Requires a target from `--ssh` or `RPMX_SSH_TARGET`. |
| `--ssh <[user@]host[:port]>` | Endpoint. User and port default to ssh's own rules. IPv6 literals use `[addr]:port`. |
| `--ssh-config <path>` | Use this SSH config instead of the operator's default. |
| `--ssh-option <KEY=VALUE>` | Extra `-o` option, repeatable. The runner's own safety options win. |
| `--ssh-key <path>` | Identity file. Falls back to `RPMX_SSH_KEY`, then the agent or config. |
| `--ssh-known-hosts <path>` | Trust anchor. Default: the operator's `~/.ssh/known_hosts`. |
| `--ssh-host-key <key-or-line>` | Pin a host key as a public key or a `known_hosts` line. |
| `--ssh-accept-new-host` | Trust an unknown host key on first use for this run. Off by default. |
| `--ssh-work-dir <path>` | Remote base directory. Default `$HOME/.rpmx`. |
| `--ssh-execution <adapter>` | `auto`, `podman`, `bwrap`, `chroot` or `host`. Default `auto`. |
| `--ssh-arch <arch>` | Trust this architecture instead of probing. Still cross-checked against the adapter, because a wrong override would silently run the wrong binaries. |
| `--ssh-connect-timeout <secs>` | Total time allowed to establish a working connection, including retries. Each attempt uses a short TCP timeout. |

Environment fallbacks: `RPMX_SSH_TARGET`, `RPMX_SSH_CONFIG`, `RPMX_SSH_KEY`,
`RPMX_SSH_KNOWN_HOSTS`, `RPMX_SSH_HOST_KEY`, `RPMX_SSH_WORK_DIR`,
`RPMX_SSH_EXECUTION`, `RPMX_SSH_ARCH`.

The flags are defined once in an `SshArgs` struct and flattened into the five
argument structs that currently repeat `--runner` (`build`, `rebuild`, `test`,
`source`, `srpm`). The five duplicated
`value_parser = ["auto", "bwrap", "qemu"]` literals in
`crates/rpmx-cli/src/commands/args.rs` become one shared parser.

Validation rules:

- `--runner ssh` without a target is a usage error, not a silent fallback.
- Any `--ssh*` flag without `--runner ssh` is a usage error, so a typo cannot
  be silently ignored.
- `--runner ssh` needs an assembled root for every adapter except `host`, like
  `--runner qemu` (`RPMX-ROOT-016`).
- `--ssh-execution=host` without `--runner ssh` is a usage error.
- `--runner auto` never yields SSH.
- `--offline` governs repository and source access only; it does not forbid the
  SSH connection, which is the build environment rather than an input.
- `rpmx shell` stays local in Phase 1; the command is hard-coded to bubblewrap
  today. `rpmx ssh shell` is a possible follow-up, not part of this plan.

### 8.3 Error codes

| Code | Meaning |
|---|---|
| `RPMX-RUNNER-002` | The remote architecture cannot execute the target (unchanged meaning). |
| `RPMX-RUNNER-003` | A remote runner backend failed. The doc comment currently says "qemu"; generalise it to qemu and SSH. |
| `RPMX-RUNNER-005` | The SSH endpoint is unreachable, rejects the key or host key, lacks a required host tool (I4), or has no adapter whose precondition holds. |
| `RPMX-SANDBOX-001` | The adapter context cannot be created or started. |

Container, namespace and step failures keep `RPMX-SANDBOX-001/002/004`,
matching the local backends.

## 9. Security

### 9.1 Credentials

- The private key is read on the control side only. It is never uploaded, never
  mounted and never written to a log. `ssh` reads it by path.
- Agent forwarding is disabled. The agent is used locally by the `ssh` client,
  which needs no forwarding.
- `BatchMode=yes` and no PTY. Password authentication is therefore unsupported;
  key or agent only.
- The local temporary directory holds the `known_hosts` file and the transfer
  archives and is removed on stop and on drop. The remote session directory
  holds the workspace, the stage and transfer archives, never a key.

### 9.2 Host keys

The default is the operator's existing trust, not a fresh one:

- `StrictHostKeyChecking=yes` against `~/.ssh/known_hosts`, or the file given
  with `--ssh-known-hosts`.
- A pinned public key or `known_hosts` line via `--ssh-host-key`.
- `--ssh-accept-new-host` opts into `StrictHostKeyChecking=accept-new` against
  a per-session `known_hosts`, with a warning on stderr. Every later connection
  in the session is verified against it, so only the first exchange is exposed.
  Use it for a newly provisioned host whose key the operator has not recorded.
- If both a pin and `--ssh-accept-new-host` are given, the pin wins.
- `StrictHostKeyChecking=no` is never generated, in any mode, by any flag.

A fingerprint alone cannot be turned into a `known_hosts` entry, so
`--ssh-host-key` takes the key or the line, not a hash.

### 9.3 What the recipe cannot do

A recipe cannot name a host, a user, a key, a known_hosts file, an adapter or a
work directory. If it could, a malicious recipe could redirect an operator's
credentials to its own machine, or downgrade the adapter to `host`. This is the
same rule that already stops a recipe-selected profile from carrying repository
`auth`.

### 9.4 Remote resource limits

The runner enforces what it owns: the step timeout, the adapter's process and
tmpfs limits, and `--jobs`. It does not claim to enforce host-level quotas it
does not own; on a host with per-user cgroup limits, those apply on top.
`rpmx doctor --ssh …` reports what it can observe and nothing more.

### 9.5 Trust boundary delta

| Adapter | Network | Capabilities | PID | Read-only `/` | Boundary |
|---|---|---|---|---|---|
| `podman` | `--network=none` | dropped | namespace | yes | container |
| `bwrap` | namespace | dropped, `no_new_privs` | namespace | yes | namespace |
| `chroot` | host network | host root | host | no | the host (chroot is not a boundary) |
| `host` | host network | host user | host | no | the host user |

- A step cannot see the remote session directory, the root cache or the image
  store: the adapter context has no mount pointing at them, except `host`.
- Step output, including the synced tar stream, is still attacker-controlled.
  The existing `tar_guard` validation and the 8 GiB cap stay on the control
  side, exactly as for QEMU.
- `--ssh-execution=host` is not a sandbox and is not a locked build. It is
  recorded as such.

## 10. Failure modes

| Symptom | Report |
|---|---|
| DNS, TCP, or auth failure | `RPMX-RUNNER-005`, with the sanitised `ssh` stderr tail |
| Host key mismatch or unknown | `RPMX-RUNNER-005`, never a retry with `no` |
| A required tool or adapter is missing | `RPMX-RUNNER-005`, naming every missing item and what each adapter needed |
| `podman info` fails (no subuid/subgid range) | `RPMX-RUNNER-005`, saying so instead of forwarding a podman error |
| `uname -m` ≠ `--arch` | `RPMX-RUNNER-002`, before any step |
| Root delivery fails | `RPMX-RUNNER-005`, including the remote error line |
| Adapter context cannot be created | `RPMX-SANDBOX-001`, with the adapter's stderr tail |
| Context exits during a step | `RPMX-SANDBOX-002`, with the step label |
| Step exceeds the timeout | `RPMX-SANDBOX-004`, context killed and recreated |
| Connection drops mid-step | `RPMX-RUNNER-003`; `stop` still attempts cleanup |
| `sync_out` finds a hostile archive | `RPMX-RUNNER-003` from `tar_guard` |
| Session directory or root cache full | `RPMX-RUNNER-005`, naming which one |

## 11. Test plan

### 11.1 Unit tests (`rpmx-runner`)

- `ssh_target_parses_user_host_and_port`; rejects an empty host and port 0;
  accepts `[ipv6]:port` and a user containing `-` or `.`.
- `ssh_options_never_override_the_safety_set` — a user `-o` cannot enable agent
  forwarding or disable host-key checking.
- `known_hosts_is_never_strict_no` — asserts the generated argv never contains
  `StrictHostKeyChecking=no`.
- `adapter_auto_prefers_podman_then_bwrap_then_chroot`.
- `an_explicit_adapter_never_falls_back`.
- `host_adapter_is_never_selected_by_auto`.
- `root_digest_maps_to_a_stable_cache_key`.
- `an_unverified_tree_is_never_keyed_with_the_lock_digest`.
- `a_remote_arch_mismatch_fails_before_the_first_step`.
- `session_directory_is_unique_per_session`.
- `stop_removes_the_context_and_the_session_directory` (fake `ssh` on PATH that
  records argv).
- `missing_remote_capabilities_are_reported_together`.
- `the_tool_probe_reads_its_marker_through_the_session` — the probe must not
  assume the host workspace reflects the remote one.
- `keep_workdir_retains_the_remote_session_directory`.
- `prune_leaves_a_referenced_root_in_place`.

The existing `qemu/ssh.rs` transport tests move with the code and keep
running.

### 11.2 Integration (`scripts/ssh-runner-test.sh`)

Gated on `RPMX_SSH_TEST_TARGET`; skipped with a clear message when unset. The
script may point at several targets, one per host shape.

1. Probe and assert arch, tools and the selected adapter.
2. Run a step that writes to `/tmp/build` and reads from `/`.
3. Assert the adapter's advertised isolation: for `podman`/`bwrap`, only
   loopback is visible; for `chroot`/`host`, record that the network is the
   host's.
4. Assert a step cannot read the session directory or the root cache.
5. Build a fixture package end to end and compare `root_digest` with the same
   closure built locally.
6. Force a step timeout and assert the context is gone afterwards.
7. Kill the SSH connection mid-step and assert `stop` cleans up.
8. Run twice with the same root and assert the cache hit skips the upload.
9. Run `rpmx ssh prune` and assert referenced roots survive and unreferenced
   ones do not.

### 11.3 Acceptance across host shapes

At least three of the shapes in §2, with the adapter each actually supports:

- a rootless-podman host (for example a `multi-platform-controller` VM) —
  `podman`;
- a root-only host with no podman — `chroot`;
- an unprivileged host with `bubblewrap` — `bwrap`;
- a host with none of them — `--ssh-execution=host`, with the warning and the
  unlocked marker.

Produce the same RPM names and `root_digest` as a local build of the same
closure for every adapter that uses the root.

### 11.4 What would prove the change

| Claim | Proof |
|---|---|
| Runner independence | The same closure assembled locally and delivered over SSH has one `root_digest` for every root-using adapter. |
| No host-package leakage | A recipe that uses `tar` without declaring it fails on the remote, as it does locally. This depends on the tool probe reading its marker through the session (SSH-5); today `check_build_tools` reads the host workspace and would silently pass. |
| Adapter isolation | The §9.5 row for the selected adapter is observed from a step, not inferred. |
| Isolation from the host | A step cannot read the session directory, the cache or the SSH key (except `host`). |
| Cleanup | No context or session directory remains after success, failure and timeout. |
| Cache correctness | A changed root never hits the key of an unchanged one; a hit never re-uploads. |
| Native foreign build | An aarch64 host builds aarch64 without local TCG for step execution. |
| No credential exposure | No argv, log line or remote file contains the key; agent forwarding is off. |

## 12. Implementation plan

Each ticket is independently shippable. SSH-0 gates SSH-3, SSH-4 and SSH-12;
the rest may proceed in parallel where their "Depends on" allows.

### SSH-0 — adapter and root-delivery spike (blocks SSH-4, SSH-12)

Goal: choose the delivery mechanism and confirm the adapter preconditions on
real hosts.

- Three host shapes: rootless podman, rootful `chroot`, unprivileged `bwrap`.
- For `podman`, prove `podman info` (including the subuid/subgid range) and
  compare `podman import` with `--userns=keep-id` against an extracted
  `--rootfs` directory: ownership, `--read-only`, bind-mount writability,
  `--network=none`, `--pids-limit`, `:Z`, and `podman exec` environment
  behaviour.
- For `chroot` and `bwrap`, prove the same step can read `/` and write
  `/tmp/build`.
- Measure the cache cost of the root per adapter (especially `vfs`) and record
  the space requirement.
- Record the chosen approach and the per-adapter deltas in §3 and §6.

Exit criteria: a step writes to `/tmp/build`, reads a file from `/`, and cannot
write outside the bind mounts, on all three shapes. The result is written into
this document before SSH-4 starts.

Files: `scripts/ssh-runner-spike.sh`, a short section appended to this
document.

### SSH-1 — extract the SSH transport from the QEMU backend

Goal: one transport, two users.

- Move `crates/rpmx-runner/src/runner/backend/qemu/ssh.rs` to
  `.../backend/transport.rs`.
- Keep the signatures; QEMU imports the shared module.
- Remove the word "qemu" from user-facing transport errors.

Exit criteria: `cargo test -p rpmx-runner` passes; a grep shows one
implementation of `stream_tar_in`, `stream_tar_out` and `step_script`.
Depends on: nothing.

### SSH-2 — target parsing, options, probe and argv

Goal: a connection spec that can be validated without running a build.

- `SshTarget` type, `FromStr` for `[user@]host[:port]`, env fallbacks.
- `SshTarget::argv()` producing the full `ssh` argument vector, honouring
  `--ssh-config` and `--ssh-option` while refusing to override the safety set.
- `probe()` returning `RemoteInfo` (arch, cpus, adapter, selinux, cgroup).

Exit criteria: unit tests from §11.1 that do not need a network; the argv for a
valid target is exactly what the integration test later replays. The CLI usage
error is SSH-7's, not this ticket's. Depends on: SSH-1.

### SSH-3 — session skeleton and adapter selection

Goal: connect, choose an adapter, create and remove the session directory, copy
in and out.

- `SshBackend`/`SshSession` with `start`, `run` (unimplemented), `sync_out`,
  `stop`, `Drop`.
- `SshExecution` selection with the rules in §3.6 and the probe from SSH-2.
- Unique session directory; `mkdir -p`; copy in workspace and stage with the
  streaming transport; sync back with the guard and the cap.
- `stop` is idempotent and best-effort.

Exit criteria: against a test endpoint, a file placed in the host workspace is
readable on the remote and a file written on the remote appears on the host
after `sync_out`; the session directory is gone afterwards; `auto` chooses the
expected adapter on each of the three spike hosts. Depends on: SSH-0, SSH-2.

### SSH-4 — root delivery and cache

Goal: a cached root keyed by `root_digest`, per adapter.

- Implement the SSH-0 outcome for `podman`, `bwrap` and `chroot`.
- Check-before-transfer; one transfer per digest per host.
- Reuse the verified digest; do not add a second digest path.

Exit criteria: two consecutive sessions with the same root transfer once; a
changed root produces a new key; a corrupted local root is rejected before
upload. Depends on: SSH-0, SSH-3.

### SSH-5 — step execution and the tool probe

Goal: steps run in the adapter context.

- Start the context with the §3.2/§3.3/§3.4 parameters.
- Execute one step per invocation with the explicit environment and cwd.
- A root without `/bin/sh` fails the first step with a clear message.
- Read the build-tool probe's marker through the session. `check_build_tools`
  in `crates/rpmx-runner/src/build_env.rs` reads a host path after the step;
  with a copy-based runner that file only exists on the remote, so the probe
  silently reports nothing missing. Sync before the read, or add a
  `Session::read_file`.

Exit criteria: the fixture step from SSH-0 runs through `Sandbox::run`; the
adapter's isolation assertions pass; a declared tool that is absent from the
root fails the build before the first pipeline step; the context is removed on
stop and on drop. Depends on: SSH-4.

### SSH-6 — timeouts, cancellation and `--keep-workdir`

Goal: a step cannot outlive its timeout, and a failure is inspectable.

- Remote `timeout`, local `timeout`, context kill and recreate on expiry.
- Map the failure to `RPMX-SANDBOX-004` with the step label.
- `--keep-workdir`: on a failed step, retain the remote session directory and
  print its path instead of deleting it in `stop`/`Drop`.
- Document that `SIGINT`/`SIGTERM` bypass `Drop` and leave the context for the
  operator's cleanup.

Exit criteria: a step that ignores `TERM` is gone after the timeout; the next
step still runs in a healthy context; no context remains; a failed step with
`--keep-workdir` leaves an inspectable remote directory. Depends on: SSH-5.

### SSH-7 — CLI surface, selection and validation

Goal: users can select the backend.

- `SshArgs` flattened into the five argument structs; one runner value parser.
- `RunnerKind::Ssh`, selection and the validation rules in §8.2.
- `SandboxConfig.ssh`, `.remote` and `.root_digest`; the rejection rules;
  coordinate the fields with the `appliance` change from document 16.
- `--ssh*` without `--runner ssh` is a usage error.

Exit criteria: `rpmx build --runner ssh --ssh user@host` reaches a real
session; `--runner auto` never selects SSH; the five help texts list `ssh`.
Depends on: SSH-5.

### SSH-8 — architecture and job count

Goal: correct `JOBS`, `RPMX_HOST_ARCH` and early failure.

- Use `RemoteInfo` for `JOBS` and the remote host arch exposed to steps.
- `--ssh-arch` override; mismatch fails with `RPMX-RUNNER-002` before step one.
- `--jobs` still wins.

Exit criteria: a non-default `--jobs` is respected; an arch mismatch produces
the documented error and no step header. Depends on: SSH-7.

### SSH-9 — installed tests, rebuild and the `host` adapter

Goal: every sandbox path works, not just `build`.

- Installed tests deliver the test root as a second cached root and run in a
  second session.
- `rpmx rebuild`, `rpmx source` and `rpmx test` accept the flags and behave.
- `--ssh-execution=host`: no root delivery, a warning, and an explicit
  unlocked marker in the build output.
- Document `rpmx shell` as local-only.

Exit criteria: a package with an installed-stage test passes end to end on a
`podman` host; `rpmx rebuild` from an SRPM works with `--runner ssh` and a
cache-only root; a `host`-mode build warns and does not claim a locked root.
Depends on: SSH-7.

### SSH-10 — cache pruning and doctor

Goal: long-lived hosts stay bounded and a failing setup is diagnosable.

- `rpmx ssh prune --host … [--older-than …] [--lock …]` for `podman` images and
  directory caches.
- `rpmx doctor --ssh user@host` reports reachability, arch, adapter
  preconditions, tools and space.
- README "Runners and architectures", `docs/usage/reference.md`,
  `docs/usage/troubleshooting.md`, `docs/contributing/09-sandboxing.md`,
  CHANGELOG.

Exit criteria: `tests/docs.rs` passes (every documented flag exists); a prune
leaves referenced roots; the doctor output names the missing capability rather
than "runner failed". Depends on: SSH-7.

### SSH-11 — integration script and acceptance matrix

Goal: the proof in §11 is runnable.

- `scripts/ssh-runner-test.sh` gated on `RPMX_SSH_TEST_TARGET`.
- A matrix runner that takes several targets and records each one's adapter.
- Example recipes/config in `examples/` or `scripts/`, documented in
  `CONTRIBUTING.md`.

Exit criteria: the script passes against three host shapes and skips cleanly
without a target. Depends on: SSH-9, SSH-10.

### SSH-12 — remote root resolution and assembly (Phase 2, optional)

Goal: an x86_64 control host can build a foreign target without local binfmt.

- Move `podman_run`/`podman_run_assembly` behind the remote transport when
  `--runner ssh` is selected, or add `--ssh-root-plumbing auto`, `local` or
  `remote`.
- The CAS payloads must reach the remote; decide between streaming the payload
  set and resolving remotely.
- Extend `root resolve` (the path that produces a lock) so it can run remotely
  too, without making `--offline` reach the network.

Exit criteria: resolving and assembling an aarch64 root on an x86_64 host
without binfmt, then building it, all with `--runner ssh`. Depends on: SSH-9.

Do not start SSH-12 before SSH-0 proves root delivery; if delivery turns out to
be cheap and deterministic, SSH-12 becomes much smaller.

## 13. Open questions

1. **Root delivery per adapter.** `podman import` and `--rootfs` differ in
   ownership and in whether `/` can be read-only; `chroot` and `bwrap` use a
   plain directory. SSH-0 decides and the answer belongs in §3 and §6.
2. **`--jobs` probing.** Is `nproc` the right number, or should it be bounded
   by the cgroup quota when one is visible? Prefer the quota.
3. **Cache lifetime defaults.** `rpmx ssh prune` needs a default threshold. Two
   weeks? Or keep anything referenced by a lock on the control host and drop the
   rest? The second is more predictable but needs the lock list.
4. **Remote plumbing scope.** Phase 1 keeps resolution and assembly local.
   Users who only have the remote host will need SSH-12; that should be stated
   in the usage docs, not left implicit.
5. **Adapter downgrade policy.** Should `auto` ever choose `chroot` when podman
   is installed but misconfigured? The current answer is no: a named or detected
   adapter that fails its precondition is an error, and `host` is never
   automatic.
6. **`host` mode provenance.** Where exactly does the "not locked" marker live:
   a stderr warning only, or a field in the build-level SPDX and the JSON
   output? Pick one and specify it.
7. **SELinux and cgroups.** `:Z` and `--pids-limit` depend on host state. The
   probe must detect both and the failure must name the capability, not a
   podman exit code.
8. **Multiple architectures per host.** A host can be multiarch (binfmt on the
   remote) even when `uname -m` says one thing. Phase 1 ignores that and
   requires `--arch` to equal the probe; supporting remote binfmt is a
   follow-up.

## 14. Non-goals

- Windows or macOS build hosts. The contract is a POSIX host that can execute
  target binaries.
- Cross-compilation. A foreign target still executes target binaries, natively
  on the remote.
- Password authentication, or a PTY.
- A shared control-host filesystem on the remote (NFS, 9p, virtiofs).
- Making `chroot` or `host` a security boundary.
- Multi-tenant scheduling: the operator or the CI decides which host, `rpmx`
  uses it.
