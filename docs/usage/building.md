# Building, locks and artifacts

## Building

```console
rpmx build                       # defaults to ./package.yaml
rpmx build hello/package.yaml
rpmx build --output /tmp/out                 # /tmp/out/<arch>/
rpmx build --arch x86_64,aarch64 --arch-jobs 2
rpmx build --arch all --arch-jobs all
rpmx build --jobs 32
rpmx build --no-tests              # skip the tests a build runs by default
rpmx build --test installed --junit report.xml
rpmx build --stage /tmp/stage    # package an existing tree, skip the pipeline
rpmx build --locked
rpmx build --offline
rpmx build --no-index
rpmx build --signing-key key.asc
```

`--output ROOT` is an artifacts root; every architecture writes to
`ROOT/<arch>/`, so the default is `packages/<arch>/`. `--arch-jobs` defaults to
the number of architectures (capped by available parallelism) for bwrap/binfmt
and to 1 for `--runner qemu`. `--jobs` is divided across the active
architectures.

`--stage DIR` packages a staged tree; it does not run source assembly, SRPM
creation or builds. It verifies the lock when `--locked` is given.

A build runs the recipe's tests after it packages them, so a green build means
the same thing it means under `rpmbuild`. `--no-tests` skips them, and
`--skip-installed` runs only build-stage tests (the installed stage needs to
execute the target binary). `--test NAME` runs one test instead of all; use
`--test OUTPUT/NAME` when an output-level test shares its bare name with
another output.

The full build runs:

```
root -> fetch -> source snapshot -> SRPM -> extract SRPM -> build steps
     -> package -> SBOM -> tests -> sign -> repodata
```

Progress goes to stderr as leveled lines, with guest stdout labelled `INFO` and
guest stderr labelled `WARN`:

```text
INFO plan     [x86_64] hello 0.1.0-1.fc44
INFO root     [x86_64] fedora-44-build · 209 packages · cache hit
INFO root     [x86_64] builder registry.fedoraproject.org/fedora:44
INFO root     [x86_64] repositories fedora, updates
INFO source   [x86_64] assembling RPMX-SOURCE.tar.gz
INFO source   [x86_64] step [1/1] runs
INFO source   [x86_64] RPMX-SOURCE.tar.gz · 0.4s
INFO srpm     [x86_64] hello-0.1.0-1.fc44.src.rpm · 0.2s
INFO wrote    [x86_64] packages/x86_64/src/hello-0.1.0-1.fc44.src.rpm
INFO build    [x86_64] 3 steps
INFO build    [x86_64] step [1/3] runs
INFO build    [x86_64] step [2/3] make
INFO [x86_64] gcc -O2 -Wall -Wextra -Werror -o hello hello.c
INFO build    [x86_64] step [3/3] strip
INFO package  [x86_64] hello · 9 files · 41.2 KiB
INFO package  [x86_64] hello · 0.1s
INFO wrote    [x86_64] packages/x86_64/hello-0.1.0-1.fc44.x86_64.rpm
INFO index    [x86_64] 1 package in packages/x86_64
INFO done     [x86_64] 4 artifacts in packages/x86_64 · 12.4s
```

Every line carries the stage it belongs to: step headers, guest output, and the
tool's own events.

stdout carries only the artifact list, one path per line, or the single
`--json` document. A first build reports each package entering the root as
`INFO install <name>-<evr>.<arch>`. `--verbose` lists every resolved root
package and each step's duration; `--color=auto|always|never` controls ANSI
color; `--debug` runs `runs:` scripts with `set -x` and echoes the command.
A failing step adds the command, cwd, a hint, and `--keep-workdir` leaves the
workspace. Packaging reports every output's file count and payload size. If the
main package receives development headers, static libraries, pkg-config/CMake
files, documentation or locale source data, the package lint warns and names
the matching `@dev`, `@static`, `@pkgconfig`, `@doc` or `@locales` preset.

### Artifacts

| Path | Meaning |
|---|---|
| `packages/<arch>/<name>-<ver>-<rel>.<arch>.rpm` | Binary RPMs. |
| `packages/<arch>/src/<name>-<ver>-<rel>.src.rpm` | Native source RPM. |
| `packages/<arch>/<name>-<ver>-<rel>.build.spdx.json` | Build-level SPDX 2.3. |
| `packages/<arch>/repodata/` | Repository metadata. |
| `packages/<arch>/repodata/repomd.xml.asc` | Present when signed. |

Each binary RPM embeds an SPDX document at
`/usr/share/sbom/<name>-<version>-<release>.spdx.json` when `sbom: true`.
`--sbom FILE` overrides only the build-level path.

### Signing

```console
rpmx keygen --output key.asc
rpmx build --signing-key key.asc
rpmx repo --rpms packages/x86_64 --output repo --signing-key key.asc
rpmx sign --signing-key key.asc packages/x86_64/*.rpm
```

rpmx never stores or discovers keys. `--signing-passphrase-env VAR` reads a
passphrase from the environment. `--signing-timestamp` defaults to the source
date epoch (then now); signed reproducible builds need a deterministic epoch.

---

## The package lock

```console
rpmx lock
rpmx lock --check
rpmx build --locked
rpmx source --locked
```

`package.lock.json` records:

- the recipe bundle digest and every recipe file with its digest;
- the context directory path and its tree digest, when the recipe declares one;
- the effective source date epoch;
- policy (dist tag, build release);
- sources (name and digest);
- every cell (id, package, version, release, outputs);
- step digests for source-assembly, build and every test, with a `scope`;
- the root requirement digest, architecture and, when known,
  `closure_digest` and `root_digest`.

`rpmx lock --root-lock FILE` records the exact resolved closure even when the
recipe does not declare a root. `--check` exits 3 on drift. `build --locked`
verifies the recipe, sources, steps, policy and epoch before resolving the root,
then verifies the active root closure. A lock that pins a closure cannot be used
with the host root or a different root; `--stage` verifies everything except
the root because no root is used.

To update: `rpmx lock`, review the diff, then `--locked`.

---

## Updating a package

```console
rpmx bump 1.5.8                 # downloads sources, updates version/URLs/digests
rpmx bump 1.5.8 --dry-run
rpmx bump 1.5.8 --no-fetch      # version and URLs only
rpmx bump 1.5.8 --json --dry-run
```

`bump` rewrites the `package.version` scalar (preserving quoting and comments),
updates `url:` lines inside `sources:`, downloads each source and updates its
own digest. It writes atomically. If the new recipe does not parse back to the
requested version, it refuses to write. A 404 or digest failure leaves the
recipe unchanged. Flow-style locators and URLs outside `sources:` are not
rewritten; edit those by hand.

---

## Repositories, keys and the cache

```console
rpmx repo --rpms out/ --output repo/
rpmx repo --rpms out/ --output repo/ --signing-key key.asc
rpmx cache gc --lock root.lock.yaml --recipe package.yaml --dry-run
rpmx cache gc --lock root.lock.yaml --recipe package.yaml
```

`repo` copies RPMs into the output directory and writes `repodata/`. If a
signing key is given it also writes `repomd.xml.asc`.

`cache gc` keeps blobs referenced by the given locks and recipes and deletes
unreferenced blobs older than `--older-than-hours` (default 24). Always run
`--dry-run` first.

Cache locations:

- CAS: `$XDG_CACHE_HOME/rpmx/cas` (or `~/.cache/rpmx/cas`).
- Roots: beside it, `$XDG_CACHE_HOME/rpmx/roots/`.
- Override with `--cache DIR`.

---

## SRPM-first plumbing

These commands are useful for debugging and CI:

```console
rpmx fetch package.yaml
rpmx source package.yaml --output snapshot/
rpmx srpm package.yaml --sources snapshot/ --output out/
rpmx rebuild out/hello-0.1.0-1.fc44.src.rpm
```

`source` writes `RPMX-SOURCE.tar.gz`; `srpm` packages a flat snapshot directory.
`build` runs both internally.

`rebuild` extracts the SRPM, reads its authoritative `package.lock.json`, and
uses the embedded `root.lock.json`. If the root is not cached, it assembles it
from the CAS automatically. `--offline` fails if the root or a source is
missing. `--root` or `--root-image` override the embedded root.

The SRPM embeds the recipe and every included fragment, the package lock,
resolved step definitions, sources and patches. It does not need the original checkout. A rebuild uses the embedded
step definitions and rejects a digest mismatch (`RPMX-SRPM-011`).

---
