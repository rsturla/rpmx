# Build matrices

> Historical design record. Some schema and execution examples describe earlier
> proposals. Use [current recipes](../usage/recipes.md) and
> [testing](../usage/testing.md) for the supported schema 2 interface.

## 1. What a matrix is

A matrix is a set of **build cells** produced from the Cartesian product of typed axes.

For a Python library:

```yaml
matrix:
  axes:
    python:
      - value: "3.10"
        version: "3.10"
        tag: "310"
        slug: py310
        package: "python3.10"
        site: /usr/lib/python3.10/site-packages
      - value: "3.11"
        version: "3.11"
        tag: "311"
        slug: py311
        package: "python3.11"
        site: /usr/lib/python3.11/site-packages
      - value: "3.12"
        version: "3.12"
        tag: "312"
        slug: py312
        package: "python3.12"
        site: /usr/lib/python3.12/site-packages
```

That expands to three build cells:

```
python=3.10  -> python3.10-foo
python=3.11  -> python3.11-foo
python=3.12  -> python3.12-foo
```

Each cell gets its own build root, dependency set, outputs and SRPM.

This generalizes melange's `data`/`range` expansion, which proves the output side of the idea but stops at subpackages. See [section 11](#11-prior-art-melanges-datarange-expansion).

## 2. Matrix vs features

They solve different problems:

| Concept | Use for | Example |
|---|---|---|
| **Feature** | Optional functionality inside one package | TLS backend, HTTP/2, LDAP support |
| **Matrix** | Different package identities or build environments | Python versions, ABI versions, variants with different names |

If two builds produce different package names and different dependency sets, use a matrix. If they produce the same package with optional code paths, use a feature.

A matrix and features can combine. The matrix is expanded first; features are resolved per cell.

## 3. Syntax

```yaml
matrix:
  axes:
    python:
      - value: "3.10"
        version: "3.10"
        tag: "310"
        slug: py310
        package: "python3.10"
        site: /usr/lib/python3.10/site-packages
      - value: "3.11"
        version: "3.11"
        tag: "311"
        slug: py311
        package: "python3.11"
        site: /usr/lib/python3.11/site-packages
      - value: "3.12"
        version: "3.12"
        tag: "312"
        slug: py312
        package: "python3.12"
        site: /usr/lib/python3.12/site-packages
    ssl:
      - value: openssl
        package: openssl-devel
      - value: gnutls
        package: gnutls-devel
  exclude:
    - python: "3.10"
      ssl: gnutls
  max-cells: 32
```

Rules:

- Each axis has a list of values.
- A value is either a scalar or a map.
- A map value can define arbitrary fields for interpolation and derived names.
- A map value must define `value`, a scalar identity used by `${matrix.<axis>}`, `cell.id` and `exclude`. Other fields are payload.
- A value should declare `slug` when the release suffix or output name depends on it. If `slug` is absent, the planner derives `<axis>-<normalized value>` (for example `python-310`). The Wolfi-derived `py310` form is explicit, not inferred.
- `exclude` removes matching cells. Unspecified axes are wildcards.
- `max-cells` defaults to a small limit (for example 32). A recipe that exceeds it fails lint until it raises the limit explicitly.
- An axis must have at least one value.
- Every cell must have a unique `id` and `slug`.

## 4. Interpolation

Only a closed set of matrix variables is available:

```
${matrix.<axis>}             scalar axis value
${matrix.<axis>.<field>}     map field
${cell.id}                   stable cell ID, e.g. python=3.12,ssl=openssl
${cell.slug}                 filesystem/NEVRA-safe slug, e.g. py312-openssl
```

No expressions, no functions, no conditionals. If a value needs a branch, use a separate axis value or an explicit `exclude`.

Interpolation is allowed in:

- output names;
- summaries/descriptions where needed (discouraged);
- dependency strings;
- pipeline step inputs and arguments;
- file selectors;
- test names and arguments;
- patch `when` conditions? No; matrix is resolved before features, so use `exclude` for matrix validity.

## 5. Naming and NEVRA

RPM requires unique NEVRAs. Two matrix cells cannot produce two SRPMs with the same `Name-Version-Release`.

The matrix expands the package **release** with the cell slug by default:

```
foo-1.0-1.py310.src.rpm
foo-1.0-1.py311.src.rpm
foo-1.0-1.py312.src.rpm
```

A profile's `dist-tag` is appended **after** the cell slug, and a build-system
counter is appended last:

```
foo-1.0-1.py310.fc44+build7.src.rpm
```

The binary outputs use explicit names:

```yaml
outputs:
  - name: "${matrix.python.package}-foo"
    files:
      include:
        - "${matrix.python.site}/foo/**"
    dependencies:
      run:
        - "${matrix.python.package}"
```

The release suffix is policy:

```yaml
matrix:
  release-suffix: true          # default: append .${cell.slug}
  # release-suffix: false       # only allowed if cell NEVRAs remain unique
```

If two cells produce the same output name, the planner fails before building.

## 6. Per-cell build inputs

The matrix can vary:

```yaml
build:
  pipeline:
    - uses: python/build-wheel
      with:
        python: /usr/bin/python${matrix.python.version}
    - uses: python/install-wheel
      with:
        python: /usr/bin/python${matrix.python.version}
        prefix: /usr
  dependencies:
    tools:
      - "${matrix.python.package}-devel"
      - python3-pip
    tests:
      - "${matrix.python.package}-pytest"
```

The source snapshot is shared. Each cell gets its own:

- resolved build dependencies;
- build root/builder image;
- pipeline step inputs;
- output manifest;
- SRPM;
- SBOM/provenance;
- test run.

This is where the matrix differs from a macro. The planner can show exactly which dependencies and files belong to each cell before building.

## 7. Lockfile

One lock records the matrix and every expanded cell:

```json
{
  "matrix": {
    "axes": {
      "python": ["3.10", "3.11", "3.12"]
    },
    "cells": [
      {
        "id": "python=3.10",
        "slug": "py310",
        "release": "1.py310",
        "srpm_digest": "sha512:...",
        "outputs": ["python3.10-foo"]
      },
      {
        "id": "python=3.11",
        "slug": "py311",
        "release": "1.py311",
        "srpm_digest": "sha512:...",
        "outputs": ["python3.11-foo"]
      }
    ]
  }
}
```

The source snapshot digest is shared. Dependencies/toolchain are per cell.

## 8. CLI

```
rpmx matrix list                 # show axes, cells, exclusions
rpmx matrix plan                 # show per-cell outputs/dependencies
rpmx build                       # build all cells
rpmx build --matrix python=3.11  # build one axis value
rpmx build --matrix python=3.11,3.12
rpmx build --cell python=3.11
rpmx build --matrix all
```

`rpmx build` builds all declared cells by default. Filtering is a local convenience; release builds should use the lock and build the full matrix.

## 9. Validation

The planner must fail if:

- a cell ID or slug collides;
- two cells produce the same output name;
- two SRPMs would have the same NEVRA;
- an `exclude` entry matches every cell;
- an axis is empty;
- the matrix exceeds `max-cells`;
- an interpolation references an unknown axis or field;
- a cell's dependency set cannot be resolved;
- a cell's file manifest is empty, unless it is a metapackage;
- a matrix output file overlaps another output in the same cell.

## 10. Python example

```yaml
schema: 1

package:
  name: foo
  version: "1.0.0"
  release: 1
  summary: An example Python library
  license: MIT

build:
  pipeline:
    - uses: python/build-wheel
      with:
        python: /usr/bin/python${matrix.python.version}
    - uses: python/install-wheel
      with:
        python: /usr/bin/python${matrix.python.version}
        prefix: /usr
  dependencies:
    tools:
      - "${matrix.python.package}-devel"
      - python3-pip
    tests:
      - "${matrix.python.package}-pytest"

sources:
  - name: foo
    kind: pypi
    package: foo
    version: "1.0.0"
    digest: "sha512:..."
    locators:
      - kind: https
        url: https://files.pythonhosted.org/...

matrix:
  axes:
    python:
      - value: "3.10"
        version: "3.10"
        tag: "310"
        slug: py310
        package: "python3.10"
        site: /usr/lib/python3.10/site-packages
      - value: "3.11"
        version: "3.11"
        tag: "311"
        slug: py311
        package: "python3.11"
        site: /usr/lib/python3.11/site-packages
      - value: "3.12"
        version: "3.12"
        tag: "312"
        slug: py312
        package: "python3.12"
        site: /usr/lib/python3.12/site-packages

outputs:
  - name: "${matrix.python.package}-foo"
    summary: Python library for ${matrix.python.version}
    files:
      include:
        - "${matrix.python.site}/foo/**"
    dependencies:
      run:
        - "${matrix.python.package}"

tests:
  - name: import-${matrix.python.tag}
    stage: installed
    argv: ["/usr/bin/python${matrix.python.version}", "-c", "import foo"]
```

## 11. Prior art: melange's `data`/`range` expansion

This design is not invented from nothing. Melange has a working, production matrix mechanism, and Wolfi uses it for every Python library.

A melange recipe declares a named data axis and expands subpackages from it:

```yaml
data:
  - name: py-versions
    items:
      3.10: '310'
      3.11: '311'
      3.12: '312'
      3.13: '313'

subpackages:
  - range: py-versions
    name: py${{range.key}}-${{vars.pypi-package}}
    dependencies:
      runtime:
        - py${{range.key}}-charset-normalizer
        - py${{range.key}}-urllib3
      provider-priority: ${{range.value}}
    pipeline:
      - uses: py/pip-build-install
        with:
          python: python${{range.key}}
    test:
      pipeline:
        - uses: python/import
          with:
            python: python${{range.key}}
            import: ${{vars.pypi-package}}
```

That is a true matrix over **outputs**. Each row gets:

- a distinct subpackage name (`py3.10-requests`, `py3.11-requests`, ...);
- its own runtime dependencies;
- its own build pipeline, parameterized by `${{range.key}}`;
- its own tests.

A companion metapackage (`py3-supported-requests`, with the per-version packages listed explicitly) depends on every row. Melange's `NEWS.md` describes the feature as "meta-programming with data and range specifiers", and notes that at the time it applied only to subpackages but could be extended to pipeline elements.

What `rpmx` adds is **cell-shaped** matrix expansion:

| Property | melange `data`/`range` | `rpmx` matrix |
|---|---|---|
| Expanded unit | subpackage entries | build cells, then outputs |
| Per-row build root / toolchain | No, one shared environment | Yes, resolved per cell |
| Per-row source package | No, one APK build | Yes, one SRPM per cell |
| Multiple axes | One `range:` per subpackage entry; no documented Cartesian product | Cartesian product of typed axes |
| Exclusion and limits | None | `exclude`, `max-cells` |
| Interpolation | `${{range.key}}`, `${{range.value}}`, `vars` | `${matrix.*}`, `${cell.*}`, `${vars.*}`, fixed point |
| Expansion target | Subpackages | Package identity, outputs, dependencies, pipeline inputs, files, tests |
| Regex rewriting | `var-transforms` | Not supported; explicit fields instead |

The distinction matters for RPM because a binary RPM's build dependencies are resolved per package, not per file. Two Python versions need two build roots and two dependency closures; melange can get away with one shared environment because Wolfi ships a single `py3-supported-python` meta-environment, but that is a Wolfi policy, not a general rule. For arbitrary RPM targets, a cell must be able to carry its own toolchain, and therefore its own SRPM.

The two models also differ in identity. Melange rows differ by subpackage name inside one source build. RPM has a source NEVRA as well as binary NEVRAs, so a cell that changes build inputs must have its own source package to be reproducible on its own.

## 12. Open questions

1. Should a matrix cell always get its own SRPM, or should a matrix SRPM contain all cells? (**Decided: one SRPM per cell.** Sources are deduplicated by the CAS.)
2. Should `rpmx build` default to all cells or a declared default subset? (Current design: all cells; `--matrix` filters.)
3. How do we model a default Python alias such as `python3-foo`? (Separate output or an explicit matrix value; not automatic.)
4. Should matrix axes be allowed to vary architecture? (Probably not; architecture is a build target, not a package variant.)
5. How do we represent cells that require different source snapshots? (That is not one matrix; it is separate recipes or a source-generation axis with explicit lock entries.)
6. Should matrix release suffixes be `.py310` or `~py310`? (Use `.`; `~` sorts earlier and is wrong for a rebuild variant.)
