# Distributing `rpmx` as a container

## 1. Goal

Make `rpmx` available as a signed, multi-arch OCI image without requiring `--privileged` and without turning the host Docker socket into a root-equivalent dependency.

The container can be the sandbox. It should not be a wrapper that runs bubblewrap inside a restricted container and hopes the kernel cooperates.

## 2. Two execution modes

### Host mode

`rpmx` runs as a host binary and creates a bubblewrap sandbox per phase.

```
host: rpmx -> bwrap -> build
```

This is the default for local development and single-tenant build hosts.

### Container mode

`rpmx` runs inside a container, and the container runtime provides the isolation boundary.

```
host: podman/docker -> rpmx container -> build
```

In this mode `rpmx` must **not** try to create a nested bubblewrap sandbox by default. It runs in `--sandbox=container` mode and treats the outer container as the sandbox.

The container must be configured correctly:

```
--network=none
--read-only
--cap-drop=all
--security-opt no-new-privileges
--pids-limit=...
--memory=...
--cpus=...
--tmpfs /tmp:...
```

If those guarantees are absent, `rpmx` warns or fails, depending on the profile.

## 3. Two kinds of images

Do not confuse the tool image with the build root.

| Image | Contents | Purpose |
|---|---|---|
| **`rpmx` tool image** | static `rpmx`, CA certs, seccomp profiles, licenses | Orchestration, fetch, SRPM verification, package tests |
| **`rpmx-builder:<profile>`** | `rpmx` + the profile's build root and toolchain | Source assembly, prep/build/install/check, installed tests |

A generic `rpmx` image does not contain GCC, CMake, Go, Rust, Java and every distro's build dependencies. Those belong in a builder image generated from a profile and recorded by digest.

A builder image is built from the resolved build root, signed, and pinned in the lock:

```json
{
  "build": {
    "builder_image": "registry.example.org/rpmx/builder-fedora-44@sha256:...",
    "root_digest": "sha256:..."
  }
}
```

## 4. Phase workflow with containers

Each phase can be its own container invocation. This is easier to secure than one long-lived container that switches network modes.

### Fetch

```
podman run --rm \
  -v "$PWD:/work:Z" \
  -v rpmx-cache:/cache \
  -v /run/rpmx/proxy.sock:/run/rpmx/proxy.sock \
  registry.example.org/rpmx/rpmx@sha256:... \
  fetch --locked
```

- Network may be enabled, or better, restricted to a proxy socket.
- Credentials come from the fetch environment, not the recipe.
- No build tools are needed.

### Source assembly and build

```
podman run --rm \
  --network=none \
  --read-only \
  --cap-drop=all \
  --security-opt no-new-privileges \
  --pids-limit=1024 \
  --memory=8g \
  --cpus=4 \
  --tmpfs /tmp:rw,size=2g \
  -v "$PWD:/work:Z" \
  -v rpmx-cache:/cache \
  registry.example.org/rpmx/builder-fedora-44@sha256:... \
  build --offline --locked --from-srpm /work/curl-8.11.1-1.src.rpm
```

- Network is disabled.
- The root filesystem is read-only; `/work`, `/cache` and `/tmp` are the writable areas.
- Capabilities are dropped.
- The builder image supplies the toolchain.

### Installed tests

Use a fresh container or a fresh disposable root per test, with the produced RPMs mounted read-only and installed into the test root.

- No network by default.
- Scriptlets execute inside the test container, not on the host.
- Test dependencies come from the lock and the builder image, not from the network.

### Package tests and signing

- Package tests run in the tool or builder image with no network.
- `rpmx` does not sign and does not hold keys. It emits unsigned RPMs, SRPMs and evidence.
- Signing is a separate job or service, outside `rpmx`. Keys never enter the build container, and no signing socket is mounted into a build.
- A signing service receives artifact digests and returns detached signatures over a narrow protocol; that interface is designed but not part of V1.

## 5. `/dev/kvm`

Mount `/dev/kvm` only when `rpmx` is using the QEMU/microVM sandbox:

```
podman run --rm \
  --device /dev/kvm \
  --network=none \
  ...
  registry.example.org/rpmx/builder-fedora-44@sha256:... \
  build --sandbox=qemu --offline
```

Use it for:

- high-assurance builds;
- untrusted multi-tenant CI;
- kernel-module or bootloader packages;
- foreign-architecture builds where emulation is required.

Do not mount `/dev/kvm` by default. It is a powerful kernel interface; the container can create VMs, and the KVM attack surface is not trivial. Treat it as a privileged capability and isolate the runner.

## 6. Sockets

Sockets are capabilities. Mount only the one a phase needs.

| Socket | Allowed? | Where | Why |
|---|---|---|---|
| **Proxy socket** | Yes | Fetch only | Restricts egress to the acquisition proxy without giving a host network interface. |
| **Signing socket** | External only | A separate signing job, never a build | `rpmx` never mounts or uses it; keys stay out of the build and out of the tool image. |
| **Log/journal socket** | Yes | Any phase | Log collection; not a privilege boundary. |
| **Docker socket** | **No** | Never | Root-equivalent host access. |
| **Podman socket** | Only with a dedicated rootless user | Nested runtime only | Still powerful; do not expose the host user's socket. |
| **SSH agent** | **No** | Never | Build must not push to remotes. |
| **D-Bus system bus** | **No** | Never | Host service access. |
| **Keyring / secret service** | **No** | Never | No credentials in the build. |

If `rpmx` needs to launch nested phase containers, use a dedicated rootless Podman service under a separate UID, not the host Docker socket. Prefer host-side orchestration of phase containers instead.

## 7. Container security contract

The `rpmx` and builder images must be usable with:

- rootless runtime;
- read-only root filesystem;
- all capabilities dropped;
- `no-new-privileges`;
- no host network by default;
- no host PID/IPC/UTS;
- no host home, SSH, D-Bus or Docker socket;
- cgroup v2 limits;
- seccomp profile;
- size-limited `/tmp`.

The images must not:

- contain secrets, signing keys or registry credentials;
- require `--privileged` for normal operation;
- assume a writable `/`;
- assume a host toolchain;
- mount the host root.

## 8. Image publication

Publish:

- `registry.example.org/rpmx/rpmx:<version>` — signed, multi-arch, SBOM, provenance, digest-pinned;
- `registry.example.org/rpmx/builder-<profile>:<version>` — signed, SBOM, build-root digest;
- an RPM/static binary for host mode.

Tags are discovery aids; the lock pins digests.

The image should include a `rpmx version --json` output with:

- version;
- source commit;
- build timestamp;
- image digest;
- sandbox backends available;
- seccomp profile version.

## 9. `--sandbox=container`

`rpmx` should detect container mode and support:

```
--sandbox=container   # outer runtime is the boundary; no nested bwrap
--sandbox=bwrap       # host mode
--sandbox=qemu        # nested VM, needs /dev/kvm
--sandbox=none        # explicit unsafe, recorded
```

In `--sandbox=container`, `rpmx` should verify what it can:

- it is PID 1 or in a dedicated PID namespace;
- network is absent or restricted to the expected proxy;
- root is read-only (or the writable mounts are the expected ones);
- capabilities are empty;
- no host `/proc`, `/sys`, `/dev` beyond the expected set.

It cannot prove all of this; the runtime configuration and provenance record are the real contract.

## 10. Bootstrap

The container image is also the easiest bootstrap artifact:

- a signed static `rpmx` binary, or
- a signed `rpmx` image digest, or
- an older `rpmx` building the new one.

Document the bootstrap chain. A native SRPM cannot be rebuilt without `rpmx`; the bootstrap artifact is therefore part of the trust root.

## 11. Open questions

1. Do we publish one builder image per distro/profile, or generate them on demand from a rootfs digest?
2. Do we require rootless Podman, or support Docker with the same flags?
3. How do we verify `--sandbox=container` actually has the promised isolation?
4. Do we ever allow a container runtime socket for nested builds, and under what dedicated UID?
5. Is `/dev/kvm` available in the target CI, and do we need a fallback to QEMU TCG?
6. How do we make the builder image reproducible and signed without making it enormous?
