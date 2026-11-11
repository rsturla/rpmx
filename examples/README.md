# Examples

This directory contains runnable schema 2 recipes and historical design sketches.
Runnable recipes were planner-validated after the execution lifecycle change;
previous build results do not establish current acceptance coverage.

`build-preferences/`, `curl/` and `libfoo/` are historical design proposals,
not valid current recipes. They use placeholder sources and unsupported features.
See [current recipes](../docs/usage/recipes.md) and
[testing](../docs/usage/testing.md) for the supported interface.

Most buildable examples name `../fedora-44-build.profile.yaml` in
`build.root`, so they resolve their own build root without extra flags. The
cross example names `../fedora-44-cross.profile.yaml` instead:

```console
rpmx build examples/jq/package.yaml
```

| Example | Demonstrates |
|---|---|
| [`build-preferences/`](build-preferences/) | **Design review set.** Three sketches of how distros change preferences: distro-only, package override, and fork. Not buildable. |
| [`curl/`](curl/) | **Historical design sketch; not runnable.** A conventional C/CMake package with features, subpackages, typed actions, tests, OCI/HTTPS locators, and a side-by-side spec. |
| [`eternal-release/`](eternal-release/) | **Built.** A minimal fedora-release replacement with a `context:` directory: `/etc/os-release`, legacy release files, and `.repo` definitions. |
| [`hello-go/`](hello-go/) | Go module acquisition through a proxy, `go mod vendor`, no hand-made tarball, offline SRPM rebuild. |
| [`go-bump/`](go-bump/) | Targeted Go module bumps for CVE remediation, using omnibump in source assembly. |
| [`libfoo/`](libfoo/) | **Historical design sketch; not runnable.** Deterministic generated source in the source-assembly stage, included in the SRPM; generator/version recorded in the manifest. |
| [`python-matrix/`](python-matrix/) | A matrix over Python 3.10–3.12: typed axis, per-cell build dependencies and outputs, one SRPM per cell, templated names. |
| [`c-hello/`](c-hello/) | **Tested.** A generated C source built with the `make` step (gcc, `make check`, `make install`) and a staged symlink. |
| [`c-hello-cross/`](c-hello-cross/) | **Tested.** Cross-compiles aarch64 on x86_64 with a native toolchain and a target sysroot; links zlib, and the checks reject a wrong-arch ELF or an unresolved soname. |
| [`zstd/`](zstd/) | **Tested.** A real upstream package: three binary RPMs, shared-library and CLI symlinks, and a byte-identical rebuild. |
| [`libgreet/`](libgreet/) | **Tested.** A shared library and devel subpackage with pkgconfig metadata and an installed test that compiles a consumer. |
| [`zlib/`](zlib/) | **Tested.** Autotools library + devel split; installed test compiles against the packaged headers. |
| [`xz/`](xz/) | **Tested.** Autotools; `xz-libs`, `xz-devel` and `xz`; versioned `liblzma` provides and CLI symlinks. |
| [`lua/`](lua/) | **Tested.** Makefile project; `lua` and `lua-devel`; installed interpreter test. |
| [`jq/`](jq/) | **Tested.** The split example: `source`, `build`, `outputs` and `tests` live in separate files; Autotools with a system `oniguruma` dependency; `libjq`, `libjq-devel` and `jq`; installed JSON test. |
| [`pyhello/`](pyhello/) | **Tested.** Pure-Python wheel, noarch output, separately resolved test dependency. |
| [`hello-daemon/`](hello-daemon/) | **Tested.** A systemd service with unit, sysusers, tmpfiles, scriptlets and `%config(noreplace)`. |

The design examples contain `package.yaml` plus a `plan.md` or a `README.md` with the expected transcript. `c-hello`, `zstd`, `libgreet`, `zlib`, `xz`, `lua`, `jq`, `pyhello` and `hello-daemon`
are built end to end from a locked root; their hashes are real.
