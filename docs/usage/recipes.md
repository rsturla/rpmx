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
  dependencies:
    tools: [gcc, make, sh]
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
```

rpmx looks in the recipe directory first, then each ancestor up to the
repository root (the first directory containing `.git`). The recipe wins when
it sets the field itself. `build.environment` entries are added when the
recipe does not already set the same variable, so a recipe can still override
one key without restating the rest. The defaults file joins the recipe bundle,
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

- `build.dependencies.tools`
- `build.dependencies.host`
- `build.dependencies.target`
- `build.dependencies.tests`
- every `needs.tools` from every resolved step (source-assembly, build, tests)

`tools` entries are **package names**, not command names. Use `bash` rather
than `sh`, `gcc` rather than `cc`.

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
  dependencies:
    tools: [gcc, make, bash, pkgconf-pkg-config]
    host: [zlib-devel, openssl-devel]
    target: []
    tests: [python3-pytest, jq]
  environment:
    CFLAGS: "-O2 -g"
```

For a native build every scope is installed into the same root. In a cross
build (a profile with a `cross` block) `tools` is installed in the host build
root and `host`/`target` in the target sysroot; see
[`cross-compilation.md`](cross-compilation.md). `tests` is used when building
the installed-stage test root. `environment` is merged into every build step's
environment; a step's own `env` wins.

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

Common fields on any entry: `name`, `cwd`, `env`, `timeout` (stored but not yet
enforced per step; use `--step-timeout`), `reason`.

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
| autotools | `autotools/configure`, `autotools/build`, `autotools/check`, `autotools/install` |
| Make | `make/build`, `make/check`, `make/install` |
| CMake | `cmake/configure`, `cmake/build`, `cmake/check`, `cmake/install` |
| Meson | `meson/setup`, `meson/compile`, `meson/test`, `meson/install` |

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
  tools: [coreutils]
```

Use it as `uses: acme/install-config`. The step's `needs.tools` also joins the
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
        environment:
          contents:
            packages: [grep]
          environment:
            HOME: /tmp
        argv: [sh, -c, "hello --version | grep -q hello"]

  - name: hello-lib
    files:
      include: [/usr/lib64/libhello.so.1]
    tests:
      - name: exported
        stage: installed
        argv: [sh, -c, "test -e /usr/lib64/libhello.so.1"]
```

- `stage: build` runs in the build sandbox, with the source workspace present.
- `stage: installed` installs the built RPMs into a disposable test root and
  runs there.
- A recipe-level installed test installs every output, so it is an integration
  test. An output-level installed test installs that output and the sibling
  outputs needed by its RPM dependencies, not unrelated outputs. This catches a
  missing `Requires` between outputs instead of masking it with the full output
  set.
- The test root still contains `bash`, `coreutils`, and the resolved test
  environment; runtime dependencies from outside the recipe must be listed in
  the test environment or supplied by the root.
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
    pipeline:
      - uses: test/compiler-hardening-check
        with:
          cc: gcc-13
```

The shared-object step matters: a wrapper that appends `-pie` to every link
will fail a `-shared` link, while a wrapper that reads the link mode from a GCC
spec can add `-pie` only to executables.
- `test.environment.contents.packages` lists packages installed only for that
  test's root. This mirrors Wolfi/melange test environments. Tests with the
  same output and package set share a root; a different set gets a separate
  root. It is valid only for `stage: installed`; build-stage tests use
  `build.dependencies.tools`.
- `build.dependencies.tests` remains the package-wide list of test-root
  packages, applied to every installed test.
- `test.environment.environment` sets variables for every step of that test. A
  step's own `env` entries win.
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
