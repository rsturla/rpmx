# Templates and variables

## 1. Goal

Templates let a recipe reuse values across package identity, outputs, dependencies, paths, pipeline inputs and tests without becoming a macro language.

The rule is:

> **Templates are variable substitution over a closed, typed scope. They are not expressions and they are not code.**

No environment lookups, no shell, no network, no arithmetic, no filters, no conditionals.

## 2. Variable scope

| Variable | Type | Meaning |
|---|---|---|
| `${package.name}` | string | Source package name. |
| `${package.epoch}` | integer | Epoch. |
| `${package.version}` | string | Version. |
| `${package.release}` | string | Release, including any matrix suffix. |
| `${package.license}` | string | SPDX license expression. |
| `${profile.name}` | string | Profile name. |
| `${profile.vendor}` | string | Profile vendor. |
| `${profile.dist_tag}` | string | Distro tag. |
| `${profile.prefix}` | string | Install prefix. |
| `${profile.libdir}` | string | Library directory. |
| `${arch}` | string | Target architecture. |
| `${target}` | string | Target triple. |
| `${cell.id}` | string | Matrix cell ID, e.g. `python=3.12`. |
| `${cell.slug}` | string | Matrix cell slug, e.g. `py312`. |
| `${matrix.<axis>}` | scalar | Scalar matrix value. |
| `${matrix.<axis>.<field>}` | scalar | Map field from a matrix value. |
| `${vars.<name>}` | scalar | User-defined variable. |
| `${inputs.<name>}` | scalar or `list<string>` | Step input, available only inside a step definition. |
| `${source.dir}` | string | Extracted primary source directory. |
| `${build.dir}` | string | Build directory. |
| `${stage}` | string | Staging root. |
| `${prefix}` | string | Install prefix. |
| `${libdir}` | string | Library directory. |
| `${jobs}` | string or `auto` | Parallelism. |

The scope is layered. A recipe sees `package.*`, `profile.*`, `arch`, `target`, `cell.*`, `matrix.*` and `vars.*`. A step definition additionally sees `inputs.*` and the executor paths. A step cannot read `matrix.*` or `vars.*` directly; its inputs are the only channel, which is what makes a step reusable across recipes.

`${profile.dist_tag}` is the normalized tag in effect (for example `.fc44`), whether it came from a profile or from `--dist-tag`; it is the empty string when no tag is in effect. The build-system counter is not a template variable: it is passed with `--build-release` and appears only through `${package.release}`.

A `list<string>` input may be bound to a list value directly (for example `args: "${inputs.args}"` inside a bundle). String interpolation always produces a string and is never used to build a list.

The short forms `${name}`, `${epoch}`, `${version}` and `${release}` are aliases for `package.*`. They exist because dependency strings read better as `foo = ${version}-${release}`.

## 3. User variables

```yaml
vars:
  py_tag: "${matrix.python.tag}"
  py_package: "python${matrix.python.version}"
  py_site: "${matrix.python.site}"
```

Rules:

- Values are scalars: strings, integers or booleans.
- A variable may reference matrix values, profile values, package values and other variables.
- Expansion is a fixed point with cycle detection.
- A variable may not reference itself, directly or indirectly.
- `vars` are recipe-local; profiles can override them via the profile overlay.
- There are no lists, maps or expressions in `vars` in v1. If a value needs structure, it belongs in the recipe, not in a template.

## 4. Where templates are allowed

Allowed:

- `package.name` (subject to validation and unique NEVRA);
- `package.version` and `package.release` (subject to validation);
- `summary` and `description`;
- output names, summaries and descriptions;
- dependency atoms;
- pipeline step inputs, arguments and environment values;
- file selectors and file attribute paths;
- test names and `argv` entries;
- patch file paths and reasons (not `when` semantics).

Not allowed:

- `schema`;
- matrix axis keys and values;
- feature names and `requires`/`conflicts` constraints;
- source `digest`, `locators`, `signature` or source type;
- action `type` and `stage`/`kind` enums;
- profile overlay paths;
- anything that would change the meaning of the schema itself.

The source identity must be concrete before expansion. A template in a digest or locator would make the trust model circular.

## 5. Expansion order

1. Parse and validate the static recipe.
2. Resolve matrix cells from static axes and exclusions.
3. Bind `matrix.*`, `cell.*`, `profile.*` and `arch`/`target`.
4. Expand `vars` to a fixed point with cycle detection.
5. Expand `package.name`, `package.version`, `package.release`.
6. Validate the expanded package identity and NEVRA uniqueness.
7. Resolve features for the cell.
8. Expand outputs, dependencies, pipeline inputs, files, actions and tests.
9. Validate the expanded cell against the schema.
10. Fail if any `${...}` token remains or any variable is unbound.

The expanded recipe is the plan input. The original template is the source of truth.

## 6. Package names and versions

Package identity may be templated:

```yaml
package:
  name: "${vars.base_name}-${matrix.python.tag}"
  version: "${vars.upstream_version}"
  release: "${vars.release}.${cell.slug}"
```

After expansion, the normal RPM validation applies:

- name and version syntax;
- release syntax;
- unique NEVRA across matrix cells;
- no `-` where RPM forbids it;
- no empty components.

If `package.version` varies by matrix, each cell has a different source version. That is allowed, but it means the matrix cells do **not** share a source snapshot. The lock records a source snapshot per cell. Most matrices should vary package names/releases and build dependencies, not the upstream version.

## 7. Examples

### Python matrix

```yaml
vars:
  py_tag: "${matrix.python.tag}"
  py_package: "python${matrix.python.version}"

outputs:
  - name: "${vars.py_package}-foo"
    summary: Foo for Python ${matrix.python.version}
    files:
      include:
        - "${matrix.python.site}/foo/**"
    dependencies:
      run:
        - "${vars.py_package}"
```

### Release suffix

```yaml
package:
  release: "1.${cell.slug}"
```

or let the matrix add it automatically:

```yaml
matrix:
  release-suffix: true
```

### Dependency template

```yaml
build:
  dependencies:
    host:
      - "${matrix.python.package}-devel"
```

## 8. Escaping

Use `$${...}` for a literal `${...}`:

```yaml
description: |
  The shell fragment $${HOME} is expanded by the build, not by rpmx.
```

The template engine does not interpret shell syntax. `$${...}` is the only escape.

## 9. Validation and safety

Fail on:

- an unknown variable;
- an unknown matrix axis or field;
- a cycle in `vars`;
- a `${...}` token left after expansion;
- a variable used in a forbidden field;
- a template result that breaks RPM name/version/release syntax;
- duplicate output names or NEVRAs after expansion;
- expansion longer than a configured limit;
- more than `max-cells` matrix cells.

Templates never read:

- environment variables;
- the host filesystem;
- the network;
- the clock;
- random values.

They are a pure function of the recipe, lock, profile and matrix cell.

## 10. Diagnostics and audit

`rpmx explain` should show:

```
package.name:
  template: "${vars.base_name}-${matrix.python.tag}"
  bindings:
    vars.base_name: foo
    matrix.python.tag: "312"
  result: foo-312
```

The manifest records template bindings per cell. The lock records the expanded identity and digests. A reviewer can see what was substituted without reading a macro expansion dump.

## 11. Prior art: melange templating

Melange already ships a template layer worth learning from:

| Melange feature | What it does | Relationship to this design |
|---|---|---|
| `${{package.name}}`, `${{package.version}}`, `${{package.epoch}}`, `${{package.full-version}}` | Built-in package identity variables. | Same idea as `${package.*}`. |
| `${{package.srcdir}}`, `${{targets.outdir}}`, `${{targets.destdir}}`, `${{targets.subpkgdir}}` | Built-in build-layout paths. | Replaced by `build.*`/staging paths, which should be defined rather than hard-coded in pipelines. |
| `${{subpkg.name}}`, `${{context.name}}` | Current output identity. | Replaced by `cell.*` and output-scoped values; RPM outputs are a first-class list, not a second parse. |
| `${{build.arch}}`, `${{build.goarch}}` | Target architecture. | `${arch}` and `${target}`. |
| `vars:` map | User-defined scalars available as `${{vars.foo}}`. | Same idea as `${vars.*}`, with fixed-point expansion and cycle detection. |
| `data:` + `${{range.key}}`/`${{range.value}}` | Named matrix axis consumed by `subpackages: - range:`. | Generalized to typed matrix axes and `${matrix.*}`; see [`11-matrices.md`](11-matrices.md). |
| `var-transforms:` | Regex-based creation of new variables from existing ones. | **Deliberately not adopted.** Regex rewriting is a macro that hides a version-mangling rule; explicit matrix fields and `vars` are checkable. |

Melange applies templates to subpackages, pipelines and tests. This design applies them to package identity as well, which melange does not do: `package.name` is concrete in a melange file. That matters for matrices, where a cell may need a different package name.

The main difference is not the syntax but the scope guarantee. Melange's `${{...}}` is a build-time interpolation in a YAML document that also contains shell; `var-transforms` can rewrite any source variable. This design keeps expansion to a closed, typed scope and forbids templates in the fields that define the build graph or the source identity.

## 12. Open questions

1. Should `package.version` be allowed to vary by matrix, or only by `vars`/profile? (Current design: allowed, but then sources are per cell.)
2. Should templates support simple conditionals like `${?feature:...}`? (No; use features and separate outputs.)
3. Should `vars` be global, per-output, or both? (Current design: global with profile override.)
4. Should templates be allowed in `summary`/`description`? (Yes, but they should not be used for dynamic prose that belongs in the adapter.)
5. Should there be typed interpolation for integers/booleans, or always string substitution followed by parsing? (Current design: strings, validated after expansion.)
