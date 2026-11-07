# The design: `rpmx` and `package.yaml`

Working names only. The design does not depend on them.

## 1. Scope

`rpmx` is a packaging frontend that produces ordinary RPM packages. It is not:

- a replacement for CMake, Meson, Autotools, Cargo, Go or npm;
- a replacement for dnf/libsolv or the RPM database;
- an image composer (use Yocto, Buildroot, kiwi, or bootc for that);
- a general-purpose programming environment;
- compatible with existing spec files.

It *is*:

- a typed, declarative recipe format for general-purpose Linux packages;
- a deterministic planner that turns a recipe plus policy plus a lock into a build plan;
- a sandboxed executor with explicit inputs and no network during the build;
- a producer of standard RPM v4/v6 packages, SRPMs, SBOMs and provenance;
- a migration path from spec files, not a flag day.

### Assumptions

- The target is a Linux distribution with RPM-based package management.
- Builds run as an unprivileged user in a sandbox (bubblewrap, rootless container, or equivalent).
- Distribution policy is supplied by a profile, not hard-coded in the recipe.
- Native builds only in V1. Cross builds keep the `tools`/`host`/`target` split in the model but are not supported until the pipeline, root and sysroot contracts are stable.
- Bit-identical reproducibility is required only for the same lock, profile and toolchain.

---

## 2. Architecture

```
                    +-------------------+
                    |   package.yaml    |  recipe: intent
                    +---------+---------+
                              |
                    +---------v---------+
                    |  distro profile   |  policy: paths, flags, allowed actions
                    +---------+---------+
                              |
                    +---------v---------+
                    | package.lock.json |  exact sources, features, toolchain, digests
                    +---------+---------+
                              |
                    +---------v---------+
                    |   elaborator      |  pure, deterministic, no host introspection
                    +---------+---------+
                              |
             +----------------+----------------+
             |                                 |
    +--------v--------+              +---------v--------+
    |   build plan    |              |  output manifest |
    |  (step graph)   |              |  (file ownership)|
    +--------+--------+              +---------+--------+
             |                                 |
    +--------v---------------------------------v--------+
    |              fetch + source assembly             |
    |  verify -> unpack -> vendor/generate -> snapshot  |
    +------------------------+--------------------------+
                             |
                    +--------v---------+
                    |   SRPM assembly  |
                    |  + verification  |
                    +--------+---------+
                             |
    +------------------------v--------------------------+
    |              sandboxed build executor             |
    |  prep -> configure -> build -> check -> install -> |
    |  package -> test -> evidence                      |
    +------------------------+--------------------------+
                             |
              +--------------+--------------+
              |                             |
      +-------v-------+             +-------v--------+
      |  binary RPMs  |             | SBOM/provenance|
      +---------------+             +----------------+
```

The important line is between the **elaborator** and the **executor**. Everything above it is pure and testable. Everything below it is sandboxed and logged.

**SRPM-first:** every build passes through an SRPM. `rpmx build` resolves sources, assembles a deterministic source snapshot, generates the SRPM, verifies it, and only then runs the build. `rpmx rebuild` starts at SRPM verification. The executor never receives loose sources, and no build path bypasses the source artifact. Source assembly is a separate sandbox from the build; it may run vendoring/generator code, but it has no network after fetch and no credentials. The SRPM contents are defined in [`07-srpm-first.md`](07-srpm-first.md); source assembly rules are in [`06-build-inputs.md`](06-build-inputs.md#510-source-assembly-is-a-pipeline-stage).

---

## 3. File layout

```
package.yaml          # the recipe (human-authored)
package.lock.json     # the lock (generated, reviewed, committed)
patches/              # patch files
files/                # auxiliary files: units, sysusers, tmpfiles, service files
tests/                # test scripts, fixtures
```

A package is a directory. There is no `%include`, no macro search path, and no ambient configuration that changes the meaning of the recipe. Profiles are passed explicitly:

```
rpmx build --profile fedora-44
```

The profile can be a local file, a file from a profile repository, or a built-in profile. Its digest is recorded in the lock.

---

## 4. The recipe

### 4.1 Top-level shape

```yaml
schema: 1

package:
  name: curl
  version: "8.11.1"
  release: 1
  epoch: 0
  summary: A command line tool for transferring data with URLs
  description: |
    curl is a command line tool and library for transferring data with URLs.
  license: curl
  upstream: https://curl.se/
  maintainers:
    - name: Jane Packager
      email: jane@example.org
  vcs:
    type: git
    url: https://github.com/curl/curl.git
    tag: curl-8_11_1

build:
  pipeline:
    - uses: cmake
      with:
        args:
          - -DBUILD_TESTING=ON

sources:
  - name: curl
    digest: "sha512:..."
    locators:
      - kind: https
        url: https://curl.se/download/curl-8.11.1.tar.xz

outputs:
  - name: curl
    summary: Command line tool for transferring data with URLs
    files:
      include:
        - /usr/bin/curl
        - /usr/share/man/man1/curl.1*
```

The top-level keys are closed: any unknown key is an error. `schema` is an integer and must match the tool's supported schema.

Schema changes are major-versioned: within `schema: 1`, unknown fields are errors and field meanings never change. A new major (`schema: 2`) ships with a deprecation window, `rpmx migrate` converts an old recipe, and archived SRPMs keep building with the major they declare. There is no "latest schema only" mode.

### 4.2 `package`

| Field | Type | Required | Notes |
|---|---|---|---|
| `name` | string | yes | RPM name; validated against RPM's name rules. |
| `epoch` | integer | no | Default `0`; must be non-negative. |
| `version` | string | yes | RPM version; validated against RPM's version rules. |
| `release` | integer or mapping | yes | See below. |
| `summary` | string | yes | One line, bounded length. |
| `description` | string | yes | Multi-line. |
| `license` | string | yes | SPDX expression. Validated against the SPDX list or a profile allowlist. |
| `upstream` | URL | no | Upstream homepage. |
| `source-license` | string | no | SPDX expression if sources differ from the binary. |
| `maintainers` | list of mappings | yes | `{name, email}`, at least one. |
| `vcs` | mapping | no | `{type, url, tag?, commit?}`. |
| `cpe` | mapping | no | `{vendor, product, part?, edition?, language?, sw-edition?, target-sw?, target-hw?, other?}`; rendered as CPE 2.3 in SBOMs. |
| `bug-url` | URL | no | Bug tracker. |
| `architectures` | mapping | no | `{allow: [...], deny: [...]}`; default all. |
| `unowned-files` | enum | no | `error` (default), `warn`, `ignore`. |
| `ignore-files` | list of globs | no | Staging paths intentionally not packaged. |
| `metapackage` | boolean | no | Allows an output with no files. |

**Release.** `release: 1` is explicit and reproducible. The alternative is:

```yaml
package:
  release:
    auto: git
    base: 1
```

`rpmx lock` resolves this to an integer using a documented, deterministic rule (commit count since the last version tag, plus `base`) and writes the result to the lock. `rpmx build --locked` uses the locked integer. This gives the convenience of rpmautospec without making the build depend on mutable git state.

Release also carries distribution identity, which is policy rather than recipe data. A profile sets `dist-tag: .fc44`; the planner appends it after any matrix suffix. The build system may supply a counter with `--build-release N`, rendered as `+buildN` after the dist tag, matching Fedora's `%buildrelease` convention. Both inputs and the final release are recorded in `package.lock.json`, so `rpmx rebuild` reproduces the same NEVR and rejects conflicting flags.

**Changelog.** If `package.changelog.generate: git`, the lock records the generated entries. The RPM `%changelog` is emitted from the lock, not recomputed at build time. Explicit `changelog` list entries are also allowed for packages that are not built from git.

### 4.3 Sources

```yaml
sources:
  - name: curl
    digest: "sha512:8a8b..."
    size: 1234567
    signature:
      type: openpgp
      url: https://curl.se/download/curl-8.11.1.tar.xz.asc
      fingerprint: "27EDEAF22F3ABCE1..."
    locators:
      - kind: https
        url: https://curl.se/download/curl-8.11.1.tar.xz
      - kind: https
        url: https://mirror.example.org/curl/curl-8.11.1.tar.xz
      - kind: oci
        ref: ghcr.io/example/sources/curl:8.11.1
    extract:
      format: tar.xz
      strip-components: 1
      destination: .

  - name: curl-tests
    digest: "sha512:..."
    locators:
      - kind: https
        url: https://example.org/curl-tests-8.11.1.tar.xz
    extract:
      format: tar.xz
      destination: tests

  - name: vendor
    type: git
    url: https://github.com/example/vendor.git
    commit: "0123456789abcdef0123456789abcdef01234567"
    destination: vendor
```

Source rules:

- `name` is unique and is referenced by `build.source`.
- `digest` is the source identity. It is required for archives and files. A `git` source requires a full `commit`; `tag` is informational and must resolve to the commit in the lock.
- `locators` is an ordered list of where the bytes might be. Any locator that yields the digest is acceptable. A single `url` plus `digest` is shorthand for one `https` locator; `mirrors` is sugar for additional `https` locators.
- `size` is optional but recommended; it lets the fetcher reject absurd responses early.
- `signature` is optional in the schema but can be required by policy. The lock records the verification result.
- `extract.format` is `tar.xz`, `tar.gz`, `tar.zst`, `zip`, `none`, or `auto`.
- `strip-components` defaults to `0`; `destination` defaults to `.`.
- Sources are fetched before the build; the build sandbox has no network.
- The lock records the digest, size, signature result, and the locator that succeeded. A rebuild may use any locator that yields the same digest.
- A native SRPM can be used as the only input for offline or archival builds. See [`07-srpm-first.md`](07-srpm-first.md).

**Context.** A recipe may declare one local context directory:

```yaml
context: files
```

Its contents are copied to the workspace root before patches and source
assembly. The path is relative to the root recipe directory. The lock records
one tree digest over paths, modes and contents. This is for package-local
static files; generated files belong in source assembly. It mirrors melange's
`--source-dir` without copying the whole recipe directory.

**Patches** are an ordered array:

```yaml
patches:
  - file: patches/0001-curl-fix-proxy.patch
    strip: 1
    when: [proxy]        # optional feature condition
    reason: Backport of upstream commit abc123
    resolves:
      - CVE-2024-12345
      - https://github.com/curl/curl/pull/1234
```

The array order is the application order. There is no `series` file. `reason` is required for local patches (not for patches fetched from an upstream source), because patch debt should be visible. `resolves` is optional metadata for SBOM/VEX.

### 4.4 Build

```yaml
build:
  dependencies:
    tools:
      - cmake
      - ninja-build
    host:
      - openssl-devel
      - zlib-devel
  environment:
    CURL_CA_BUNDLE: /etc/pki/tls/certs/ca-bundle.crt
  pipeline:
    - uses: cmake
      with:
        args:
          - -DBUILD_TESTING=ON
          - -DCURL_USE_OPENSSL=ON
          - -DCURL_USE_NGHTTP2=ON
    - uses: strip
```

**There is no `build.system`.** The pipeline is the build language, following melange. `uses: cmake` is a named bundle; it expands to `cmake/configure`, `cmake/build`, `cmake/check` and `cmake/install`, and `rpmx plan` prints the expanded commands.

Built-in bundles:

| Bundle | Expands to |
|---|---|
| `autotools` | `autoreconf` (optional), `configure`, `make`, `make check`, `make install` |
| `cmake` | `cmake -S . -B build`, `cmake --build`, `ctest`, `cmake --install` |
| `meson` | `meson setup`, `meson compile`, `meson test`, `meson install` |
| `make` | `make`, `make check`, `make install` |
| `cargo` | `cargo build --locked`, `cargo test --locked`, `cargo install --path . --root ...` |
| `go` | `go build`, `go test`, `go install` |
| `python` | PEP 517 wheel build and install with a declared interpreter |
| `npm` | `npm ci --offline`, `npm test`, `npm install --prefix ...` |

A bundle is itself a named step that composes other steps. A recipe that needs a different sequence lists individual steps or writes inline steps; there is no `custom` bundle.

The full contract — step fields, inputs, `needs`, bundles, lock entries and error codes — is in [`13-pipelines.md`](13-pipelines.md).

### 4.5 Dependencies

Build dependencies are scoped:

```yaml
build:
  dependencies:
    tools:
      - "cmake >= 3.20"
      - ninja
      - pkgconf
    host:
      - "openssl-devel >= 3.0"
      - zlib-devel
      - libnghttp2-devel
    target: []
    tests:
      - python3-pytest
    extra:
      - name: libidn2-devel
        scope: host
        when: [idn]
        reason: IDN support is optional
```

- `tools` run on the build machine.
- `host` are libraries/headers for the machine the package will run on (the sysroot in a cross build).
- `target` are packages needed for the target machine during a cross build but not linked into the output (rare; e.g., a target interpreter).
- `tests` are test-only dependencies; they are not in the consumer graph.

A dependency is either a shorthand string (`name [op version]`) or a table with `name`, `op`, `version`, `arch`, `when`, `qualifiers`.

Dependency operators: `=`, `!=`, `<`, `<=`, `>`, `>=`. Version strings use RPM's EVR comparison. Arch is optional and must be a valid RPM arch.

`qualifiers` maps to RPM's dependency qualifiers: `pre`, `post`, `preun`, `postun`, `pretrans`, `posttrans`, `verify`, `interp`, `meta`. `meta = true` is shorthand for `qualifiers = ["meta"]`.

**Runtime dependencies** are per output:

```yaml
outputs:
  - name: curl
    dependencies:
      run:
        - "libcurl = ${version}-${release}"
        - ca-certificates
      recommends:
        - curl-minimal
      suggests:
        - jq
      supplements:
        - curl-openssl
      enhances:
        - http-client
      conflicts:
        - "curl-full < 8.0"
      obsoletes:
        - "curl-old < 8.0"
      provides:
        - web-client
      order-with-requires:
        - filesystem
```

Dependency strings may use the closed template scope, typically `${package.version}`, `${package.release}` and matrix fields. They are resolved by the planner, not by a general macro expander; see [`12-templates.md`](12-templates.md).

`recommends`, `suggests`, `supplements` and `enhances` map to RPM's weak dependency tags. They are solver hints, not guarantees: rpm itself ignores them when installing packages directly, so installed-stage tests must list anything they need in `build.dependencies.tests`. Weak dependencies currently accept the same simple atoms as `run`; rich weak dependencies are not implemented yet.

**Rich dependencies.** Common cases are typed:

```yaml
outputs:
  - name: curl
    requires:
      - any_of:
          - curl-openssl
          - curl-gnutls
      - all_of:
          - "curl-libs = ${version}-${release}"
          - ca-certificates
```

For the long tail, a `rich` string is allowed and validated with RPM's parser:

```yaml
outputs:
  - name: curl
    requires:
      - rich: "(curl-openssl if openssl) else curl-gnutls"
```

`rich` is linted and discouraged. It is an escape hatch, not the primary interface.

**Automatic generation.** Per output:

```yaml
outputs:
  - name: libcurl
    generators:
      elf: true
      pkgconfig: true
      script: true
      python: false
      perl: false
      desktop: false
      metainfo: false
      debuginfo: false
      bundled: true
      exclude:
        - libcurl.so.4()(64bit)
```

Every generator is on by default. `exclude` removes a generated provide or
require by exact match. Generated dependencies are computed at package time
from the staged payload and merged with the output's declared dependencies;
they are not invisible macro behaviour, and an output can turn a generator off.

Bundled provides are a separate generator fed by the component inventory, not
by the staged payload alone. Go and Rust are discovered from vendor metadata;
C and C++ are declared explicitly with `outputs[].bundled` or a build-time
`.rpmx/bundled/<output>/*.list` file. See
[`19-bundled-provides.md`](19-bundled-provides.md).

### 4.6 Features

```yaml
features:
  default: [ssl, http2]
  ssl:
    summary: TLS support
    requires: [tls]
    conflicts: [gnutls]
    dependencies:
      host:
        - "openssl-devel >= 3.0"
    pipeline:
      - at: cmake/configure
        inputs:
          args:
            append:
              - -DCURL_USE_OPENSSL=ON
    provides:
      run:
        - curl-tls
  gnutls:
    summary: TLS support via GnuTLS
    requires: [tls]
    conflicts: [ssl]
    dependencies:
      host:
        - "gnutls-devel >= 3.8"
    pipeline:
      - at: cmake/configure
        inputs:
          args:
            append:
              - -DCURL_USE_GNUTLS=ON
  tls:
    summary: A TLS backend is selected
  http2:
    summary: HTTP/2 support
    dependencies:
      host:
        - libnghttp2-devel
    pipeline:
      - at: cmake/configure
        inputs:
          args:
            append:
              - -DCURL_USE_NGHTTP2=ON
  idn:
    summary: Internationalized domain names
    dependencies:
      host:
        - libidn2-devel
    pipeline:
      - at: cmake/configure
        inputs:
          args:
            append:
              - -DCURL_USE_LIBIDN2=ON
```

Rules:

- `default` lists features enabled unless the CLI/profile says otherwise.
- `requires` and `conflicts` refer to **features**, not packages.
- Feature dependencies, pipeline contributions and provides are merged into the plan when the feature is enabled.
- A pipeline contribution targets an expanded step by name and merges into its typed inputs: `set` for any input, `append`/`prepend` for `list<string>`. An unknown step, an unknown input or an incompatible merge is an error before any build step.
- Features are resolved to a fixed point; a cycle or unsatisfied constraint is an error before any build step.
- The enabled set is recorded in the lock and passed as `FEATURES`.
- A feature may add patches (via `patches[].when`), outputs (via `outputs[].when`), and actions (via `when`).
- There is no `%if`. A feature is either enabled or not; the planner never evaluates arbitrary expressions.

CLI:

```
rpmx build --features default
rpmx build --features +idn,-http2
rpmx plan --features all
rpmx features
```

`rpmx features` prints the matrix, constraints and defaults. `rpmx plan --features all` reports which combinations are invalid.

### 4.7 Outputs

```yaml
outputs:
  - name: curl
    summary: Command line tool for transferring data with URLs
    description: The curl command line tool.
    arch: any
    files:
      include:
        - /usr/bin/curl
        - /usr/share/man/man1/curl.1*
    dependencies:
      run:
        - "libcurl = ${version}-${release}"
        - ca-certificates

  - name: libcurl
    summary: Library for transferring data with URLs
    description: libcurl is a client-side URL transfer library.
    arch: any
    files:
      include:
        - /usr/lib/libcurl.so.4*
    dependencies:
      run:
        - ca-certificates
      provides:
        - libcurl.so.4()(64bit)

  - name: libcurl-devel
    summary: Development files for libcurl
    description: Headers and pkg-config files for libcurl.
    arch: any
    files:
      include:
        - /usr/include/curl/**
        - /usr/lib/libcurl.so
        - /usr/lib/pkgconfig/libcurl.pc
    dependencies:
      run:
        - "libcurl = ${version}-${release}"

  - name: curl-doc
    summary: Documentation for curl
    description: Documentation and examples for curl.
    arch: noarch
    files:
      include:
        - /usr/share/doc/curl/**
        - /usr/share/man/**
```

Output rules:

- Every output has a unique `name`. The set of outputs is explicit; there is no implicit `-libs`/`-devel` unless auto-split is enabled.
- Outputs inherit `version`, `release`, `epoch`, `license` and `maintainers` from `package`. They may override `summary`, `description`, `arch` and dependency sets.
- `arch: noarch` outputs must not contain ELF objects; the planner checks.
- An output with no files is an error unless `metapackage: true`.
- Output order is declaration order; it is used for manifest reporting and action ordering, not for file ownership precedence.

**File selectors.**

```yaml
outputs:
  - name: curl
    files:
      include:
        - /usr/bin/curl
        - /usr/share/man/man1/curl.1*
        - /usr/share/doc/curl/**
      exclude:
        - /usr/share/doc/curl/CHANGES
```

Glob semantics:

- Paths are absolute in the staging root.
- `*` matches within one path segment; `**` matches across segments; `?` matches one character; `[abc]` is a character class.
- No shell expansion, brace expansion, command substitution or variable expansion.
- Includes are collected; excludes remove. A path must match at least one include to be packaged.
- A path claimed by more than one output is an error.
- A staging path claimed by no output is an error unless it is in `ignore-files` or the output policy says otherwise.

**File attributes.**

```yaml
outputs:
  - name: curl
    files:
      attributes:
        - path: /etc/curlrc
          config: noreplace
          mode: "0644"
          owner: root
          group: root
        - path: /usr/bin/curl
          mode: "0755"
          caps: cap_net_raw=ep
        - path: /var/log/curl.log
          ghost: true
          mode: "0640"
          owner: curl
          group: curl
        - path: /usr/share/doc/curl/COPYING
          license: true
```

`path` may be an exact path or a glob. Exact paths win over globs; more specific globs win over less specific ones. Attributes:

| Attribute | Meaning |
|---|---|
| `config` | `false`, `"noreplace"`, `"missingok"`. |
| `doc` | Mark as documentation. |
| `license` | Mark as a license file. |
| `ghost` | Do not include contents; metadata only. |
| `artifact` | Mark as an RPM `%artifact`. |
| `missingok` | Absence is not a verification failure. |
| `dir` | Own the directory itself, not its contents. |
| `caps` | POSIX capabilities string. |
| `mode` | Octal string. |
| `owner`, `group` | Names; resolved at package build time from policy. |
| `verify` | List of attributes to ignore in `rpm -V`. |
| `when` | Feature condition. |

**Auto-split.** Optional:

```yaml
outputs:
  auto-split: [libs, devel, doc, locale]
```

Auto-split is a profile-driven policy with standard rules:

| Class | Typical paths |
|---|---|
| `libs` | `/usr/lib/*.so.*`, `/usr/lib/*.so.*.*` |
| `devel` | `/usr/include/**`, `/usr/lib/*.so`, `*.pc`, `*.cmake`, `*.h` |
| `doc` | `/usr/share/doc/**`, `/usr/share/man/**`, `/usr/share/info/**` |
| `locale` | `/usr/share/locale/**` |

**Debuginfo is opt-in.** It is not part of auto-split and is never generated implicitly. A package that wants it declares an output and a step:

```yaml
outputs:
  - name: foo-debuginfo
    files:
      include:
        - /usr/lib/debug/**
```

or uses `uses: debuginfo` in the pipeline, which scans the staged ELF files, writes `$PREFIX/lib/debug/<path>.debug`, adds `.gnu_debuglink` and strips the binary. This avoids depending on rpmbuild's debuginfo machinery, and it means the default package set is smaller. There is no distro-wide flag: the recipe opts in, as with melange's `split/debug`. Build-id links and source references are not generated yet.

If a package has explicit outputs with the same names, auto-split fills them rather than creating duplicates. `rpmx plan` shows the resulting manifest before the build. Auto-split never silently discards a file; if a rule is ambiguous, it is an error with the path and the candidate classes.

### 4.8 Actions

Actions are typed system integration. They compile to RPM scriptlets/triggers, but the recipe never contains scriptlet boilerplate.

```yaml
outputs:
  - name: curl
    actions:
      - type: systemd
        units:
          - curl.service
        enable:
          - curl.service
        restart-on-upgrade: true
      - type: sysusers
        file: files/curl.sysusers
      - type: tmpfiles
        file: files/curl.tmpfiles
      - type: ldconfig
      - type: alternatives
        name: curl
        link: /usr/bin/curl
        target: /usr/bin/curl-full
        priority: 100
      - type: desktop-database
      - type: icon-cache
        paths:
          - /usr/share/icons/hicolor
      - type: shell
        phase: post
        argv: ["/usr/bin/curl-system-integration", "--quiet"]
        reason: upstream requires a one-time cache migration
```

Supported action types (v1):

| Type | Purpose |
|---|---|
| `systemd` | Enable/disable/restart units; daemon-reload. |
| `sysusers` | Create users/groups from a `sysusers.d` file. |
| `tmpfiles` | Create tmpfiles entries from a `tmpfiles.d` file. |
| `ldconfig` | Update the dynamic linker cache. |
| `alternatives` | Register/remove an alternative. |
| `desktop-database` | Update the desktop entry database. |
| `icon-cache` | Update icon caches. |
| `mime-database` | Update the MIME database. |
| `font-cache` | Update the font cache. |
| `pam` | Install PAM configuration safely. |
| `selinux` | Label files / load policy module. |
| `modules-load` | Install a modules-load.d file. |
| `udev` | Install udev rules and reload. |
| `firewalld` | Install a firewalld service definition. |
| `trigger` | Package or file trigger with a typed script. |
| `shell` | Explicit escape hatch. |

Rules:

- Action order is deterministic: output declaration order, then action declaration order.
- The planner validates that required files exist (e.g., a systemd unit listed in `units` must be in the output's manifest or provided by a dependency).
- `shell` actions require a `reason` and are forbidden by profiles that set `forbid-shell-actions = true`.
- Actions are translated by a **profile-specific emitter**. `alternatives` on Fedora emits `update-alternatives`; on another distro it may emit a different implementation. This is exactly the kind of policy that should not live in the recipe.

### 4.9 Tests

```yaml
tests:
  - name: unit
    stage: build
    argv: [ctest, --output-on-failure, --test-dir, build]
  - name: smoke
    stage: installed
    argv: [curl, --version]
    dependencies:
      run:
        - ca-certificates
  - name: abi
    stage: installed
    kind: abi
    library: libcurl.so.4
    baseline: abi/libcurl-8.10.json
  - name: reproduce
    stage: package
    kind: reproduce
```

- `stage: build`: run inside the build sandbox after `build` (or after `install`, if declared).
- `stage: installed`: install the outputs into a disposable root, then run
  the command there. A recipe-level test installs every output; an output-level
  test installs its own output plus the sibling outputs its RPM dependencies
  require.
- `stage: package`: operate on the produced RPMs (reproducibility, manifest, signature checks).
- `kind: command` (default), `abi`, or `reproduce`.
- Test dependencies are scoped like build dependencies.
- A profile can require at least one `installed` test and can fail the build if a required test is missing or fails.
- `rpmx test` writes JUnit XML and JSON. `rpmx build` runs tests only if `--test` is passed; CI profiles set `require-tests = true`, which makes `rpmx build` fail if tests are not run.

### 4.10 Profiles

Profiles are versioned policy overlays. They are not recipes and cannot execute code.

```yaml
# profiles/fedora-44.yaml
schema: 1

profile:
  name: fedora-44
  vendor: Fedora Project
  dist-tag: .fc44

set:
  paths.prefix: /usr
  paths.libdir: /usr/lib64
  build.environment.CFLAGS: "-O2 -g -pipe -Wall"
  build.environment.LDFLAGS: "-Wl,-z,relro -Wl,-z,now"
  package.vendor: Fedora Project

append:
  build.dependencies.tools:
    - redhat-rpm-config
    - gcc
    - gcc-c++
  features.default:
    - ssl

remove:
  features.default:
    - http2

policy:
  allowed-actions:
    - systemd
    - sysusers
    - tmpfiles
    - ldconfig
    - alternatives
    - desktop-database
    - icon-cache
  forbid-shell-actions: true
  require-argv: true
  require-tests: true
  unowned-files: error
  config-files-under:
    - /etc
  license-files-required: true
```

Overlay semantics:

- `set` replaces a scalar or array at a dotted path.
- `append` appends to an array at a dotted path.
- `remove` removes matching scalar values from an array at a dotted path.
- `merge` merges a table at a dotted path (keys in the overlay win).
- The target path must exist in the recipe and have the expected type. A profile cannot create a new output in v1; use a feature for optional outputs, or a separate recipe.
- Paths are validated against the schema; a typo is an error, not a silent no-op.
- `rpmx plan --profile` shows a typed diff: what changed, why, and which values came from the profile.
- The profile digest is recorded in the lock.

This is deliberately less powerful than Yocto layers or Nix overlays. The goal is to make distro policy visible and auditable, not to create a second programming language. If a distro needs more, it should add a feature to the recipe or maintain a delta recipe.

### 4.11 Lockfile

```json
{
  "schema": 1,
  "recipe_digest": "sha512:...",
  "profile": "fedora-44",
  "profile_digest": "sha512:...",
  "features": ["ssl", "http2"],
  "epoch": 0,
  "version": "8.11.1",
  "release": 1,
  "source_date_epoch": 1735689600,
  "sources": [
    {
      "name": "curl",
      "digest": "sha512:...",
      "size": 1234567,
      "signature": {
        "type": "openpgp",
        "fingerprint": "...",
        "verified": true
      },
      "locator": {
        "kind": "https",
        "url": "https://curl.se/download/curl-8.11.1.tar.xz"
      }
    },
    {
      "name": "vendor",
      "type": "git",
      "url": "https://github.com/example/vendor.git",
      "commit": "0123456789abcdef0123456789abcdef01234567"
    }
  ],
  "dependencies": [
    {
      "scope": "host",
      "name": "openssl-devel",
      "version": "3.2.1-1.fc44",
      "source": "fedora-44"
    }
  ],
  "build": {
    "toolchain": {
      "cc": "gcc 14.2.1",
      "cmake": "3.31.0",
      "ninja": "1.12.1"
    },
    "root_digest": "sha512:..."
  },
  "steps": [
    {"name": "cmake/configure", "version": 1, "digest": "sha512:..."},
    {"name": "cmake/build", "version": 1, "digest": "sha512:..."},
    {"name": "cmake/check", "version": 1, "digest": "sha512:..."},
    {"name": "cmake/install", "version": 1, "digest": "sha512:..."}
  ],
  "outputs": {
    "curl": {
      "manifest_digest": "sha512:...",
      "file_count": 1
    },
    "libcurl": {
      "manifest_digest": "sha512:...",
      "file_count": 1
    }
  }
}
```

Lock rules:

- The lock is generated by `rpmx lock`; it is committed and reviewed.
- `rpmx lock --update` refreshes sources, dependencies, the root and steps, and shows a diff. It does not silently accept changes.
- `rpmx build --locked` verifies the recipe digest, profile digest, source hashes, resolved dependency NEVRAs, build-root digest and every step digest. Any drift is an error.
- The lock pins all three layers: exact package NEVRAs (readable, auditable), the rootfs digest (the actual toolchain contract), and step digests (the build logic). A change in any layer is a lock change.
- Dependency versions are recorded for audit; the build-root digest is the actual reproducibility contract. If a repository is not reproducible, pin a snapshot or an OCI image digest in the profile.
- `source_date_epoch` is derived from the source archive mtime, the git commit time, or an explicit `package.source-date-epoch`; it is locked.

### 4.12 SBOM and provenance

`rpmx` generates:

- **CycloneDX** and **SPDX** SBOMs from the lock, the manifest, and the ELF/package metadata.
- A **provenance statement** (in-toto/SLSA-style) containing the recipe digest, profile digest, source digests, builder identity, build log digest, output digests and step digests. In V1 it is **unsigned** and `rpmx` never holds a key. Signing and transparency-log publication are an external interface: an attestation is handed to a signer or a provenance service and returned as a signed envelope. Package signing is a separate, optional operation with a caller-supplied OpenPGP key; it does not sign attestations.
- A **reproducibility report** with the result of a second build in a different root.

The statement format and the signing interface are specified in V1 so that later services can consume them without a format change. Producing signed attestations is post-V1.

These are outputs of `rpmx build --evidence` and are attached to the package in the repository, not embedded in the RPM header (RPM has no standard SBOM tag). They can also be installed as `/usr/share/sbom/<name>.cdx.json` if policy requires it.

### 4.13 Package-set manifest for image composers

`rpmx` does not compose images. It should, however, emit a stable **package-set manifest** for tools that do: apko-like composers, OSBuild, bootc/rpm-ostree, or a distro's image builder.

```json
{
  "schema": 1,
  "packages": [
    {
      "name": "curl",
      "epoch": 0,
      "version": "8.11.1",
      "release": 1,
      "arch": "x86_64",
      "digest": "sha512:...",
      "file_count": 1,
      "sbom": "curl-8.11.1-1.x86_64.cdx.json"
    }
  ],
  "repositories": ["https://example.org/repo"],
  "keyring": ["https://example.org/keys/repo.pub"]
}
```

This is the analogue of apko's `contents` section, but generated from an actual build rather than hand-written. It gives image composers a declarative, lockable input and preserves the boundary that apko demonstrates: package build may run arbitrary code in a sandbox; image composition should not.

### 4.14 Source assembly

Source generation is a pipeline stage before the SRPM, not part of `%build`. It produces the deterministic source snapshot that goes into the SRPM:

```yaml
source-assembly:
  pipeline:
    - uses: go/vendor
    - uses: cargo/vendor
    - argv: ["python3", "generate-parser.py"]
      reason: Parser tables are deterministic and belong in the source snapshot
```

Rules:

- Runs in a separate sandbox from the build; no network after fetch; no credentials.
- The recipe names built-in steps or inline commands; the builder image supplies the tools, and the lock records their resolved digests.
- Output is deterministic and content-addressed; the snapshot digest goes into the lock and manifest.
- Vendoring (`go/vendor`, `cargo/vendor`, `npm/ci`) and deterministic generated code belong here.
- Target-specific generation and code that must run on the target belongs in the build, with pinned tools and no network.
- A generator change is a source change and requires review.

`rpmx source` stops after this stage; `rpmx srpm` consumes the result. The full rules are in [`06-build-inputs.md`](06-build-inputs.md#510-source-assembly-is-a-pipeline-stage).

### 4.15 Matrices and templates

Two related mechanisms expand one recipe into many concrete builds:

- **Matrices** turn typed axes into build cells. Each cell has its own resolved dependencies, build root, options, outputs and SRPM. Cells can be excluded and are bounded by `max-cells`. Melange's `data`/`range` expansion is the closest prior art; it expands subpackages, not build cells.
- **Templates** substitute a closed set of variables (`package.*`, `profile.*`, `cell.*`, `matrix.*`, `vars.*`) into package identity, outputs, dependencies, options, files and tests. There are no expressions and no conditionals; expansion is a fixed point with cycle detection.

Both are resolved by the planner before any fetch or build, and both are recorded in the lock and the manifest. See [`11-matrices.md`](11-matrices.md) and [`12-templates.md`](12-templates.md).

### 4.16 Build root

`rpmx` owns build-root resolution. The profile declares repositories, keys and the package set; the resolver produces exact NEVRAs; the assembled root is hashed into `build.root_digest`.

- Resolution uses RPM semantics (libsolv or an equivalent solver), not a hand-rolled dependency walk.
- The root can be assembled from a package set or consumed as a builder OCI image. Both are the same root; the image is a distribution and cache form, pinned by digest.
- The root is read-only at build time. Only `/build`, `/stage` and `/tmp` are writable.
- Offline builds use the locked root digest from the local CAS or a pinned image; they never resolve against a live repository.
- Delegating root construction to mock/koji is not part of V1, because it hides the dependency closure the lock is supposed to record.

The lock pins the readable NEVRAs and the root digest; `rpmx plan` shows the resolved package set and any root change.

---

## 5. Semantics

### 5.1 Elaboration

`rpmx plan` runs the pure part:

1. Parse YAML strictly (YAML 1.2, no anchors/aliases/merge keys, no duplicate keys).
2. Validate the schema.
3. Load and validate the profile.
4. Apply the profile overlay.
5. Resolve features to a fixed point.
6. Resolve sources and patches; verify the lock.
7. Resolve build dependencies and generators.
8. Resolve the named steps, bundles and `needs` into an expanded step list.
9. Compute file selectors and predicted ownership.
10. Emit a plan.

`rpmx build` adds:

11. Fetch sources into the local CAS.
12. Assemble the deterministic source snapshot in a separate source-assembly sandbox.
13. Assemble the native SRPM from the snapshot, recipe, lock, steps and patches.
14. Verify the SRPM manifest and digests, and its signature if policy requires one.
15. Extract the verified SRPM into the build sandbox.
16. Execute the step graph.
17. Scan the staging tree.
18. Assign every staged file to an output.
19. Enforce totality and attributes.
20. Generate actions and the RPM spec IR.
21. Assemble binary RPMs.
22. Run tests.
23. Emit evidence.

`rpmx rebuild FILE` starts at step 14 with an existing SRPM. A normal build produces both the source and binary artifacts in one run.

The plan is deterministic: same recipe, profile, features and lock produce the same plan, byte for byte (modulo documented ordering). There is no host introspection at plan time.

### 5.2 Feature resolution

Features are a boolean constraint problem:

- Start with `default` plus CLI/profile additions/removals.
- Apply `requires` and `conflicts` transitively.
- If a feature is required but disabled, enable it and record why.
- If a feature is both required and conflicted, fail.
- If a feature is unknown, fail.
- If `--features all` produces an invalid combination, list the minimal conflicting sets.

The result is a set, not an order. Pipeline contributions from enabled features are merged into the named steps; two features that `set` the same input to different values are an error (unless the feature declares a precedence, which v1 does not support). This is stricter than RPM's last-macro-wins, and that is the point.

### 5.3 Dependency resolution

The planner produces a dependency graph with provenance:

- direct recipe dependency,
- feature dependency,
- profile dependency,
- generated dependency (ELF, pkgconfig, etc.),
- implicit dependency (e.g., `/bin/sh` for a scriptlet).

`rpmx explain requires <output>` prints each edge with its source. The lock records the resolved versions.

Build dependency resolution is a separate problem from runtime dependency resolution. The planner does not try to be a full SAT solver for the distro; it asks the configured repository/solver for a build root that satisfies the constraints, then records the root digest. This is the same division of labour as mock/koji, but with an explicit contract.

### 5.4 File manifest

The manifest is the heart of the design.

- The staging tree is scanned after `install`.
- Every path is classified: regular file, directory, symlink, hardlink, device, socket, FIFO.
- Every path must be claimed by exactly one output, or explicitly ignored.
- File attributes are applied in a deterministic order: default policy, output selector, exact path override, profile override.
- The manifest records path, type, mode, owner, group, size, digest, symlink target, capabilities, and RPM flags.
- The manifest digest goes into the lock and the provenance attestation.
- `rpmx diff old.rpm new.rpm --files` compares manifests; `rpmx diff old/ new/` compares recipes.

This makes `%files` glob surprises impossible: a glob that matches nothing is an error, a duplicate claim is an error, and an unowned file is an error (by default).

### 5.5 Actions

Actions are compiled to a typed intermediate representation, then to RPM scriptlets by the profile emitter. The IR records:

- action type,
- output,
- phase(s),
- files it depends on,
- script/argv,
- ordering constraints.

The planner checks that actions do not reference files outside the output (except files provided by dependencies) and that systemd units/sysusers/tmpfiles files exist. The emitter is deterministic and quote-safe.

### 5.6 Tests and evidence

Tests are part of the plan, not an afterthought. `rpmx plan` shows which tests will run and which artifacts they need. `rpmx build --test` runs them; a profile can make that mandatory.

Evidence is generated from the actual build:

- SBOM components come from the lock and the manifest.
- File hashes come from the payload.
- Provenance includes the plan digest and the executor log digest.
- Reproducibility compares two builds and reports differences with `diffoscope`-style detail.

---

## 6. CLI

```
rpmx init [name]                 # create a recipe skeleton
rpmx fmt                         # canonical formatting
rpmx lint                        # schema + policy + style
rpmx lock [--update]             # resolve and write package.lock.json
rpmx plan [--profile P] [--features ...] [--json]
rpmx fetch [--profile P] [--locked]       # populate local CAS
rpmx source [--profile P] [--locked]      # deterministic source snapshot
rpmx srpm [--profile P] [--locked] [--output DIR]
rpmx build [--profile P] [--locked] [--target TRIPLE] [--test] [--evidence]
rpmx build <recipe> [--root-lock LOCK --root DIR] [--output DIR]   # full pipeline
rpmx build <recipe> --stage DIR [--cell ID] [--output DIR]         # package an existing stage
rpmx build --from-srpm FILE
rpmx rebuild FILE                 # alias for build --from-srpm
rpmx keygen [--output KEY.asc]    # generate an OpenPGP key pair
rpmx sign --signing-key KEY FILE  # sign existing RPMs in place
rpmx repo --rpms DIR --output DIR # write and optionally sign repository metadata
rpmx inspect-srpm FILE
rpmx verify-srpm FILE
rpmx test [--install] [--reproduce] [--abi]
rpmx explain <topic>             # requires, files, features, actions, provenance
rpmx graph [--format dot|json]   # dependency/output/file graph
rpmx diff <old> <new>            # recipes, RPMs, manifests
rpmx sbom [--format cyclonedx|spdx]
rpmx attest                      # emit an unsigned provenance statement (interface only)
rpmx import-spec <file>          # best-effort conversion
rpmx shell [--stage ...]         # enter the sandbox at a stage
rpmx clean
```

Common flags:

- `--profile <name|path>`
- `--features <spec>`
- `--locked`
- `--json`
- `--no-sandbox` (loud warning; records `sandbox = false` in provenance)
- `--keep-workdir`
- `--jobs N`
- `--target <triple>`

Exit codes:

| Code | Meaning |
|---|---|
| 0 | Success. |
| 2 | Recipe/schema error. |
| 3 | Lock drift. |
| 4 | Dependency or feature resolution failure. |
| 5 | Build failure. |
| 6 | Test failure. |
| 7 | Policy violation. |
| 8 | Evidence/reproducibility failure. |
| 9 | Internal error. |

`--json` output is a stable, versioned schema. Every diagnostic has:

```json
{
  "code": "RPMX-FILE-007",
  "severity": "error",
  "message": "file /usr/lib/libcurl.so.4 is claimed by both 'libcurl' and 'libcurl-devel'",
  "path": "package.yaml",
  "line": 142,
  "field": "output[2].files.include",
  "suggestions": ["remove the path from one output", "use auto-split"]
}
```

Codes are centralised as `Code` constants in `rpmx_core::error::codes`; `codes::ALL` is the catalog, and a test enforces uniqueness and the `RPMX-<DOMAIN>-<NNN>` format. Messages, paths and suggestions stay at the call site because they are contextual.

---

## 7. Implementation strategy

### 7.1 V1: native SRPM writer first

The native SRPM is the contract, so its writer cannot be deferred. The early implementation should:

1. Plan and execute all build steps in a sandbox.
2. Produce the deterministic source snapshot.
3. Write the native SRPM directly:
   - RPM source header from the recipe/lock;
   - payload containing sources, patches, recipe, lock, steps and manifest;
   - no spec in the native variant;
   - deterministic ordering, mtimes and compression.
4. Verify the SRPM with `rpmx verify-srpm` and with independent RPM tooling.

The SRPM is emitted unsigned by default. Signing is optional and caller-driven: `rpmx srpm --signing-key KEY` signs the SRPM after it is written, outside the build sandbox. `rpmx sign` applies the same operation to existing artifacts. rpmx never creates, stores or discovers a key unless the caller runs `rpmx keygen` and keeps the result.

The writer options should be evaluated in M1:

- `rpm-rs` `PackageBuilder` / `rpm` crate, if it can produce source packages directly;
- `librpmbuild` bindings, if they expose source-package construction without a spec;
- a minimal writer for the source header + compressed payload.

The spike in [`spikes/01-native-srpm`](../../spikes/01-native-srpm/README.md) answered this for `rpm` 0.30.2: the crate writes a readable, digest-valid package, but marks every package binary. A 12-line patch that sets `SOURCEPACKAGE=1` and omits `SOURCERPM` is enough for RPM 6 to accept it as a source package and for `rpm2cpio` to extract it. The production writer still has to set the real build architecture and normalise payload paths; deriving "source" from `arch: src` is a spike shortcut, not the design.

The binary spike in [`spikes/02-native-binary`](../../spikes/02-native-binary/README.md) confirms the same crate can produce an installable binary RPM, but its `SOURCERPM` is hardcoded to `(none)`. The production writer must set the actual SRPM NEVRA, or rebuild traceability breaks.

If none can produce a valid source package without a spec, the writer milestone is blocked; there is no spec-based fallback.

### 7.2 V1: native binary writer, no `rpmbuild`

Binary RPMs are assembled by `rpmx` itself, not by `rpmbuild` and not through an ephemeral internal spec. **No release ships a path that depends on spec semantics.**

This is a deliberate scope increase. It means the first usable build requires:

- a binary RPM writer or a thin `librpm` binding;
- header construction for every field the recipe emits;
- scriptlet and trigger emission from typed actions;
- file metadata, digests and dependency generation;
- debuginfo only where the package opts in (§4.7), never as an automatic rpmbuild side effect.

The `rpmbuild` path was previously kept as a temporary bridge. It is removed from the plan because it would define binary semantics with a second, invisible build definition. If the native binary writer cannot be made to work, the project is blocked rather than degraded; that is the intended failure mode.

There is no compatibility spec. Native SRPMs are rebuilt by `rpmx`; downstreams that require `rpmbuild --rebuild` are not supported.

### 7.3 Language choice

A reference implementation in Rust is a good fit:

- deterministic YAML parsing and schema generation (`serde_yaml`/`schemars` or equivalent);
- a single static binary for CI and build roots;
- safe process execution and sandbox integration;
- no Python runtime in the build root.

A Python 3.13 implementation is a reasonable v0 for faster iteration (`tomllib`, `subprocess`, `jsonschema`), at the cost of a runtime dependency and slower planning. The architecture, not the language, is the design.

### 7.4 Sandbox

- Linux namespaces via bubblewrap or a rootless container.
- Fresh root per build; only the source tree, declared tools and dependency sysroot are mounted.
- Network namespace with no interfaces after fetch.
- `no_new_privs`, dropped capabilities, seccomp where practical.
- Build user with a fixed UID/GID; file ownership normalized at packaging time.
- `--no-sandbox` exists for bootstrapping and debugging, but is recorded as a provenance fact and is rejected by profiles that require isolation.

### 7.5 Reproducibility

- `SOURCE_DATE_EPOCH` from the lock.
- `LC_ALL=C.UTF-8`, `TZ=UTC`, `umask 022`.
- Sorted manifests; stable archive order; normalized mtimes.
- Build path normalization (`--remap-path-prefix`, `-ffile-prefix-map` where supported).
- `%build_mtime_policy` / RPM 4.20 timestamp policy for binary package assembly.
- `rpmx test --reproduce` builds twice in different directories and compares the unsigned payload and headers.
- Differences are classified: expected (signature, build time) vs. unexpected (content, order, metadata).

### 7.6 Security

- The proxy is untrusted; sources are verified against the trusted lock before use.
- The SRPM is untrusted until its manifest and digests are verified, and its signature is verified when policy requires one.
- Signatures are verified if required by policy; the result is recorded.
- No network in the build sandbox.
- No credentials in the sandbox.
- Path traversal and symlink attacks are checked during extraction and packaging.
- Scriptlet/action emitters quote all paths and arguments.
- `rich` dependency strings are parsed by RPM's parser, not by string concatenation.
- `rpmx` emits unsigned artifacts and evidence by default. It does not hold keys. Optional package signing happens after the build with a caller-supplied key (`--signing-key` or `rpmx sign`); the key is never mounted into the build sandbox. Provenance attestations remain unsigned for an external signer or transparency service.

The full threat model, including what content addressing does **not** protect against, is in [`08-threat-model.md`](08-threat-model.md).

---

## 8. Design decisions and rejected alternatives

### Strict YAML, not TOML, for the recipe

The first draft used TOML because it has explicit types and no significant indentation. The `curl` example showed the cost: `[[output]]` blocks, header-driven nesting, inline tables, and "last array element" scoping make a package tree hard to scan. YAML 1.2 gives visual hierarchy and block sequences, at the cost of implicit typing and anchors. The cost is manageable with a strict profile: core schema only, `true`/`false` only, no anchors/aliases/merge keys, no custom tags, quoted versions/hashes/modes, duplicate keys rejected, JSON Schema validation, and a canonical formatter. JSON is kept for the generated lock and machine interfaces, where comments and indentation do not matter.

### No general-purpose language in the recipe

Rejected: Starlark, Nix, Scheme, Python, Lua. They make the recipe analyzable only by executing it, and they raise the barrier for packagers. The escape hatch is an inline pipeline step (`argv` or `runs`), which is sandboxed and visible in the plan.

### No macros

Rejected: `%define`, `%global`, `%{...}`, `%if`. The interpolation surface is a closed, typed variable scope (`package.*`, `profile.*`, `cell.*`, `matrix.*`, `vars.*`) with fixed-point expansion and no expressions; see [`12-templates.md`](12-templates.md). Profiles and features replace conditionals, and matrices replace per-version `%if` blocks.

### Pipelines, melange-style

The build is an ordered pipeline of named steps, as in melange. This replaces an earlier design that put a typed adapter (`build.system: cmake`) in front of the steps. The adapter hid the commands, had no defined option schema, and let a recipe omit tools the build actually needed. A pipeline shows what runs, and `needs` on a step makes its tools explicit.

`runs:` is a normal step form, not a quarantined escape hatch. The sandbox, declared tool set, disabled network and post-hoc file manifest supply the guarantees; a `reason:` field would be boilerplate and a schema ban would add friction without a security property. Distro profiles may opt into `require: argv` where policy demands it. See [`13-pipelines.md`](13-pipelines.md).

### Package build and image composition are separate

Melange/apko is the best recent evidence that these are different problems. `rpmx` builds RPMs and emits a package-set manifest; it does not grow an image composer. Tools like apko, OSBuild or bootc can consume the manifest and keep their own composition invariants (apko's no-`RUN` rule is the strongest example).

### Native SRPM writer early

Rejected: deferring the native writers behind a generated-spec backend. The format is complex, but making the spec the default creates two build definitions and contradicts the native-SRPM contract. Both the source and binary writers are required before a release; there is no `rpmbuild` path, ephemeral or otherwise.

### Auto-split with review

Rejected: fully manual file lists (too much boilerplate) and fully automatic splitting (too surprising). Auto-split is policy-driven, shown in `rpmx plan`, and overridable.

### Typed rich dependencies, with an escape hatch

Rejected: exposing RPM's full rich-dependency grammar as the primary interface. Most packages need `any_of`/`all_of`; the long tail can use `rich`.

### Limited profiles

Rejected: a full overlay language that can add arbitrary outputs or code. That would recreate the macro problem at a different layer. V1 profiles set/append/remove/merge existing values; features handle optional outputs.

### Installed tests are profile-mandatory

Rejected: making every package run an installed test in the format. Some packages cannot. The format supports tests; the distro decides whether they are mandatory.

---

## 9. What "better" would mean

The design is a hypothesis. It should be judged by measurable outcomes, not by elegance:

| Metric | Target |
|---|---|
| Time to package a simple new package | Less than the spec equivalent, for a packager who knows the tool. |
| Recipe size for common packages | Smaller than the spec equivalent; no macro boilerplate. |
| Invalid states caught before build | File collisions, unowned files, feature conflicts, missing sources, bad schema, policy violations. |
| Reproducible rebuild rate | High for packages with locked toolchains; failures explained. |
| Review time | Reviewers can see metadata, dependencies, files and actions without running the build. |
| Policy change cost | A distro flag/path/test policy change requires no recipe edits. |
| Inline-step rate | Low; named steps are the norm. |
| Migration cost | Simple specs convert mechanically; complex specs convert with a clear TODO list. |

The next document covers migration, adoption and the experiments that would falsify this design.
