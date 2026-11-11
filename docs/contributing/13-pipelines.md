# Pipelines and named steps

> Historical design record. Some schema and execution examples describe earlier
> proposals. Use [current recipes](../usage/recipes.md) and
> [testing](../usage/testing.md) for the supported schema 2 interface.

## 1. Decision

The build is an **ordered pipeline of steps**, modeled directly on melange.

There is no `build.system` selector and no separate "adapter" layer. Common build systems ship as named step bundles (`autotools`, `cmake`, `meson`, `cargo`, `go`, `python`, `npm`). A recipe either `uses` a bundle, `uses` individual steps, or writes an inline step.

This is a deliberate reversal of the earlier design, which put a typed adapter in front of the step list. The adapter hid the commands, had no defined option schema, and let a recipe declare a build without declaring the tools that build needed. Melange's model is simpler and more legible: what runs is a list, and reuse is a name in that list.

## 2. Recipe shape

```yaml
build:
  packages:
    - cmake
    - ninja-build
  sysroot:
    - openssl-devel
    - zlib-devel
  environment:
    CURL_CA_BUNDLE: /etc/pki/tls/certs/ca-bundle.crt
  pipeline:
    - uses: cmake/configure
      with:
        args:
          - -DBUILD_TESTING=ON
          - -DCURL_USE_OPENSSL=ON
    - uses: cmake/build
    - uses: cmake/check
    - uses: cmake/install
    - uses: strip
```

A bundle collapses the common sequence:

```yaml
build:
  pipeline:
    - uses: cmake
      with:
        args:
          - -DBUILD_TESTING=ON
          - -DCURL_USE_OPENSSL=ON
```

`uses: cmake` is a named pipeline that itself uses `cmake/configure`, `cmake/build`, `cmake/check` and `cmake/install`. A bundle is not magic; it is the same mechanism applied one level up.

Inline steps:

```yaml
build:
  pipeline:
    - uses: cmake/build
    - name: remove-duplicate-docs
      argv: [rm, -rf, "${stage}/usr/share/doc/curl"]
    - name: fix-prefix
      runs: |
        sed -i 's|/usr/local|/usr|g' Makefile
```

## 3. Step forms

A pipeline entry is exactly one of:

| Form | Meaning |
|---|---|
| `uses: <name>` + `with:` | Run a named step or bundle from the step library. |
| `argv: [...]` | Run a command directly. No shell. |
| `runs: \|` | Run a shell fragment. |

Common optional fields on any entry:

| Field | Type | Meaning |
|---|---|---|
| `name` | string | Label for diagnostics and the plan. Defaults to the `uses` name or `argv[0]`. |
| `cwd` | string | Working directory. Defaults to `${source.dir}` for build steps. |
| `env` | map | Extra environment for this step only. |
| `timeout` | duration | Per-step limit. |

`uses` and `argv`/`runs` are mutually exclusive, and `argv` and `runs` are mutually exclusive.

An inline step must not spell the workspace or stage root (`/tmp/build`,
`/tmp/stage`). Use `${source.dir}` and `${stage}`; the planner rejects a literal
with `RPMX-STEP-019`.

There is no `if:`. Conditional behavior belongs in features, matrix axes or separate outputs. Melange's `if:` is rejected for the same reason `%if` is rejected.

## 4. Step definition

A named step is a small YAML document:

```yaml
name: python/build-wheel
version: 1
summary: Build a PEP 517 wheel with a specific interpreter
inputs:
  python:
    type: string
    required: true
  outdir:
    type: string
    default: dist
needs:
  tools:
    - python3-build
run:
  cwd: "${source.dir}"
  argv:
    - "${inputs.python}"
    - -m
    - build
    - --wheel
    - --no-isolation
    - --outdir
    - "${inputs.outdir}"
```

Fields:

| Field | Required | Meaning |
|---|---|---|
| `name` | yes | Namespaced step name, e.g. `python/build-wheel`. |
| `version` | yes | Integer step schema version. |
| `summary` | yes | One line shown by `rpmx explain`. |
| `inputs` | no | Typed inputs with `type`, `required`, `default`. |
| `needs.tools` | no | Tools the planner must add to the build root. |
| `needs.host` | no | Libraries/headers the step needs. |
| `run.argv` or `run.runs` | yes | The command, or the shell fragment. |
| `run.cwd` | no | Working directory. |
| `run.env` | no | Step-local environment. |
| `run.shell` | no | Shell for `run.runs`; defaults to `/bin/sh -euo pipefail`. |

Input types are a closed set: `string`, `integer`, `boolean`, `list<string>`. Unknown inputs, missing required inputs and type mismatches are planner errors.

`needs.packages` and `needs.sysroot` accept scalar input templates. For example,
`packages: ["${inputs.build-package}"]` lets a recipe select a versioned tool
package. The planner expands the input before it adds the dependency. Keep each
entry to one package requirement.

`needs` is the important part. It means a step cannot run in an environment that lacks its tools, and it removes a whole class of broken recipes: `uses: python/build-wheel` brings `python3-build` with it.

## 5. Bundles

A bundle is a named step with only `uses` entries:

```yaml
name: cmake
version: 1
summary: Configure, build, check and install a CMake project
inputs:
  args:
    type: list<string>
    default: []
  build_dir:
    type: string
    default: build
pipeline:
  - uses: cmake/configure
    with:
      args: "${inputs.args}"
      build_dir: "${inputs.build_dir}"
  - uses: cmake/build
    with:
      build_dir: "${inputs.build_dir}"
  - uses: cmake/check
    with:
      build_dir: "${inputs.build_dir}"
  - uses: cmake/install
    with:
      build_dir: "${inputs.build_dir}"
```

A bundle may not contain `argv` or `runs`; if it needs a command, it uses a step that does. This keeps bundles reviewable as composition.

## 6. What the executor provides

Unpacking is not a step. The verified source snapshot is already extracted when the pipeline starts, because the sandbox is entered from the SRPM payload. The executor sets:

| Variable | Meaning |
|---|---|
| `${source.dir}` | Extracted primary source directory. |
| `${build.dir}` | Build directory. |
| `${stage}` | Staging root, the future package root. |
| `${bundled.dir}` | Root for build-time bundled declarations; `${build.dir}/.rpmx/bundled`. |
| `${prefix}` | Install prefix from the profile. |
| `${libdir}` | Library directory from the profile. |
| `${jobs}` | Parallelism, or `auto`. |
| `${arch}`, `${target}` | Architecture and target triple. |
| `${profile}` | Profile name. |
| `${features}` | Enabled features. |
| `${cell.id}`, `${cell.slug}` | Matrix cell. |
| `${matrix.*}`, `${vars.*}` | Matrix and user variables. |
| `SOURCE_DATE_EPOCH` | Reproducible timestamp. |

`JOBS` is set from the recipe's jobs setting. Built-in steps pass it explicitly (`make -j"$JOBS"`, `cmake --build --parallel "$JOBS"`, `meson compile -j "$JOBS"`), so a step does not have to guess the parallelism. `MAKEFLAGS` is not set.

`${stage}` is also exported as the runtime environment variable `STAGE`. Recipes may use either form for `DESTDIR`.

Every executor path has an environment equivalent: `SRC`/`BUILD` for
`${source.dir}`/`${build.dir}`, `STAGE` for `${stage}`, `RPMX_BUNDLED_DIR` for
`${bundled.dir}`, `PREFIX`/`LIBDIR` for `${prefix}`/`${libdir}`, and `JOBS` for
`${jobs}`. A step's `runs` script uses
the variable form (`"$STAGE"`, `--prefix="$PREFIX"`); `cwd`, `argv` and `env`
use the `${...}` form because they are not shell. Built-in steps contain no
literal guest path and no hardcoded `/usr`; both come from the executor. The
staging root is the only tree that is packaged; output `files.include` patterns
are matched relative to it.

Steps that vendor C or C++ code may declare the bundled components and their
versions under `${RPMX_BUNDLED_DIR}/<output>/*.list`. The engine creates the
root and one directory per planned output before the source-assembly and build
pipelines start, so steps do not create them. The step writes declaration data,
not RPM strings: the build engine reads it after the step list finishes and
turns it into `bundled(...)` provides. Source-assembly declarations are captured
in the source snapshot. The format and conflict rules are in
[`19-bundled-provides.md`](19-bundled-provides.md).

Patches declared at the top level of the recipe are applied in order after the
source archives are materialised and before the source-assembly pipeline runs.
They are part of the source snapshot, so a rebuild does not reapply them.
Source assembly runs in a locked root by default (`--root-lock --root`) and
records `source-root.lock.json`; `--unsafe-host-root` is the explicit exception.

Tests run after packaging. `stage: build` tests run in the build sandbox.
`stage: installed` tests run in a fresh test root: each test's
`environment.contents.packages` and the selected RPMs' external runtime
requirements are resolved from the same repositories, assembled
offline, and the selected built RPMs are installed over it with real dependency
checking, not `--nodeps` (`--run-tests`). Requirements provided by another
selected output stay out of the repository resolution. A
recipe-level test selects every output. An output-level test selects its own
output plus the sibling outputs that satisfy its RPM requirements. Tests with a
different output or package set get separate roots. The resolved test roots are
written beside the RPMs as
`<name>-<version>-<release>[.<n>].test-root.lock.json`; `--test-root-lock FILE`
assembles a pinned lock offline and checks that the fixed test base environment
is present; the lock is authoritative for the rest.
`--sbom FILE` overrides the build-level SPDX path; per-package documents are
embedded in the RPMs.

`strip` (`uses: strip`) removes debug sections. Debug information is opt-in per
recipe: `uses: debuginfo` replaces `strip`, extracts each ELF's debug sections
to `$PREFIX/lib/debug/<path>.debug`, strips the binary and adds
`.gnu_debuglink`. The recipe compiles with `-g` and declares an output that
claims `/usr/lib/debug/**`; the engine generates no debug subpackage and the
profile has no debuginfo field. The step fails when nothing contains DWARF.
`-debugsource` is not generated. The policy is recorded in the
root lock and the SRPM, so a rebuild reproduces it.

The profile's `sbom: true` (the default) emits SPDX 2.3. Every binary RPM
contains `/usr/share/sbom/<name>-<version>-<release>.spdx.json`, with
relationships to its source RPM, source files and the build configuration. A
build-level document, `<name>-<version>-<release>.build.spdx.json`, is written
beside the RPMs and adds the root closure as build dependencies. `--sbom FILE`
overrides only the build-level path. The generator is behind the
`rpmx_core::sbom::Generator` trait; `NativeSpdx` is the current implementation,
so a Syft-backed generator can replace it without changing callers.

Before the first build step, `build.packages` is checked against the
selected root. A tool is accepted when the locked root contains a package with
that name, or when `command -v` finds an executable with that name inside the
root (`RPMX-ROOT-018` otherwise). A version constraint is parsed off the name
and is not enforced yet.

There are no `RPM_*` variables, no macro environment, and no `PATH` inherited from the host.

## 7. Resolution and the lock

`uses: python/build-wheel` is resolved by the planner against the step library:

1. The built-in library embedded in `rpmx` (YAML files shipped with the tool).
2. Local files under the pipeline directory.
3. Anything else is an error.

Built-ins are not privileged Rust code. They are YAML step files with the same
schema, inputs, `needs` and `run` forms as engineer-provided steps; they are
simply embedded in the binary and resolved first.

The lock records every referenced step:

```json
{
  "steps": [
    {"name": "python/build-wheel", "version": 1, "digest": "sha512:..."},
    {"name": "python/install-wheel", "version": 1, "digest": "sha512:..."}
  ]
}
```

The resolved step definitions — built-in and local YAML alike — are embedded in the native SRPM under `RPMX-STEPS.json`, and their digests are in the lock. On `rpmx rebuild`, the SRPM copies override the built-in library, so a rebuild uses the exact steps the package was built with even if the installed `rpmx` has newer built-ins.

An unpinned step reference is a lock error. Changing a step version is a lock change and requires review, like a source bump.

### 7.1 Local step files

Packagers can define their own steps without publishing a library or an image.
A step is a YAML file in the package repository:

```
pipelines/
  acme/
    strip.yaml
```

`uses: acme/strip` resolves to `pipelines/acme/strip.yaml` relative to the
recipe, or to `--pipeline-dir DIR` when given.

```yaml
name: acme/strip
version: 1
summary: Strip a staged binary
inputs:
  path:
    type: string
    required: true
needs:
  tools: [binutils]
run:
  cwd: "${stage}"
  argv: ["strip", "--strip-unneeded", "${inputs.path}"]
```

Resolution order:

1. YAML files embedded in the SRPM (rebuild only; these win over the tool).
2. Built-in YAML files embedded in `rpmx`.
3. Local files under the pipeline directory.
4. Anything else is an error.

A local file whose name matches a built-in is rejected, not silently ignored.
Step names are `<namespace>/<name>`, limited to letters, digits, `_` and `-`, so
a name cannot escape the pipeline directory.

Rules:

- Inputs are typed: `string`, `integer`, `boolean`. Required inputs must be
  supplied by `with:`; defaults are allowed. Unknown `with` keys are errors.
- `${inputs.*}` is substituted at plan time. Executor paths such as
  `${stage}` and `${source.dir}` are left as placeholders for the executor.
- A step file must not spell a guest path (`${source.dir}`, `${stage}`, the
  configured prefix or libdir). Use `$STAGE`/`$PREFIX`/`$LIBDIR` in `runs` and
  `${stage}`/`${prefix}`/`${libdir}` in `cwd`, `argv` and `env`. The loader
  rejects a literal with `RPMX-STEP-019`.
- `run.argv` and `run.runs` are mutually exclusive. `needs` is merged into the
  build-root requirements.
- A local step is a source input. Its SHA-512 digest goes into the lock and the
  file is embedded in the SRPM under `steps/`, so a rebuild is offline and does
  not depend on the repository checkout.
- Local steps run in the same sandbox as built-ins. No network, no credentials.
- Reuse across packages is just committing the file. Review it like a recipe.

Local steps are available in both `build.pipeline` and
`source-assembly.pipeline`.

## 8. Plan output

`rpmx plan` must print the fully expanded pipeline, not the `uses` names:

```
step 1  python/build-wheel  (library 1, sha512:...)
        cwd  /build/foo-1.0.0
        argv /usr/bin/python3.10 -m build --wheel --no-isolation --outdir dist
step 2  python/install-wheel  (library 1, sha512:...)
        cwd  /build/foo-1.0.0
        argv /usr/bin/python3.10 -m pip install --root /stage --prefix /usr --no-index --no-deps dist/foo-1.0.0-py3-none-any.whl
```

`rpmx explain build` shows one step: its inputs, its bindings and the resolved command. `rpmx graph` includes steps and their tool requirements.

This is the concrete answer to "where are the build commands": the recipe names them, the step defines them, and the plan prints them expanded.

## 9. Inline `runs` is not gated

`runs:` is a normal step form, not a quarantined escape hatch.

- It runs in the same sandbox as `argv`.
- The build root contains only declared tools; shell cannot invoke an undeclared compiler.
- Network is off.
- The file manifest is computed from the staged tree, and every staged file must be claimed by an output. A step cannot hide what it installed.
- Reproducibility is enforced by the environment, not by the authoring form.

So no `reason:` field is required, and the schema does not forbid `runs`. What is enforced:

- `runs` is linted with `shellcheck` when available;
- `/bin/sh -euo pipefail` is the default shell, and a different shell must be stated;
- profiles may opt into `require: argv`, for distro policies that demand auditable commands;
- `rpmx explain` counts inline shell per recipe, so review can see it.

The last point matters more than a gate: shell is visible in the plan and counted in the manifest, so it is a review signal rather than a hidden capability.

## 10. Feature and matrix interaction

- The matrix is expanded before the pipeline is resolved, so `with:` values may use `${matrix.*}` and `${cell.*}`.
- Features contribute to named steps after bundle expansion:

```yaml
features:
  http2:
    pipeline:
      - at: cmake/configure
        inputs:
          args:
            append:
              - -DCURL_USE_NGHTTP2=ON
```

This is the one place where ordering matters, and it is explicit: a contribution targets an expanded step by name, merges into its typed inputs with `set`, `append` or `prepend`, and cannot insert a new positional step. A feature that cannot express its change this way needs a separate output or a separate recipe, not a conditional.

## 11. Tests

Tests are still a separate section with their own pipeline, dependencies and
root. They may sit at the recipe level or under an output:

```yaml
outputs:
  - name: python-foo
    tests:
      - name: import
        stage: installed
        packages: [python3-pytest]
        pipeline:
          - uses: python/import
            with:
              python: /usr/bin/python3.10
              module: foo
```

Build-stage tests (`stage: build`) may reuse the build pipeline's environment.
Installed-stage tests get a fresh root with the selected built RPMs and
declared test dependencies. A recipe-level test installs every output; an
output-level test installs its output and the sibling outputs needed by its
runtime dependencies. Network is off in both. Use `--test OUTPUT/NAME` when a
test name is shared by several outputs.

Test pipelines use the same `Step` type, the same step library and the same
`pipelines/` directory as build. A local step used by both resolves to the same
file and the same digest.

There is one resolver for all three sections. `source-assembly.pipeline`,
`build.pipeline` and `tests[].pipeline` call the same `plan_steps()` against the
same built-in registry and the same local step directory. The section changes
where the steps run, not how they are resolved.

## 12. Errors

| Code | Condition |
|---|---|
| `RPMX-STEP-001` | A step has no command, or does not name exactly one of `uses`, `argv`, or `runs`. |
| `RPMX-STEP-002` | A step's `argv` list is empty. |
| `RPMX-STEP-003` | A step's `runs` script is empty. |
| `RPMX-STEP-010` | A step name is not a valid namespaced name. |
| `RPMX-STEP-011` | A step file cannot be read, or the named step cannot be found. |
| `RPMX-STEP-012` | A step file is not UTF-8 or does not parse as a valid step. |
| `RPMX-STEP-013` | A step file's declared name does not match the requested name. |
| `RPMX-STEP-014` | A step is given an input it does not declare. |
| `RPMX-STEP-015` | A required step input is missing, or an input reference is unknown. |
| `RPMX-STEP-016` | A local step shadows a built-in step, or an input value has the wrong type. |
| `RPMX-STEP-017` | A step has an empty `argv`, or specifies both or neither of `argv` and `runs`. |
| `RPMX-STEP-018` | A step requests an unknown template renderer. |
| `RPMX-STEP-019` | A step definition hardcodes a guest path. |

## 13. External tools and built-in steps

The recipe names a **built-in step**; it does not name the tool, its version or
its image.

```yaml
source-assembly:
  pipeline:
    - uses: go/bump
      with:
        deps:
          - module: golang.org/x/net
            version: v0.33.0
            reason: CVE-2024-45337
            advisory: https://pkg.go.dev/vuln/GO-2024-3333
    - uses: go/vendor
```

The step library defines what `go/bump` needs: Go, `omnibump`, and a writable
workspace. The builder image or the active profile supplies them. The lock
records the resolved tool version and digest; the recipe stays intent-only.

Built-in source steps in v1:

| Step | What it does | Tools it needs |
|---|---|---|
| `go/bump` | Renders the deps file, runs omnibump, records the module diff. | `go`, `omnibump` |
| `go/vendor` | Runs `go mod vendor` and records `vendor/modules.txt`. | `go` |
| `cargo/vendor` | Runs `cargo vendor`. | `cargo` |
| `npm/ci` | Populates `node_modules` from the lock, offline. | `npm` |

A step is an external command, not a linked library or an in-process plugin.
This keeps Go components available without giving `rpmx` a Go runtime, a Python
interpreter or a cgo dependency.

Rules:

- The recipe must not carry an OCI reference, a tool digest or a tool locator
  for a built-in step. Those are toolchain distribution policy.
- The lock records every resolved tool (`name`, `version`, `digest`). The
  manifest records the argv, exit code, workspace digest before/after and the
  stdout digest.
- Tools are mounted read-only and run in the active sandbox. Source assembly and
  build have no external network.
- Exit 0 is success; any other exit fails the step. Stderr is a log. Stdout may
  be JSON if the step declares a result schema.
- Tools are never built from source inside the package build. A separate
  reproducible tool-build pipeline publishes them into the builder image.
- `source-assembly.tools` remains as an **advanced override** for a tool that is
  not in the builder image: it is pinned by digest there, and the plan marks it
  as recipe-resolved rather than builder-resolved. Do not use it for
  `omnibump`; that is what the built-in step is for.

`rpmx fetch` caches the modules for the bumped versions while the proxy is
allowed. `rpmx source` runs `go/bump` and `go/vendor` offline and records
`go.mod`/`go.sum` before and after, the selected versions, the advisory IDs and
the resolved tool digest. See
[`06-build-inputs.md`](06-build-inputs.md#511-bumping-go-modules-for-cve-remediation).

## 14. What this costs

- **The planner reasons less.** It cannot know that `cmake/install` used `DESTDIR`; it can only check the staged result. That is acceptable because the manifest is the real check.
- **Bad steps are shared bad steps.** A flawed `python/install-wheel` affects every user, like a flawed eclass or macro. Mitigation: step review, digest pinning, and a small library.
- **More YAML per recipe.** A simple package lists three or four `uses` lines instead of `system: cmake`. In exchange, the recipe shows what it does, and `rpmx plan` shows the commands.
- **Melange familiarity.** Packagers who know melange can read the pipeline immediately. That is a feature, not an accident.

## 15. Open questions

1. Should a step be able to declare a **file claim** (for example, "installs into `${stage}${libdir}`") so the planner can predict ownership before running? (Current: no; the staged tree is the truth.)
2. Should bundles be built in or fetched from a registry? (Current: built in, with pinned third-party libraries as a later option.)
3. Should `uses` support a version range, or only an exact version? (Current: exact; the lock pins the digest.)
4. Should steps be able to produce **named artifacts** for other steps, or is the shared staging root enough? (Current: shared root.)
5. Do we need `with` to support structured values beyond scalars and `list<string>`? (Current: no.)
6. How do we review a step library change without re-reading every recipe that uses it? (Digest pinning makes the change visible; a changelog and `rpmx explain --diff` would help.)
