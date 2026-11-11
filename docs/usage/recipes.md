# Recipes

## Recipe reference

A recipe is one YAML document with a closed schema, or a bundle of files composed with `include:`. Unknown fields are errors. `schema` must be `1` in the root file.

A recipe can also be split into one file per top-level section:

```yaml
# package.yaml
schema: 1
include:
  - sources.yaml
  - build.yaml
  - outputs.yaml
  - tests.yaml

package:
  name: hello
  version: "0.1.0"
  release: 1
  summary: A tiny hello package
  license: MIT
```

```yaml
# build.yaml
build:
  root:
    profile: profiles/fedora-44.yaml
  packages: [gcc, make, sh]
  pipeline:
    - uses: make
```

Rules:

- Include paths are relative to the file that names them.
- The root recipe declares `schema`; included fragments must not.
- Every top-level section has exactly one owner across the bundle. A duplicate
  section is an error naming both files; there is no overlay merge.
- Includes may be nested and must stay inside the root recipe's directory.
- `rpmx lock` records every file and its digest, and the SRPM embeds them at
  their original relative paths, so `rpmx rebuild` stays self-contained.
- `rpmx bump` edits the file that owns `package` and the file that owns
  `sources`.
- `rpmx init --split` writes this layout for a new package.

See [`examples/jq/`](../../examples/jq/) for a tested split recipe.

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

`source-date-epoch` is optional: it drives file mtimes, SBOM dates and signature
time. rpmx derives it from the newest source archive mtime, or from the pinned
git commit time, and uses the newest value when a recipe has several sources.
A repository-wide fallback in `rpmx-defaults.yaml` covers packages with no
source of their own. Set the recipe field only to override the derived value.
Without any derivable source or fallback, the timestamp is `0` and `lint`
warns.

#### Repository defaults

A `rpmx-defaults.yaml` can supply package metadata and build environment that
are identical for every recipe in a repository:

```yaml
schema: 1
package:
  source-date-epoch: 1704067200  # fallback for packages with no source
  maintainers:
    - name: Example Linux maintainers
      email: maintainers@example.org
build:
  environment:
    PATH: /usr/libexec/example/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
dependencies:
  run: [filesystem]  # every output of every package requires this
```

rpmx looks in the recipe directory first, then each ancestor up to the
repository root (the first directory containing `.git`). The recipe wins when
it sets the field itself. `build.environment` entries are added when the
recipe does not already set the same variable, so a recipe can still override
one key without restating the rest. An optional `metadata` map stores repository
policy for other tools; rpmx preserves it without adding recipe variables.
The defaults file joins the recipe bundle,
so it is recorded in the package lock and embedded in the SRPM; a rebuild does
not need the original checkout.

Keep repository defaults narrow. License, upstream URL, version, summary and
CPE describe the upstream component and belong in the recipe. CPE must identify
the upstream project, not the distribution package name.

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

The first output is always the main package. It receives every staged file
that no later output claims, matching Wolfi/melange and keeping the recipe
focused on the build plus the deliberate subpackage splits. File presets are
available in `files.include` and `files.exclude`:

| Preset | Expands to |
| --- | --- |
| `@dev` | headers, unversioned `.so` linker scripts, pkg-config, aclocal and CMake files |
| `@headers` | `/usr/include/**` |
| `@static` | `/usr/lib*/lib*.a` |
| `@pkgconfig` | `/usr/lib*/pkgconfig/**`, `/usr/share/pkgconfig/**` |
| `@doc` | `/usr/share/doc/**` |
| `@man` | `/usr/share/man/**` |
| `@info` | `/usr/share/info/**` |
| `@locales` | `/usr/share/i18n/**`, `/usr/share/locale/**` |
| `@debug` | `/usr/lib/debug/**`, `/usr/lib*/*.debug` |

**What goes into the root.** The profile's `packages.install`, plus:

- `build.packages`
- `build.sysroot`
- every `needs.packages` and `needs.sysroot` from every resolved step
  (source-assembly, build)

Entries are **package names**, not command names. Use `bash` rather than `sh`,
`gcc` rather than `cc`.

#### Compiler policy

A recipe does not remove or replace distribution build flags. Compiler
hardening, optimisation and the target ISA baseline are installed by a
package in the build root, usually as compiler specs or wrapper scripts. A
recipe that must opt out sets the package's documented environment variable,
for example:

```yaml
build:
  environment:
    GCC_SPEC_FILE: no-hardening.spec
```

Because the policy package is part of the root closure, it is recorded in the
root lock, the package lock and the SRPM. `rpmx rebuild` therefore reproduces
the same compiler policy without the original checkout.

### 5.3 Build dependencies

```yaml
build:
  packages: [gcc, make, bash, pkgconf-pkg-config]
  sysroot: [zlib-devel, openssl-devel]
  environment:
    CFLAGS: "-O2 -g"
```

Two lists, two roots. `packages` is the build root: the environment the build
steps run in. `sysroot` is the target root whose headers and libraries the
compiler sees. For a native build both are installed into the same root. In a
cross build (a profile with a `cross` block) `packages` is installed in the
host build root and `sysroot` in the target sysroot; see
[`cross-compilation.md`](cross-compilation.md). Test-root packages are not
here; they belong to the test that needs them, in `tests[].packages`.
`environment` is merged into every build step's environment; a step's own
`env` wins.

#### 5.3.1 Automatic dependencies

Every output also gets dependencies derived from the files it actually ships,
merged with the ones declared above:

| File class | Generated |
|---|---|
| ELF | `libz.so.1()(64bit)` provides and requires, versioned symbols, and a `.so` symlink requiring its target's soname |
| pkgconfig | `pkgconfig(zlib) = 1.2` provides and `pkgconfig(libcrypto) >= 3` requires |
| Executable `#!` | the interpreter path, such as `/usr/bin/python3` |
| Python | `python(abi) = 3.12`, plus `python3.12dist(foo) = 1.2` provides and requires from `.dist-info`/`.egg-info` metadata |
| Perl | `perl(JSON::PP) = 4.16` provides and requires from `.pm` and Perl scripts |
| Desktop/AppStream | `application(...)`, `mimehandler(...)` and `metainfo(...)` provides |
| Debuginfo | `debuginfo(build-id) = ...` from `/usr/lib/debug/.build-id` |

The Python generator evaluates common `Requires-Dist` markers (`python_version`,
`sys_platform`, `platform_machine`, `extra`) and keeps unmarked dependencies.
It emits version-specific `pythonX.Ydist(...)` names; there is no
`python3dist(...)` major alias and no extras subpackage; a distro that wants
one declares it on its runtime package. The Perl reader understands `package`,
`$VERSION`, `use`/`require`/`no` and POD, but not `eval`-generated modules. It
does not invent a Perl runtime package or ABI capability; a recipe or profile
must declare those if the distro has them. A `#!/usr/bin/env name` shebang
requires `/usr/bin/env`, not `name`, because rpmx does not rewrite shebangs.

Each output can turn a generator off or drop a generated entry by exact match:

```yaml
outputs:
  - name: libcurl
    files:
      include: ["/usr/lib64/**"]
    generators:
      pkgconfig: false
      exclude:
        - libcurl.so.4()(64bit)
```

The keys are `elf`, `pkgconfig`, `script`, `python`, `perl`, `desktop`,
`metainfo`, `debuginfo` and `bundled`; all default to `true`. `elf` also
controls the `.so` symlink requirement.

`perl` also accepts a mapping, because the package that ships the interpreter
needs one side and not the other: its `perl(Module)` provides describe what it
contains, while the requires its own files record are either satisfied by those
provides or name modules no distribution carries (other systems' helpers such
as `Win32`, `VMS` and `Mac`, optional back ends, and imports only the test
suite makes). Publishing those makes the interpreter's own metadata
unresolvable, so a Perl package keeps the provides and drops the requires:

```yaml
outputs:
  - name: perl-5.42
    generators:
      perl:
        requires: false
```

#### 5.3.2 Bundled components

`bundled(...)` provides describe code that is vendored into the package. They
are discovered from vendor metadata, not from the payload:

| Evidence | Generated |
|---|---|
| `vendor/modules.txt` | `bundled(golang(<module>)) = <version>` |
| `Cargo.lock` with a `vendor/` directory | `bundled(crate(<crate>)) = <version>` |
| `package-lock.json` with `node_modules/` | `bundled(nodejs-<module>) = <version>` |
| `yarn.lock` (classic or Berry) with an installed tree or cache | `bundled(nodejs-<module>) = <version>` |
| `pnpm-lock.yaml` with `node_modules/` | `bundled(nodejs-<module>) = <version>` |
| Python `.dist-info`/`.egg-info` metadata, or `vendored.txt` | `bundled(python3dist(<name>)) = <version>` |
| Ruby `Gemfile.lock` with `vendor/bundle`, or a vendored gemspec | `bundled(rubygem-<name>) = <version>` |
| PHP `composer.lock` with `vendor/`, or `vendor/composer/installed.json` | `bundled(php-composer(<vendor>/<name>)) = <version>` |

Java, Conan and vcpkg catalogers are planned; their evidence is described in
the contributor document.

A lockfile without its vendor tree is only a declaration, not evidence that the
code is bundled, so it does not produce a provide. Node catalogers follow the
production dependency closure; a lock entry that is only reachable from
`devDependencies` is not reported. Yarn derives that closure from the root and
workspace `package.json` files and lock descriptors, pnpm from all importers
and snapshots, and npm from its lockfile flags.

Python and Ruby do not record development groups in the metadata we read.
They rely on the installed/vendored tree, so a vendor tree that still contains
development packages reports them. Prune the tree before packaging when that
matters.

C and C++ have no reliable vendor manifest. Declare them explicitly when the
version is known in the recipe:

```yaml
outputs:
  - name: libfoo
    bundled:
      - name: zlib
        version: "1.3.1"
        license: Zlib
      - name: libpng
        version: "1.6.43"
```

When the version is computed during the build, use the `bundled/declare`
step:

```yaml
build:
  pipeline:
    - uses: bundled/declare
      with:
        output: libfoo
        name: zlib
        version-file: third_party/zlib/VERSION
```

The step accepts `version` or `version-file`, not both. It writes the same
declaration file the engine reads. The engine creates the per-output directory
and exports `RPMX_BUNDLED_DIR`, so a step that computes the version itself can
write the file directly:

```yaml
build:
  pipeline:
    - name: declare-bundled-zlib
      runs: |
        version=$(sed -n 's/^#define ZLIB_VERSION "\(.*\)"/\1/p' \
          third_party/zlib/zlib.h)
        printf 'zlib=%s\n' "$version" \
          > "$RPMX_BUNDLED_DIR/libfoo/zlib.list"
```

Each line is `name` or `name=version`; blank lines and `#` comments are
ignored. A manually declared generic component may have only one version per
output, and a version that conflicts with a discovered component is an error.

The full policy, including output attribution and conflict handling, is in
[`../contributing/19-bundled-provides.md`](../contributing/19-bundled-provides.md).

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

### 5.5 Context

`context:` copies a local directory into the source workspace before patches
and source assembly:

```yaml
context: files
```

The path is relative to the root recipe directory and must be a subdirectory. The
contents land at the workspace root, matching melange's `--source-dir`, so a
file at `files/os-release` is available as `os-release` inside the build.

- One tree digest covers every path, mode and file content. It is recorded in
  `package.lock.json`; changing any file makes `--locked` fail.
- The copied tree becomes part of the deterministic source snapshot and the
  SRPM, so `rpmx rebuild` does not need the original directory.
- Context entries may be regular files, directories or relative symlinks.
  Absolute symlinks and symlinks containing `..` are rejected.
- A context entry that collides with an extracted upstream source is an error.
- Use `context:` for static package-local files. Use `source-assembly:` for
  files generated from the package version or other build data.

### 5.6 Source assembly

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

### 5.7 Build pipeline

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

Common fields on any entry: `name`, `cwd`, `env`, `timeout` (a positive whole-number duration with `s`, `m`, or `h` suffix,
for example `1s`, `30m`, or `4h`), `reason`.

`runs` is executed as `sh -c <script>` in the sandbox. It is not `-euo
pipefail`; add `set -eu` when you need it. `argv` never goes through a shell.

Built-in steps can be listed with:

```console
rpmx steps list
rpmx steps show cmake/configure
```

The standard build systems have individual reusable steps:

| Build system | Steps |
| --- | --- |
| autotools | `autotools/configure`, then the `make` steps |
| Make | `make/build`, `make/check`, `make/install` |
| CMake | `cmake/configure`, `cmake/build`, `cmake/check`, `cmake/install` |
| Meson | `meson/setup`, `meson/compile`, `meson/test`, `meson/install` |
| Python | `python/build-wheel`, `python/install-wheel` |

The Python steps support a build interpreter and a different installation
interpreter. This is useful for pure Python wheels in matrix builds:

```yaml
build:
  pipeline:
    - uses: python/build-wheel
      with:
        python: python3.14
        build-package: python-3.14-build
    - uses: python/install-wheel
      with:
        python: python${matrix.python.version}
        pip-python: python3.14
        pip-package: python-3.14-pip
        script-version: ${matrix.python.version}
```

`build-package` selects the build frontend package for the chosen interpreter.
The step installs only the build
frontend. Declare the project's backend dependencies in `build.packages`.
The frontend reads the backend and its paths from `pyproject.toml`. Bootstrap
recipes can use a distribution's frontend without a dependency on the package
under construction. For example, Fedora seeds must explicitly select
`build-package: python3-build` and `pip-package: python3-pip`.
The default requirements use Eternal package names.

`frontend` defaults to `build`. Set `frontend: pip` to use upstream pip's
wheel builder during a bootstrap. Select the matching tool package with
`build-package`; for example, use `python-3.14-pip` for an Eternal host frontend.
The pip frontend disables package indexes, dependency resolution and build
isolation. Declare its backend dependencies in the recipe.

Both wheel steps accept an optional `pip-zipapp` path. This selects a standalone
pip application instead of an installed pip module. For the build step, also
set `frontend: pip`. Select the package that owns the application through
`build-package` or `pip-package`. Bootstrap profiles and recipes supply seed
packages; rpmx does not download an implicit seed or choose a bootstrap stage.

`pip-package` selects the pip package. `pip-python` selects the interpreter
that runs pip. If omitted, it uses the target `python` interpreter. pip receives
the target through `--python`; it installs without dependency resolution,
network access, build isolation or bytecode compilation.
It ignores installed distributions so staging cannot uninstall bootstrap tools.

`script-version` is optional. When supplied, the step appends `-<version>` to
commands that do not already end with that Python version. The step rejects
command collisions. It removes top-level wheel `RECORD` files and sets
`INSTALLER` to `rpm`, since RPM owns the installed file inventory. Private
vendor metadata remains intact.

A step is named after the command it runs, so a command has exactly one step.
`autotools/configure` only configures; the Makefile it writes is built,
checked and installed with `make/build`, `make/check` and `make/install`, which
are the same steps a plain Makefile project uses. CMake and Meson still drive
their own build and install, so they keep their own steps.

Prefer the individual steps. They get their own progress line, can be skipped
or reordered, and take their own `with:` inputs. The single-name `autotools`,
`make`, `cmake`, `meson` and `python` steps are convenience wrappers that run
their fixed sequence in one shell step; use them only when that exact sequence
is what you want. `strip` is a step. `go/build`, `go/test`, `go/vendor`,
`go/bump`, `cargo/vendor`, `npm/ci`, `python/build-wheel` and
`python/install-wheel` are also available. `license/copy` copies one or more
license files into `$STAGE$PREFIX/share/licenses/<output>/` for each output, so
a split package does not duplicate the loop or accidentally share one path.
`test/hardening-check` checks the staged ELF files for PIE, NX, RELRO,
BIND_NOW, stack protector, fortify, CFI/branch protection and build IDs.
`test/runtime-linkage` checks installed shared libraries with the installed ELF
loader. Its required `libraries` input lists SONAME basename patterns in the
profile's library directory. Each pattern must match an ELF library whose
shared dependencies resolve. See [runtime library linkage](testing.md#runtime-library-linkage)
for an output-scoped example.

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
  cwd: "${source.dir}"
  argv: [install, -D, -m, "0644", "${inputs.source}", "${stage}/etc/acme.conf"]
needs:
  packages: [coreutils]
```

Use it as `uses: acme/install-config`. The step's `needs.packages` also joins the
root requirement. Local steps are hashed into the lock and embedded in the
SRPM.

### 5.8 Outputs and files

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

- The **first output is the main package**. It may omit `files.include`; it
  receives every staged path that no later output claims.
- A path claimed by two outputs is an error (`RPMX-OUTPUT-011`).
- An output after the main package whose selectors match no staged files is an
  error (`RPMX-OUTPUT-012`).
- `metapackage: true` allows an output with no files.
- A subpackage can use `@dev`, `@doc`, `@man`, `@info`, `@locales`, `@static`,
  `@pkgconfig`, `@headers` or `@debug` in `files.include` instead of spelling
  out every path. Presets expand during planning.
- `scripts` are raw shell and remain the escape hatch. Typed actions are not yet
  implemented.

`scriptlets` phases: `pre`, `post`, `preun`, `postun`; `interpreter` defaults to
`/bin/sh`.

#### Dependencies every output shares

A `Requires` that every subpackage needs is a property of the distribution,
not of one artifact. Declare it once at the recipe level and every output
inherits it:

```yaml
dependencies:
  run: ["filesystem"]
outputs:
  - name: hello
    files:
      include: ["/usr/bin/hello"]
  - name: hello-libs
    files:
      include: ["/usr/lib64/**"]
```

Both outputs require `filesystem`. An output that should not carry an
inherited entry drops it by exact match:

```yaml
  - name: hello-debuginfo
    metapackage: false
    dependencies:
      exclude: ["filesystem"]
```

Rules:

- Only `run` is inherited. `provides`, `obsoletes`, `conflicts`, `recommends`
  and the other weak relations stay per-output claims; the schema rejects them
  at recipe level.
- The output's own `dependencies.run` is appended, and duplicates collapse, so
  a subpackage can add to the base without restating it.
- `exclude` removes an entry from the merged list, inherited or declared.
- Values are templated per cell, like every other dependency list.
- The merged list is what the planning output, the RPM header and the SBOM
  show, so nothing is hidden.
- [`rpmx-defaults.yaml`](#repository-defaults) can supply the base for a whole
  repository; its entries come first and a recipe adds to them.

#### Debug information

Debug information is opt-in per recipe. Add the `debuginfo` step (it replaces
`strip`), compile with `-g`, and declare an output that claims the debug tree:

```yaml
build:
  environment:
    CFLAGS: "-g"
  pipeline:
    - uses: make
    - uses: debuginfo
outputs:
  - name: hello
    files:
      include: ["/usr/bin/hello"]
  - name: hello-debuginfo
    summary: Debug information for hello
    files:
      include: ["/usr/lib/debug/**"]
```

The step writes `$PREFIX/lib/debug/<path>.debug`, strips the binary and adds
`.gnu_debuglink`. It fails when no staged ELF contains DWARF, which is almost
always a missing `-g`, so an opt-in cannot silently produce an empty package.
Packages with no ELF (noarch, metapackages) omit both the step and the output.
The step uses `$OBJCOPY`/`$STRIP`, so it keeps working in a cross build; a
recipe's `CFLAGS` extend the cross flags rather than replacing them.

### 5.9 Tests

Recipe-level tests live at the top level and see every output:

```yaml
tests:
  - name: unit
    stage: build
    argv: [ctest, --output-on-failure]

  - name: integration
    stage: installed
    argv: [hello, --version]
```

Output-level tests live under an output and see that output plus the sibling
outputs that provide its runtime dependencies:

```yaml
outputs:
  - name: hello
    files:
      include: [/usr/bin/hello]
    dependencies:
      run: [hello-lib]
    tests:
      - name: smoke
        stage: installed
        packages: [grep]
        environment:
          HOME: /tmp
        script: "hello --version | grep -q hello"

  - name: hello-lib
    files:
      include: [/usr/lib64/libhello.so.1]
    tests:
      - name: exported
        stage: installed
        script: "test -e /usr/lib64/libhello.so.1"
```

- `stage: build` runs in the build sandbox, with the source workspace present.
- `stage: transaction` uses a private writable package root for real RPM transactions.
  See [package transactions](testing.md#package-transactions) for lifecycle tests and previous-version fixtures.
- `stage: installed` installs the built RPMs into a disposable test root and
  runs there.
- A recipe-level installed test installs every output, so it is an integration
  test. An output-level installed test installs that output and the sibling
  outputs needed by its RPM dependencies, not unrelated outputs. This catches a
  missing `Requires` between outputs instead of masking it with the full output
  set.
- The test root contains `bash`, `coreutils`, the declared test environment,
  and the external runtime requirements of the selected RPMs. Those
  requirements are resolved from the same repositories as the test root;
  capabilities provided by another selected output are installed from the
  build output instead. Tools or data that are not runtime requirements of the
  package go in that test's `packages`.
- `stage: package` is not implemented yet.
- Tests run with `rpmx build`, `rpmx rebuild` and `rpmx test`; `--no-tests`
  skips them on a build.
- `--test NAME` selects every test with that name; within one matrix cell the
  bare name must be unique. Use `--test OUTPUT/NAME` when several outputs
  declare the same test name. Test names must be unique within one `tests:`
  block; the same name may be reused by different outputs or matrix cells. A
  test name cannot contain `/`.
- `--junit FILE` writes JUnit XML.

#### Hardening check

A package can assert that its staged ELF files carry the profile's hardening:

```yaml
tests:
  - name: hardening
    stage: build
    pipeline:
      - uses: test/hardening-check
```

The step walks all of `$STAGE` and fails if an executable or shared object is
missing PIE, NX, RELRO, BIND_NOW, stack-protector, fortified libc, the target's
CFI/branch-protection property, or a build ID. Narrow it with `paths` and opt
out of individual checks with `skip`:

```yaml
      - uses: test/hardening-check
        with:
          paths: [/usr/bin/hello]
          skip: [fortify, cf-protection]
```

Use this for packages whose output should carry the full profile. It is not a
distro-wide gate: packages with hand-written assembly, static binaries or
language runtimes may legitimately need a skip, and a noarch package has no ELF
to check.

`test/compiler-hardening-check` is the companion for a package that supplies a
compiler wrapper or spec. It compiles an executable and a shared object through
the configured `cc`, then checks the resulting ELF properties:

```yaml
tests:
  - name: compiler
    stage: installed
    packages: [gcc-13]
    pipeline:
      - uses: test/compiler-hardening-check
        with:
          cc: gcc-13
```

`cc` is required. Declare its compiler package in the test's `packages` list.
The step does not add a default compiler that could conflict with the selected stream.

The shared-object step matters: a wrapper that appends `-pie` to every link
will fail a `-shared` link, while a wrapper that reads the link mode from a GCC
spec can add `-pie` only to executables.
- `tests[].packages` lists tools for the test. Build-stage packages join the
  build root, including the host root in cross builds. Installed-stage packages
  join only the disposable test root. Reusable test steps add their declared
  tool requirements. Tests with the same output and package set share a root.
- `tests[].environment` sets variables for every step of that test. A step's
  own `env` entries win.
- Real dependency checking is performed; the only retry is for a conflict with
  a package the test root already has, which the package under test replaces.
- A build writes one test-root lock per distinct environment:
  `<name>-<version>-<release>.test-root.lock.json` for the first,
  `.<n>.test-root.lock.json` for the others. `--save-test-root` and
  `--test-root-lock` require a single environment; pass `--test NAME` or
  `--test OUTPUT/NAME` to select one.

### 5.10 Matrix and variables

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

## Complete example

```yaml
schema: 1

package:
  name: hello
  version: "0.1.0"
  release: 1
  summary: A tiny hello package
  license: MIT
  maintainers:
    - name: Your Name
      email: you@example.org

build:
  root:
    profile: fedora-44
  packages: [gcc, make, bash]
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
