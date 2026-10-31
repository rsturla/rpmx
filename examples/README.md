# Examples

Design artifacts, not tested builds.

| Example | Demonstrates |
|---|---|
| [`curl/`](curl/) | A conventional C/CMake package with features, subpackages, typed actions, tests, OCI/HTTPS locators, and a side-by-side spec. |
| [`hello-go/`](hello-go/) | Go module acquisition through a proxy, `go mod vendor`, no hand-made tarball, offline SRPM rebuild. |
| [`go-bump/`](go-bump/) | Targeted Go module bumps for CVE remediation, using omnibump in source assembly. |
| [`libfoo/`](libfoo/) | Deterministic generated source in the source-assembly stage, included in the SRPM; generator/version recorded in the manifest. |
| [`python-matrix/`](python-matrix/) | A matrix over Python 3.10–3.12: typed axis, per-cell build dependencies and outputs, one SRPM per cell, templated names. |
| [`c-hello/`](c-hello/) | **Tested.** A generated C source built with the `make` step (gcc, `make check`, `make install`) and a staged symlink. |
| [`zstd/`](zstd/) | **Tested.** A real upstream package: three binary RPMs, shared-library and CLI symlinks, and a byte-identical rebuild. |
| [`libgreet/`](libgreet/) | **Tested.** A shared library and devel subpackage with pkgconfig metadata and an installed test that compiles a consumer. |
| [`zlib/`](zlib/) | **Tested.** Autotools library + devel split; installed test compiles against the packaged headers. |
| [`xz/`](xz/) | **Tested.** Autotools; `xz-libs`, `xz-devel` and `xz`; versioned `liblzma` provides and CLI symlinks. |
| [`lua/`](lua/) | **Tested.** Makefile project; `lua` and `lua-devel`; installed interpreter test. |
| [`jq/`](jq/) | **Tested.** Autotools with a system `oniguruma` dependency; `libjq`, `libjq-devel` and `jq`; installed JSON test. |
| [`pyhello/`](pyhello/) | **Tested.** Pure-Python wheel, noarch output, separately resolved test dependency. |
| [`hello-daemon/`](hello-daemon/) | **Tested.** A systemd service with unit, sysusers, tmpfiles, scriptlets and `%config(noreplace)`. |

The design examples contain `package.yaml` and a `plan.md`. `c-hello`, `zstd`, `libgreet`, `zlib`, `xz`, `lua`, `jq`, `pyhello` and `hello-daemon`
are built end to end from a locked root; their hashes are real.
