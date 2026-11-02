# Bundled provides and the component inventory

Status: phase 1 implemented, 2026-11-02. Go, Cargo, npm, Yarn, pnpm, Python,
Ruby, PHP and explicit C/C++ declarations are live. Java, Conan, vcpkg, SBOM
projection, `explain components` and `bundled/declare` remain planned.

## 1. Goals

1. Generate RPM `Provides: bundled(...)` from evidence, not from hand-maintained
   recipe entries.
2. Support ecosystems with machine-readable vendor metadata: Go, Rust,
   Node.js (npm, Yarn, pnpm), Java, and later Python, Ruby, PHP, Conan and
   vcpkg.
3. Give C and C++ recipes an explicit, build-controlled way to declare bundled
   components and versions. Automatic detection is not required there.
4. Reuse one component inventory for three consumers:
   - `bundled(...)` provides in binary RPMs,
   - SPDX components in the build SBOM,
   - `rpmx explain` and lint diagnostics.
5. Keep discovery out of pipeline steps. A step materialises a vendor tree; the
   build engine discovers what is there.
6. Keep the result deterministic, lockable and offline-rebuildable.

## 2. Non-goals

- Deciding whether bundling is allowed. That is a distribution policy decision.
- Automatically proving that a C or C++ vendored library is present. There is
  no reliable universal manifest; C/C++ uses explicit declarations.
- Running package managers during packaging. Discovery parses existing files;
  it does not run `go list`, `cargo metadata`, `npm ls` or `mvn`.
- Replacing Syft. Syft may become an optional cataloger and SBOM backend, but
  it is not the authority for `bundled()` provides.

## 3. Compatibility policy

This is greenfield work. There are no users to migrate, so there is no legacy
fallback:

- A native SRPM produced by this version always contains
  `RPMX-BUNDLED.json`, including an empty component list.
- A rebuild of an SRPM without that file fails with a clear error. Do not add a
  fallback scanner for old SRPMs.
- Recipe schema changes are additive where possible. `outputs[].bundled`,
  `generators.bundled` and the `${BUILD}/.rpmx/bundled/` contract are new; no
  alias fields and no dual layout.

## 4. Fedora policy

The feature follows Fedora's packaging guidelines.

- Generic bundling: `Provides: bundled(<libname>) = <version>`, or
  `Provides: bundled(<libname>)` when no version can be determined.
  `<libname>` should be the distribution package name if one exists.
- Go: every vendored module gets
  `Provides: bundled(golang(<import path>)) = <version>`.
- Rust: every vendored crate gets
  `Provides: bundled(crate(<crate>)) = <version>`.
- Node.js: every bundled module gets
  `Provides: bundled(nodejs-<module name>) = <version>`. Fedora's Node
  generator prefixes the module name with `nodejs-`, not `npm(...)`.
- Provides on the main package are not inherited by subpackages. A component
  belongs on the output that carries the code.
- No package may `Requires: bundled(...)`.
- Java packaging normally requires unbundling. The Java policy should reject a
  bundled jar by default, not silently label it.

Version formatting is policy, not cataloger logic:

| Ecosystem | Rule |
|---|---|
| Go | Strip a leading `v`. Replace remaining `-` with `_`. For a pseudo-version, use the 7-character commit hash. |
| Rust | Convert semver prerelease `-` to `~` (`0.2.0-rc.0` becomes `0.2.0~rc.0`). Git snapshots become `<version>+git<ref>`. |
| Node.js | Use the version from the lockfile/`package.json` after validating it as an RPM version. |
| Python | Normalize the name to PEP 503. Drop local versions and trailing `.0` components. Pre-release becomes `~rc1`/`~a1`/`~b1`, development becomes `~~devN`, post-release becomes `^postN`. |
| Ruby | Strip a gem platform suffix such as `-x86_64-linux`. A prerelease segment becomes `~rc1` (`1.14.0.rc1` becomes `1.14.0~rc1`). |
| PHP | Use the Composer version after validating it as an RPM version. |
| Generic | Use the declared version after validating it as an RPM version. If it is missing or invalid, emit an unversioned provide and warn. |

Multiple versions of the same automatically detected component are allowed and
produce one versioned provide per version, matching Fedora. A manually declared
generic component may have only one version per output.

## 5. Model

All discovery converges on one record:

```rust
pub struct Component {
    pub ecosystem: Ecosystem,       // Golang, Cargo, Npm, Yarn, Pnpm, Maven, ...
    pub name: String,               // module path, crate, npm package, group:artifact
    pub version: Option<String>,    // raw ecosystem version
    pub purl: Option<String>,       // pkg:golang/..., pkg:cargo/..., ...
    pub license: Option<String>,
    pub scope: Scope,               // Declared | Vendored | Embedded | Installed
    pub confidence: Confidence,     // Exact | Derived | Heuristic
    pub source: Source,             // Auto | Recipe | Build
    pub evidence: Vec<Evidence>,    // relative path + cataloger
    pub outputs: BTreeSet<String>,  // resolved after attribution
}
```

`scope` is how the engine distinguishes “declared” from “bundled”:

- `Declared`: the project declares a dependency, but it may come from the system.
- `Vendored`: the source tree contains the dependency.
- `Embedded`: a built artifact contains it.
- `Installed`: the payload contains it under a bundled location.

Only `Vendored`, `Embedded`, and appropriate `Installed` evidence produce
automatic provides. `Declared` alone produces a warning, not a provide.

`confidence` controls automatic policy:

- `Exact`: a lockfile or vendor manifest with versions. Automatic provide.
- `Derived`: binary metadata or generated manifest. Automatic provide.
- `Heuristic`: directory or filename inference. Warning only unless the recipe
  opts in.

## 6. Catalogers

A cataloger owns one evidence format:

```rust
pub trait Cataloger {
    fn name(&self) -> &'static str;
    fn version(&self) -> u32;
    fn scopes(&self) -> &'static [Scope];
    fn detect(&self, ctx: &DiscoveryContext) -> Result<Vec<Component>>;
}
```

`DiscoveryContext` contains a single file index of the source or stage tree.
Catalogers declare filename patterns; they do not walk the tree independently.
This keeps discovery linear in the number of files, not catalogers times files.

Catalogers:

| Cataloger | Evidence | Scope | Automatic | Status |
|---|---|---|---|---|
| `go/modules-txt` | `vendor/modules.txt` | Vendored | yes | implemented |
| `cargo/lock` | `Cargo.lock` with `vendor/` | Vendored | yes | implemented |
| `node/package-lock` | `package-lock.json` v2/v3 | Installed | yes | implemented |
| `node/installed` | `node_modules/*/package.json` | Installed | yes | implemented |
| `manual/recipe` | `outputs[].bundled` | Vendored | explicit | implemented |
| `manual/build` | `${RPMX_BUNDLED_DIR}/<output>/*.list` | Vendored | explicit | implemented |
| `go/buildinfo` | `.go.buildinfo` in staged ELF | Embedded | yes | planned |
| `cargo/vendor` | `cargo-vendor.txt`, per-crate manifests | Vendored | yes | planned |
| `cargo/auditable` | `.dep-v0` section | Embedded | yes | planned |
| `node/yarn-lock` | `yarn.lock` v1 and Berry | Installed | yes | implemented |
| `node/pnpm-lock` | `pnpm-lock.yaml` | Installed | yes | implemented |
| `python/dist` | `.dist-info`/`.egg-info` metadata, `vendored.txt` | Installed/Vendored | yes | implemented |
| `ruby/gems` | `Gemfile.lock`, vendored gemspecs | Declared/Installed | yes | implemented |
| `php/composer` | `composer.lock`, `vendor/composer/installed.json` | Declared/Installed | yes | implemented |
| `java/maven-embedded` | `META-INF/maven/**/pom.properties` in jars | Embedded | policy-dependent | planned |
| `java/maven-declared` | `pom.xml` | Declared | no | planned |

Later catalogers follow the same shape: Poetry/Pipfile locks, Conan
`conan.lock`, vcpkg `vcpkg.json` plus `vcpkg_installed`, and an optional
SPDX/CycloneDX input cataloger.

A cataloger must parse strictly:

- cap input sizes (for example 8 MiB per text manifest);
- reject unsupported schema versions with a specific error;
- never execute a package manager;
- never follow a symlink outside the discovery root;
- report evidence paths relative to the root.

## 7. Naming and policy

Catalogers report components, not RPM strings. A naming policy turns a
component into one of:

```rust
pub enum Decision {
    Provide(String),
    Warn(String),
    Reject(String),
}
```

The Fedora policy emits:

- `bundled(golang(<path>)) = <version>`
- `bundled(crate(<name>)) = <version>`
- `bundled(nodejs-<name>) = <version>` for Node.js
- `bundled(python3dist(<pep503-name>)) = <version>` for Python
- `bundled(rubygem-<name>) = <version>` for Ruby
- `bundled(php-composer(<vendor>/<package>)) = <version>` for PHP
- `bundled(<name>) = <version>` for generic components
- `bundled(<name>)` when the version is unknown

The policy also decides that a Java bundled jar is rejected, and that a
`Declared`-only component is warned about rather than provided.

The policy is configurable as a whole (`fedora`, `generic`, later `opensuse`),
not per component. Do not put naming rules in catalogers.

## 8. Output attribution

The provide must land on the output that carries the code. Attribution runs
after discovery and before packaging:

1. If the evidence path is inside a staged output, use that output.
2. If a binary cataloger found the component inside a file in the output, use
   that output.
3. If an explicit declaration names an output, use that output.
4. If the source evidence has no output context, use the output whose
   `files.include` covers the component’s installed path.
5. Otherwise attach to the primary output (`output.name == package.name`) and
   emit a warning.

Never attach to a metapackage, `-devel`, `-doc` or `-debuginfo` without direct
evidence. If a component cannot be attributed at all, warn; do not silently drop
it.

## 9. Explicit C and C++ declarations

C and C++ dependencies are declared explicitly. There are two forms, and both
feed the same inventory.

### 9.1 Static recipe declaration

Use this when the version is known when the recipe is written:

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

`name` and `version` are strings. `version` and `license` are optional.
`license` is used only for the SBOM; it does not change the provide. A missing
version emits `bundled(<name>)` without a version.

Manual declarations are always generic. They cannot spell `golang(...)` or
`crate(...)`; those come from their catalogers. This prevents a recipe from
forging ecosystem evidence.

Validation:

- `name` matches `[A-Za-z0-9][A-Za-z0-9+._-]*`.
- `version` is a valid RPM EVR fragment: no whitespace, `=`, `(`, `)`, `/` or
  control characters.
- The output name must exist.
- The same name may appear at most once per output.
- A recipe declaration with the same name and version as an automatic
  component is deduplicated. A different version is an error.

### 9.2 Build-time declaration

Use this when the version is computed during source assembly or build, for
example from a vendored header, `configure` output or a generated version file.

A step writes a line-oriented file:

```text
${RPMX_BUNDLED_DIR}/<output>/<anything>.list
```

`RPMX_BUNDLED_DIR` defaults to `${BUILD}/.rpmx/bundled` and is also available
as the `${bundled.dir}` template. The engine creates the root and one directory
per planned output before the source-assembly and build pipelines start, so a
step does not need `mkdir -p`. A misspelled output name fails because that
directory does not exist.

One file per declaration keeps concurrent steps from racing. The directory name
is the output name. The file is UTF-8, one component per line:

```text
# comments and blank lines are ignored
zlib=1.3.1
libpng=1.6.43
```

`name` is required. `version` is optional. A line with only `name` emits an
unversioned provide. The name and version must satisfy the same validation as
the recipe form. Files and directories are read in sorted order.

Example:

```yaml
build:
  pipeline:
    - name: declare-bundled-zlib
      runs: |
        version=$(sed -n 's/^#define ZLIB_VERSION "\(.*\)"/\1/p' \
          third_party/zlib/zlib.h)
        test -n "$version"
        printf 'zlib=%s\n' "$version" \
          > "$RPMX_BUNDLED_DIR/libfoo/zlib.list"
```

A later built-in step can make this safer:

```yaml
    - uses: bundled/declare
      with:
        output: libfoo
        name: zlib
        version-file: third_party/zlib/VERSION
        license: Zlib
```

`bundled/declare` is a convenience. The file layout is the stable contract; a
step may always write it directly. If both `version` and `version-file` are
given, the step fails. If neither is given, the component is unversioned.

### 9.3 Merge and conflict rules

- Recipe declarations and build declarations are both explicit and
  authoritative.
- The same `(output, name, version)` deduplicates.
- The same `(output, name)` with different versions is an error for a manual
  generic component. Split the output or use a distinct name.
- The same `(output, name)` with one versioned and one unversioned declaration
  is an error.
- If a manual generic component has the same name and version as an automatic
  component in the same output, deduplicate.
- If they differ, fail. Do not silently override lockfile evidence. To drop the
  automatic provide instead, use `generators.exclude` with its exact string.
- An unknown output name in a build declaration is an error.
- A declaration directory is refused if it is a symlink. A non-directory entry
  directly under the bundled root is an error. Only `*.list` files inside an
  output directory are read.

`rpmx build --stage` runs no build steps and therefore sees no build-time
declarations. Use the static recipe form for a staged-tree package.

## 10. Integration

Add a `rpmx-components` crate. Keep it independent of RPM writing and of the
sandbox; it operates on paths and bytes.

Integration points:

- `crates/rpmx-build/src/source.rs`: after `sandbox.sync()` and before the
  source snapshot is packed, discover components from the assembled workspace
  and write the inventory.
- `crates/rpmx-build/src/source_package.rs`: add `RPMX-BUNDLED.json` to the
  SRPM, reserve the name, and record its digest in `RPMX-MANIFEST.json`.
- `crates/rpmx-cli/src/commands/rebuild/mod.rs`: read the embedded inventory,
  run stage discovery after the build steps, merge with the same conflict
  rules, and pass the result to packaging.
- `crates/rpmx-build/src/binary_package.rs`: for each output, resolve names and
  append generated provides before `sorted_unique`.
- `crates/rpmx-build/src/sbom.rs`: project inventory components into SPDX
  packages with purls, license and `CONTAINS` relationships.
- `crates/rpmx-cli/src/commands/explain.rs`: add an `explain components` topic
  showing evidence, scope, confidence, output and final provide.
- `crates/rpmx-cli/src/commands/package_lint.rs`: reject `Requires:
  bundled(...)`; warn on unattributed, declared-only or heuristic components.
- `crates/rpmx-language/src/model.rs`: add `bundled` to the output model and
  `generators.bundled` to `OutputGenerators`, defaulting to `true`.
- `crates/rpmx-runner/src/sandbox/mod.rs`: export `RPMX_BUNDLED_DIR` and
  `${bundled.dir}`. The caller creates `${BUILD}/.rpmx/bundled/<output>/` for
  every planned output before starting the sandbox.

No pipeline step needs to change. `go/vendor`, `cargo/vendor` and `npm/ci`
continue to materialise trees only.

## 11. Inventory artifact

`RPMX-BUNDLED.json` is a required SRPM file. It is deterministic and contains
no timestamps or absolute paths:

```json
{
  "schema": 1,
  "catalogers": [
    {"name": "go/modules-txt", "version": 1},
    {"name": "manual/build", "version": 1}
  ],
  "components": [
    {
      "ecosystem": "generic",
      "name": "zlib",
      "version": "1.3.1",
      "license": "Zlib",
      "scope": "vendored",
      "confidence": "exact",
      "source": "build",
      "evidence": [".rpmx/bundled/libfoo/zlib.list"],
      "outputs": ["libfoo"]
    }
  ]
}
```

Components are sorted by `(outputs, ecosystem, name, version)`, and evidence
paths are sorted. The cataloger list is sorted by name. `schema` changes are
breaking; a rebuild requires the same inventory schema.

The inventory is the authority for rebuild. Do not rescan source files to
invent components that are absent from the SRPM. Stage discovery may add
components that only become visible after the build, such as classes inside a
fat jar, but it must apply the same conflict rules.

## 12. SBOM

The SBOM uses the same inventory. Each component becomes an SPDX package with:

- a purl when one is available;
- name and version;
- license or `NOASSERTION`;
- the same evidence path;
- a `CONTAINS` relationship from the binary or source package.

This keeps `bundled()` provides and the SBOM from drifting apart.

## 13. Errors

Proposed error families, final codes to be assigned in `rpmx-base`:

| Condition | Behaviour |
|---|---|
| Unsupported lockfile or inventory schema | Error |
| Malformed manual declaration | Error |
| Unknown output in a build declaration | Error |
| Conflicting manual versions | Error |
| Explicit and automatic versions differ | Error |
| Component has no version | Unversioned provide and warning |
| Declared-only component | Warning, no provide |
| Heuristic component | Warning, no provide by default |
| Component cannot be attributed | Warning, attach to primary output |
| `Requires: bundled(...)` | Error |

## 14. Tests and acceptance

Unit tests:

- Go: semver, pseudo-version, replacement, local replacement, missing version.
- Rust: prerelease, git snapshot, multiple versions of one crate.
- Node: npm, Yarn v1, Yarn Berry, pnpm; production-only filtering.
- Java: `pom.properties` inside a jar versus `pom.xml` alone.
- Manual: recipe form, build-list form, conflicts, unknown output, invalid
  names and versions.
- Inventory: deterministic bytes across repeated runs.

Integration tests:

- A fixture package with a vendored Go module and a vendored crate builds an
  RPM whose `rpm -qp --provides` contains the exact expected strings.
- A C fixture whose `VERSION` file changes between builds changes the generated
  provide without editing the recipe.
- `rpmx rebuild` reproduces the same provides offline from the SRPM.
- A conflicting manual and automatic declaration fails with a specific error.
- `Requires: bundled(zlib)` is rejected.
- Cross builds parse target ELF/JAR evidence host-side.

## 15. Rollout

1. Component model, inventory artifact, resolver, naming policy. **Done.**
2. Manual C/C++ declarations and Go/Rust catalogers. Prove end to end with
   `rpm -qp --provides`. **Done.**
3. npm cataloger and `bundled(nodejs-<name>)` provides. **Done.**
4. Yarn, pnpm, Python, Ruby and PHP catalogers. **Done.**
5. Java embedded-jar cataloger. Java uses a reject-by-default policy.
6. SBOM projection, `explain components`, Conan, vcpkg, optional SPDX input and
   optional Syft.

Each phase must add fixtures and an end-to-end assertion. A cataloger without a
test that can fail is not done.
