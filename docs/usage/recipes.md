# Recipes

## Recipe reference

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

## Complete example

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
