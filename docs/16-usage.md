# Using rpmx

This is the day-to-day guide. It describes the current CLI and recipe schema as
implemented. For the rationale and the larger design, see
[`03-design.md`](03-design.md); for the UX audit against Melange, see
[`15-ux-audit.md`](15-ux-audit.md). For exhaustive flags, run
`rpmx <command> --help`.

---

## 1. What rpmx does

`rpmx` turns a strict YAML recipe into ordinary RPMs without `rpmbuild` or a
spec file:

```
package.yaml + profile + sources
        |
        v
  source snapshot  ->  native SRPM  ->  binary RPMs + SBOM + repodata
```

Every build passes through a source RPM. The SRPM contains the recipe, the
package lock, the resolved step definitions, the sources and patches. `rpmx
rebuild` starts from that SRPM, so it does not need the recipe checkout.

The three things to remember:

1. The **recipe** is intent. It names sources, steps, outputs and tests.
2. The **profile** (or an inline `build.root`) describes the machine that runs
   the build.
3. The **lock** records what those two resolved to. `--locked` refuses to drift
   from it.

---

## 2. Requirements

- Linux, x86_64 or aarch64 host.
- Rust 1.85+ to build rpmx.
- `bubblewrap` for native steps, or `podman` for root resolution and
  image-backed rebuilds.
- `dnf5` in the builder image used for root resolution.
- `tar`, `findutils`, `binutils` and the usual build tools inside the root.
- For foreign-architecture builds: `qemu-system-<arch>`, `qemu-user-static` with
  binfmt, `ssh`, `ssh-keygen`, `mke2fs`, `debugfs` and `tar`.

Check the host:

```console
rpmx doctor
rpmx doctor --json
rpmx doctor --strict     # non-zero when a required feature is missing
```

Build:

```console
cargo build --release
./target/release/rpmx --help
```

---

## 3. First package

```console
rpmx init hello
cd hello

rpmx lint
rpmx build
rpmx test
```

`rpmx init` writes:

```
hello/
  package.yaml
  profiles/fedora-44.yaml
  pipelines/          (empty; local steps go here)
  README.md
  .gitignore
```

`rpmx init --template python` and `--template metapackage` are also available.
`--profile NAME` changes the generated profile name and reference.
`--force` overwrites existing files; without it, `init` refuses.

Output:

```
packages/x86_64/hello-0.1.0-1.fc44.x86_64.rpm
packages/x86_64/src/hello-0.1.0-1.fc44.src.rpm
packages/x86_64/hello-0.1.0-1.fc44.build.spdx.json
packages/x86_64/repodata/repomd.xml
```

The first build resolves and assembles the root (network, a few minutes). The
next build reuses the cached root and is fast.

---

## 4. Command map

| Command | Use it for |
|---|---|
| `rpmx init [DIR]` | Start a package. |
| `rpmx lint [FILE]` | Check every rule, expand the matrix. |
| `rpmx plan [FILE]` | Show cells, outputs, tests and expanded commands. |
| `rpmx build [FILE]` | Resolve root, fetch, snapshot, SRPM, build, package, index. |
| `rpmx test [FILE]` | Run tests, building first when needed. |
| `rpmx rebuild SRPM` | Rebuild a source RPM, assembling its embedded root. |
| `rpmx lock [FILE]` | Write or check `package.lock.json`. |
| `rpmx explain TOPIC` | Explain policy, root, steps, matrix, requires or files. |
| `rpmx bump VERSION [FILE]` | Update version, source URLs and digests. |
| `rpmx repo` | Write repodata for a directory of RPMs. |
| `rpmx shell` | Open a shell with the build mounts. |
| `rpmx steps list\|show` | Inspect the built-in step library. |
| `rpmx doctor` | Check host tools, runners and cache. |
| `rpmx fetch` | Populate the source cache. |
| `rpmx source` | Produce the deterministic source snapshot. |
| `rpmx srpm` | Write an SRPM from an existing snapshot. |
| `rpmx root resolve\|assemble\|digest` | Explicit root plumbing for debugging. |
| `rpmx keygen` / `rpmx sign` | Keys and package signatures. |
| `rpmx cache gc` | Remove unreferenced cache blobs. |

Every recipe command defaults `FILE` to `./package.yaml`. `build`, `test` and
`rebuild` default output to `./packages/<arch>/` and jobs to all available
CPUs.

Global options:

| Option | Meaning |
|---|---|
| `--pipeline-dir DIR` | Where local step files live (default: `<recipe>/pipelines`). |
| `--keep-workdir` | Keep temporary directories and print their paths. |
| `--debug` | Echo step commands before running them. |

---

## 5. Recipe reference

A recipe is one YAML document with a closed schema. Unknown fields are errors.
`schema` must be `1`.

### 5.1 Package metadata

```yaml
schema: 1

package:
  name: hello
  epoch: 0                  # optional, default 0
  version: "0.1.0"
  release: 1                # string or integer
  summary: A tiny hello package
  description: |
    A longer description used in the RPM header.
  license: MIT
  source-date-epoch: 1704067200
  upstream: https://example.org/hello
  maintainers:
    - name: Your Name
      email: you@example.org
  vcs:
    type: git
    url: https://example.org/hello.git
    tag: v0.1.0
  cpe:
    vendor: example
    product: hello
```

`name`, `version`, `release`, `summary` and `license` are required.
`source-date-epoch` is strongly recommended: it drives file mtimes, SBOM dates
and signature time. Without it the timestamp defaults to `0` and `lint` warns.

### 5.2 Build root

The recipe names the root in one of two ways.

Named profile:

```yaml
build:
  root:
    profile: fedora-44
```

The name is resolved relative to the recipe first
(`./profiles/fedora-44.yaml`, `./profiles/fedora-44.profile.yaml`), then
`$XDG_CONFIG_HOME/rpmx/profiles/`. An explicit relative path
(`profile: profiles/review.yaml`) is relative to the recipe, not the shell's
working directory.

Inline root:

```yaml
build:
  root:
    releasever: "44"
    archs: [x86_64]
    repositories:
      - id: fedora
        metalink: https://mirrors.fedoraproject.org/metalink?repo=fedora-$releasever&arch=$basearch
      - id: updates
        metalink: https://mirrors.fedoraproject.org/metalink?repo=updates-released-f$releasever&arch=$basearch
    packages: [bash]
    builder:
      image: registry.fedoraproject.org/fedora:44
```

A root declaration is either a `profile` name or inline fields, never both.
The inline `packages` are merged with everything the recipe needs.

If `build.root` is absent, `--profile FILE` supplies the root. If neither is
present, `build`/`test` fail with `RPMX-ROOT-016`.

**What goes into the root.** The profile's `packages.install`, plus:

- `build.dependencies.tools`
- `build.dependencies.host`
- `build.dependencies.target`
- `build.dependencies.tests`
- every `needs.tools` from every resolved step (source-assembly, build, tests)

`tools` entries are **package names**, not command names. Use `bash` rather
than `sh`, `gcc` rather than `cc`.

### 5.3 Build dependencies

```yaml
build:
  dependencies:
    tools: [gcc, make, bash, pkgconf-pkg-config]
    host: [zlib-devel, openssl-devel]
    target: []
    tests: [python3-pytest, jq]
  environment:
    CFLAGS: "-O2 -g"
```

All scopes are installed into the same build root today. `tests` is used when
building the installed-stage test root. `environment` is merged into every
build step's environment; a step's own `env` wins.

### 5.4 Sources

```yaml
sources:
  - name: hello-0.1.0.tar.gz
    kind: archive
    digest: "sha512:..."
    locators:
      - kind: https
        url: https://example.org/hello-0.1.0.tar.gz
      - kind: https
        url: https://mirror.example.org/hello-0.1.0.tar.gz
    extract:
      format: auto          # auto, tar, tar.gz, tar.xz, tar.zst, none
      strip-components: 1
      destination: .
```

- `digest` is `sha512:` (128 hex) or `sha256:` (64 hex). The algorithm is
  recorded and verified.
- Locator kinds implemented today: `https`, `file`, `git`, `go-module`.
  `file` paths are relative to the recipe unless absolute. `go-module` needs
  `source.module`, `source.version`, `source.sum`, `locator.proxy` and
  `locator.sumdb`.
- `kind: archive` (or a `.tar.gz`/`.tgz`/`.tar.xz`/`.tar.zst` name) extracts by
  default. `format: auto` detects gzip, xz and zstd by magic bytes.
- Extraction defaults to `strip-components: 1`, so the tarball's top directory
  is removed and the source tree lands at the workspace root.
- `destination` must stay under the workspace. Source names are single path
  components.
- To add a source's digest after a mismatch, run `rpmx fetch` and copy the
  digest from the error, or use `rpmx bump`.

Patches are a top-level ordered list. They are applied in order after sources
are materialised and before source-assembly:

```yaml
patches:
  - file: patches/0001-fix.patch
    strip: 1
    reason: Backport of upstream commit abc123
```

The patched tree is part of the source snapshot, so a rebuild does not reapply
patches.

### 5.5 Source assembly

Source assembly runs in a locked root before the SRPM is written. Use it for
deterministic generation and vendoring:

```yaml
source-assembly:
  pipeline:
    - runs: |
        python3 generate-tables.py > tables.c
    - uses: go/vendor
    - uses: cargo/vendor
```

Anything installed here becomes part of the source snapshot and the SRPM. It
does not run during `rpmx rebuild`.

### 5.6 Build pipeline

A pipeline is an ordered list. Each entry is exactly one of:

```yaml
- uses: cmake                 # built-in bundle or local step
  with:
    args: [-DBUILD_TESTING=ON]

- name: fix-prefix            # label only
  argv: [sed, -i, 's|/usr/local|/usr|g', Makefile]

- name: generate-header
  runs: |
    set -eu
    python3 generate.py > generated.h
```

Common fields on any entry: `name`, `cwd`, `env`, `timeout` (stored but not yet
enforced per step; use `--step-timeout`), `reason`.

`runs` is executed as `sh -c <script>` in the sandbox. It is not `-euo
pipefail`; add `set -eu` when you need it. `argv` never goes through a shell.

Built-in steps can be listed with:

```console
rpmx steps list
rpmx steps show cmake/configure
```

Bundles such as `cmake`, `meson`, `autotools`, `make`, `python` expand to
several steps. `strip` is a step. `go/build`, `go/test`, `go/vendor`,
`go/bump`, `cargo/vendor`, `npm/ci`, `python/build-wheel` and
`python/install-wheel` are available.

A named local step is a YAML file under `pipelines/`:

```yaml
# pipelines/acme/install-config.yaml
name: acme/install-config
version: 1
summary: Install a config file
inputs:
  source:
    type: string
    required: true
run:
  cwd: /tmp/build
  argv: [install, -D, -m, "0644", "${inputs.source}", /tmp/stage/etc/acme.conf]
needs:
  tools: [coreutils]
```

Use it as `uses: acme/install-config`. The step's `needs.tools` also joins the
root requirement. Local steps are hashed into the lock and embedded in the
SRPM.

### 5.7 Outputs and files

```yaml
outputs:
  - name: hello
    summary: The hello program
    description: A longer description.
    arch: any                 # or noarch
    metapackage: false
    files:
      include:
        - /usr/bin/hello
        - /usr/share/man/**
      exclude:
        - /usr/share/man/man1/old.1
      license:
        - /usr/share/licenses/hello/**
      doc:
        - /usr/share/doc/hello/**
      config:
        - /etc/hello.conf
      noreplace:
        - /etc/hello.conf
      directories:
        - path: /var/lib/hello
          mode: "0750"
          user: root
          group: root
      attributes:
        - path: /usr/bin/hello
          mode: "0755"
          user: root
          group: root
        - path: /usr/bin/hello-ping
          caps: cap_net_raw=ep
    dependencies:
      run:
        - "hello-libs = ${package.version}-${package.release}"
      recommends: [hello-doc]
      suggests: [jq]
      supplements: [hello-extra]
      enhances: [http-client]
      provides: [hello-cli]
      conflicts: ["hello-old < 1.0"]
      obsoletes: ["hello-old < 1.0"]
    scriptlets:
      interpreter: /bin/sh
      post: |
        echo installed
```

Rules:

- Every staged path must be claimed by exactly one output; an unclaimed file is
  an error.
- A path claimed by two outputs is an error.
- An output whose selectors match no staged files is an error.
- `metapackage: true` allows an output with no files.
- `scripts` are raw shell and remain the escape hatch. Typed actions are not yet
  implemented.

`scriptlets` phases: `pre`, `post`, `preun`, `postun`; `interpreter` defaults to
`/bin/sh`.

### 5.8 Tests

```yaml
tests:
  - name: unit
    stage: build
    argv: [ctest, --output-on-failure]

  - name: cli
    stage: installed
    argv: ["hello", "--version"]

  - name: scripted
    stage: installed
    pipeline:
      - runs: |
          hello | grep -q hello
```

- `stage: build` runs in the build sandbox, with the source workspace present.
- `stage: installed` installs the built RPMs into a disposable test root and
  runs there.
- `stage: package` is not implemented yet.
- Tests run only with `rpmx build --run-tests`, `rpmx test`, or when a filter is
  given.
- `--test NAME` selects tests; `--junit FILE` writes JUnit XML.
- Installed tests use `build.dependencies.tests` plus the built RPMs. Real
  dependency checking is performed; there is no `--nodeps`.

### 5.9 Matrix and variables

Two mechanisms, both resolved before any build:

```yaml
vars:
  py_tag: "${matrix.python.tag}"

matrix:
  axes:
    python:
      - value: "3.10"
        version: "3.10"
        tag: "310"
        slug: py310
        package: python3.10
        site: /usr/lib/python3.10/site-packages
      - value: "3.12"
        version: "3.12"
        tag: "312"
        slug: py312
        package: python3.12
        site: /usr/lib/python3.12/site-packages
  exclude:
    - python: "3.10"
  max-cells: 20
  release-suffix: true
```

Use `${matrix.python.version}`, `${cell.id}`, `${cell.slug}`, `${vars.py_tag}`,
`${package.version}`, `${arch}` and the short forms `${version}`,
`${release}`. Templates are variable substitution only: no expressions, no
conditionals, no shell. Use `$${...}` for a literal.

A matrix cell gets its own release, dependency set and SRPM. `--cell ID`
selects one cell for a per-cell command.

---

## 6. Profiles

A profile is root policy, not a recipe:

```yaml
document: rpmx-profile
version: 1
name: fedora-44
releasever: "44"
dist-tag: .fc44
archs: [x86_64]
builder:
  image: registry.fedoraproject.org/fedora:44
repositories:
  - id: fedora
    metalink: https://mirrors.fedoraproject.org/metalink?repo=fedora-$releasever&arch=$basearch
  - id: updates
    metalink: https://mirrors.fedoraproject.org/metalink?repo=updates-released-f$releasever&arch=$basearch
packages:
  install: [bash, coreutils, gcc, make]
  reinstall: []
options:
  weak_deps: false
  docs: false
debuginfo: false
sbom: true
```

A repository sets exactly one of `baseurl`, `metalink`, `mirrorlist`, plus an
optional `gpgkey`. `dist-tag` is appended to every release. `debuginfo` and
`sbom` are recorded in the root lock and applied at build time. Module streams
and `reinstall` are rejected today.

### Local repositories

`baseurl` may point at a host directory:

```yaml
repositories:
  - id: local
    baseurl: file:///srv/rpmx/packages
    gpgkey: file:///srv/rpmx/keys/repo.asc
```

rpmx mounts the directory read-only into the resolver container and rewrites
the URL inside the container. A local `gpgkey` is copied into the resolver work
directory. This lets a package built locally satisfy another package's build
dependency:

```console
rpmx repo --rpms out/ --output /srv/rpmx/packages
# profile has local + distro repositories, and package A is in `packages.install`
rpmx build B/package.yaml
```

The path must exist on the resolve host and be absolute. The resulting lock
records the original repository id; for another machine, use `--root-lock` and
`root assemble` from the CAS, or publish the repository over HTTPS.

---

## 7. Roots

### Automatic root (normal use)

With `build.root` or `--profile`, `build`, `test`, `source` and `lock` do this
automatically:

1. Merge the profile packages with the plan's dependencies and step needs.
2. Hash the requirement.
3. Look under `$XDG_CACHE_HOME/rpmx/roots/<requirement-digest>/`.
4. On a miss, resolve with dnf5, assemble offline, verify the digest and record
   `root.lock.yaml`.
5. Reuse the root on the next build.

`--offline` never resolves or assembles; it fails with the missing digest.
`--cache DIR` changes the CAS (roots live beside it).

### Explicit root

For debugging or a prebuilt root:

```console
rpmx root resolve profile.yaml --output root.lock.yaml
rpmx root assemble root.lock.yaml --root ./root --output root.assembled.yaml
rpmx build package.yaml --root-lock root.assembled.yaml --root ./root
```

Explicit roots are verified by hashing the tree. Auto-resolved roots are
trusted on a cache hit, with a mtime check; use explicit `--root` when you need
proof. `rpmx root digest ROOT` computes the deterministic digest. On a root
with mode-000 files it re-enters a user namespace via `unshare` or `podman
unshare`.

`root export` and `root import` translate a lock to and from a libpkgmanifest
`rpm-package-manifest`.

---

## 8. Building

```console
rpmx build                       # defaults to ./package.yaml
rpmx build hello/package.yaml
rpmx build --output /tmp/out
rpmx build --jobs 32
rpmx build --run-tests
rpmx build --test installed --junit report.xml
rpmx build --stage /tmp/stage    # package an existing tree, skip the pipeline
rpmx build --locked
rpmx build --offline
rpmx build --no-index
rpmx build --signing-key key.asc
```

`--stage DIR` packages a staged tree; it does not run source assembly, SRPM
creation or builds. It verifies the lock when `--locked` is given.
`--test` implies `--run-tests`.

The full build runs:

```
root -> fetch -> source snapshot -> SRPM -> extract SRPM -> build steps
     -> package -> SBOM -> tests -> sign -> repodata
```

Every step prints its label and duration to stderr. Failure output includes the
command, cwd, a hint, and `--keep-workdir` leaves the workspace. `--debug` runs
`runs:` scripts with `set -x` and echoes the command.

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

## 9. The package lock

```console
rpmx lock
rpmx lock --check
rpmx build --locked
rpmx source --locked
```

`package.lock.json` records:

- the recipe digest;
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

## 10. Tests

### Build and test in one run

```console
rpmx test
rpmx test --test cli
rpmx test --test unit --test installed --junit report.xml
rpmx test --json
```

This builds (source, SRPM, RPMs) with tests enabled, then runs the selected
tests with the workspace and the produced RPMs available. `--json` lists the
produced RPMs; use `--junit` for per-test results.

### Test existing RPMs

```console
rpmx build
rpmx test --install-dir packages/x86_64
rpmx test --install-dir packages/x86_64 --test installed
rpmx test --install-dir packages/x86_64 --json --junit report.xml
```

- Reads `*.rpm` from the directory, ignores `*.src.rpm`.
- Skips the build entirely.
- Runs installed-stage tests only. With no `--test`, build-stage tests are
  skipped; naming one is an error.
- Needs a root lock, not an assembled `--root`; the test root is prepared from
  the lock and the supplied RPMs.
- JSON: `{schema, ok, command: "test", tests: [{name, stage, ok, duration_ms,
  message}]}`.
- Exit 6 on failure.

Test roots are disposable and use real dependency checking. If the test root
cannot install its packages, add the missing runtime packages to
`build.dependencies.tests` or pass `--test-root-lock` on `build`/`rebuild`.

---

## 11. Linting and errors

```console
rpmx lint
rpmx lint --strict
rpmx lint --json
rpmx lint --profile fedora-44
```

`lint` parses, plans and applies semantic rules. Current rules include:

- SPDX syntax for `license`;
- empty/very short or very long summary;
- release starting at zero;
- missing `source-date-epoch`;
- duplicate dependency or tool entries;
- duplicate source/output/test names;
- missing patch files;
- `http` locators;
- missing maintainers;
- empty tests and empty build pipelines;
- unused `vars`.

`--strict` makes warnings fatal. Every diagnostic has a stable code such as
`RPMX-LINT-010`.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Success. |
| 2 | Recipe, schema, plan or lint error; usage error. |
| 3 | Lock drift (recipe, sources, steps, root closure). |
| 4 | Root or source resolution/fetch failure. |
| 5 | Build or packaging failure. |
| 6 | Test failure. |
| 7 | Policy or signing failure. |
| 8 | SBOM or extraction/evidence failure. |
| 9 | Internal or I/O failure. |

`doctor --strict` uses 9 when a required host feature is missing.

---

## 12. Inspecting and debugging

```console
rpmx plan
rpmx plan --json
rpmx explain policy
rpmx explain root
rpmx explain steps
rpmx explain matrix
rpmx explain requires
rpmx explain files
rpmx explain root --json
rpmx steps list
rpmx steps show cmake/configure
rpmx doctor
```

`explain policy` shows the effective dist tag (including one from
`build.root.profile`), build release, profile, release and source date epoch.
`explain root` shows the merged package set and requirement digest.

On failure:

```console
rpmx build --keep-workdir
# stderr prints: work directory: /tmp/...
rpmx shell --root /path/to/root \
  --workspace /tmp/.../build --stage /tmp/.../stage
```

`rpmx shell` uses the same bubblewrap mounts as a build step: root read-only at
`/`, workspace at `/tmp/build`, stage at `/tmp/stage`, no network, cleared
environment. `--command 'make -j8'` runs one command instead of an interactive
shell.

`--debug` echoes the resolved command and sets `set -x` for `runs:` steps.

---

## 13. Updating a package

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

## 14. Repositories, keys and the cache

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

## 15. SRPM-first plumbing

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

The SRPM embeds the recipe, package lock, resolved step definitions, sources and
patches. It does not need the original checkout. A rebuild uses the embedded
step definitions and rejects a digest mismatch (`RPMX-SRPM-011`).

---

## 16. JSON output

Every command with `--json` writes exactly one document to stdout. Logs and
step output go to stderr, so this is safe:

```console
rpmx build --json 2>/dev/null | jq .artifacts
```

Success:

```json
{
  "schema": 1,
  "ok": true,
  "command": "build",
  "artifacts": [
    {"kind": "srpm", "path": "packages/x86_64/src/hello-0.1.0-1.fc44.src.rpm"},
    {"kind": "rpm", "path": "packages/x86_64/hello-0.1.0-1.fc44.x86_64.rpm"},
    {"kind": "evidence", "path": "packages/x86_64/hello-0.1.0-1.fc44.build.spdx.json"},
    {"kind": "repository", "path": "packages/x86_64/repodata/repomd.xml"}
  ]
}
```

Failure:

```json
{
  "schema": 1,
  "ok": false,
  "command": "build",
  "diagnostics": [
    {
      "severity": "error",
      "code": "RPMX-SANDBOX-002",
      "message": "step 'compile' failed with exit 3\n  command: ...\n  cwd: ...\n  help: ...",
      "path": null,
      "line": null,
      "column": null
    }
  ]
}
```

`lint --json` includes `summary.errors`, `summary.warnings` and all
diagnostics. `explain --json`, `test --install-dir --json`, `doctor --json` and
`steps list --json` use the same envelope.

---

## 17. Environment variables

| Variable | Effect |
|---|---|
| `XDG_CACHE_HOME` | Base for the CAS and root cache. |
| `TMPDIR` | Location of temporary work directories (large builds need space). |
| `RPMX_PROXY` / `RPMX_ACQUISITION_PROXY` | Default acquisition proxy. |
| `RPMX_QEMU_SYSTEM` | Override the `qemu-system-<arch>` binary. |
| `<VAR>` named by `--signing-passphrase-env` / `--passphrase-env` | Signing key passphrase. |

Steps see a fixed environment: `PATH`, `HOME`, `TMPDIR`, `LANG`, `LC_ALL`,
`TZ`, `SOURCE_DATE_EPOCH`, `SRC`, `BUILD`, `STAGE`, `JOBS`, `RPMX_HOST_ARCH`,
`RPMX_TARGET_ARCH`, `RPMX_CROSS` and `RPMX_DEBUGINFO`. The host environment is
not inherited. `SOURCE_DATE_EPOCH`, `SRC`, `BUILD`, `STAGE` and `JOBS` are
reserved and cannot be set by a step.

---

## 18. Troubleshooting

**`RPMX-ROOT-016: a full build requires --root-lock and --root`**  
The recipe has no `build.root` and no `--profile`. Add a root or pass explicit
root flags.

**`RPMX-ROOT-016: root ... is not cached ...` with `--offline`**  
Resolve once without `--offline`, or use the root lock that produced the cache.

**Root resolution is slow**  
The first resolve downloads and assembles the closure. Later builds reuse
`~/.cache/rpmx/roots/`. `rpmx doctor` checks dnf5 and the runner.

**Test root install fails**  
The test root needs runtime dependencies. Add them to
`build.dependencies.tests`, or use `--test-root-lock` with a pinned closure.
Ensure `TMPDIR` has space; a test root is a full distribution root.

**A step failed**  
Read the `command`, `cwd` and help line. Re-run with `--keep-workdir` and enter
the workspace with `rpmx shell` (using the root and paths printed by the build),
or add `set -eu`/`set -x` to the step. `--debug` sets `set -x`.

**`rpmx root digest` fails with permission denied**  
Some roots contain mode-000 files. The command falls back to `unshare -Ur` or
`podman unshare`; install one of them, or run as a user namespace that can read
the tree.

**A local `file://` repository is ignored**  
Use an absolute path that exists on the resolve host. It is mounted read-only
during `root resolve`; `root assemble` uses the CAS, not the repository.

**`--locked` reports drift**  
Review `rpmx lock --check` output, run `rpmx lock`, and review the
`package.lock.json` diff. Do not hand-edit step digests.

---

## 19. Current limitations

- Typed `features` are not implemented yet.
- Typed `actions` (systemd, sysusers, tmpfiles, alternatives) are not
  implemented; use `scriptlets` raw shell.
- `update:` metadata and `rpmx update` are not implemented; `rpmx bump` handles
  manual version updates.
- `build.dependencies.tools/host/target/tests` all land in one root; there is no
  separate sysroot for cross builds.
- Cross-compilation is not implemented. Foreign targets use a full qemu-system
  VM and need a bootable target root.
- `stage: package` tests, ABI tests and reproducibility tests are not
  implemented.
- Rich dependencies are not implemented; weak dependencies are.
- SELinux labels, `%ghost`, `%lang` and arbitrary xattrs are not implemented;
  file capabilities are.
- `-debugsource` and per-subpackage debuginfo are not implemented.
- Step-level `timeout` is parsed but not enforced individually; use
  `--step-timeout`.
- Local `file://` repository paths are host-specific and not portable.
- There is no config file yet; use flags and profiles.

---

## 20. Complete example

```yaml
schema: 1

package:
  name: hello
  version: "0.1.0"
  release: 1
  summary: A tiny hello package
  license: MIT
  source-date-epoch: 1704067200
  maintainers:
    - name: Your Name
      email: you@example.org

build:
  root:
    profile: fedora-44
  dependencies:
    tools: [gcc, make, bash]
  pipeline:
    - uses: make
    - runs: |
        install -D -m 0644 LICENSE "$STAGE/usr/share/licenses/hello/LICENSE"
    - uses: strip

source-assembly:
  pipeline:
    - runs: |
        cat > hello.c <<'SOURCE'
        #include <stdio.h>
        int main(void) { puts("hello"); return 0; }
        SOURCE
        cat > Makefile <<'SOURCE'
        all: hello
        hello: hello.c
        	$(CC) $(CFLAGS) -o $@ hello.c
        check: hello
        	./hello
        install: hello
        	install -D -m 0755 hello $(DESTDIR)/usr/bin/hello
        SOURCE
        cat > LICENSE <<'SOURCE'
        MIT License
        SOURCE

outputs:
  - name: hello
    summary: The hello program
    files:
      include:
        - /usr/bin/hello
        - /usr/share/licenses/hello/LICENSE
      license:
        - /usr/share/licenses/hello/LICENSE

tests:
  - name: smoke
    stage: installed
    argv: ["hello"]
```

```console
rpmx lint
rpmx lock
rpmx build --locked
rpmx test --install-dir packages/x86_64
```
