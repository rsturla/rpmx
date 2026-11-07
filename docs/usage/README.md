# rpmx usage guide

This directory is for people who build packages with rpmx. Design,
architecture and contribution material lives in
[`../contributing/`](../contributing/).

## Read in this order

| Guide | What it covers |
|---|---|
| [`getting-started.md`](getting-started.md) | What rpmx does, requirements, first package, command map. |
| [`recipes.md`](recipes.md) | The complete `package.yaml` reference and a worked example. |
| [`profiles-and-roots.md`](profiles-and-roots.md) | Profiles, inline roots, local and authenticated repositories, root resolution and caching. |
| [`building.md`](building.md) | The build pipeline, artifacts, signing, the package lock, `bump`, repositories and SRPM plumbing. |
| [`cross-compilation.md`](cross-compilation.md) | Building for another architecture with a native toolchain and a target sysroot. |
| [`bootstrap.md`](bootstrap.md) | Ordered bootstrap ladders: manifest, stage graph, seed policy, fixed point, deterministic stage repositories and replacement-lock rebuilds. |
| [`testing.md`](testing.md) | Build- and installed-stage tests, selection, JUnit, and `--install-dir`. |
| [`debugging.md`](debugging.md) | `plan`, `explain`, `steps`, `doctor`, `shell`, `--keep-workdir` and `--debug`. |
| [`troubleshooting.md`](troubleshooting.md) | Common failures and what to do about them. |
| [`reference.md`](reference.md) | Lint rules, exit codes, JSON output, environment variables, limitations. |

## Quick start

```console
rpmx init hello
cd hello

rpmx lint
rpmx build
rpmx test
```

`rpmx init` writes a recipe, a root profile, a README and a `.gitignore`. The
first build resolves and assembles the root; later builds reuse the cache.
Artifacts land in `packages/<arch>/`, with repository metadata already written.

Pin what was resolved and make CI reject drift:

```console
rpmx lock
rpmx build --locked
```

Rebuild from the SRPM without the recipe (the root lock is embedded):

```console
rpmx rebuild packages/x86_64/src/hello-0.1.0-1.fc44.src.rpm
```

For exhaustive flags, run `rpmx <command> --help`. For why the tool is designed
this way, see the [contributor documentation](../contributing/).
