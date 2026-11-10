# Worked plan: python-matrix

Illustrative, not executed.

## 1. Matrix expansion

```
python=3.10 -> slug py310 -> python3.10-foo
python=3.11 -> slug py311 -> python3.11-foo
python=3.12 -> slug py312 -> python3.12-foo
```

Three cells, one shared source snapshot, three SRPMs:

```
foo-1.0.0-1.py310.src.rpm
foo-1.0.0-1.py311.src.rpm
foo-1.0.0-1.py312.src.rpm
```

The release suffix keeps the source-package NEVRAs unique. The binary
packages use the Python-specific names.

## 2. Per-cell plan

| Cell | Interpreter | Build deps | Output |
|---|---|---|---|
| `python=3.10` | `/usr/bin/python3.10` | `python3.10-devel`, `python3-pip`, plus the explicitly selected `python3-build` and `python3-pip` from step requirements | `python3.10-foo` |
| `python=3.11` | `/usr/bin/python3.11` | same, for 3.11 | `python3.11-foo` |
| `python=3.12` | `/usr/bin/python3.12` | same, for 3.12 | `python3.12-foo` |

The Fedora example explicitly selects its build and pip packages through step inputs.
The recipe declares its Setuptools backend in `build.packages`.
Without overrides, the Python steps require Eternal's `python-build` and `python-pip`.

`rpmx matrix plan` prints this before anything is built.

## 3. Source assembly

The source snapshot is shared:

```
fetch foo-1.0.0.tar.gz
verify sha512
source snapshot digest: sha512:...
```

No Python-specific source generation is needed. If the project used C
extensions, source assembly would still be shared; the build would compile per
cell.

## 4. Build

The recipe names two steps:

```yaml
pipeline:
  - uses: python/build-wheel
    with:
      python: /usr/bin/python${matrix.python.version}
  - uses: python/install-wheel
    with:
      python: /usr/bin/python${matrix.python.version}
      prefix: /usr
```

For the `python=3.10` cell, `rpmx plan` expands them to:

```
step 1  python/build-wheel  (library 1, sha512:...)
        cwd  /build/foo-1.0.0
        argv /usr/bin/python3.10 -m build --wheel --no-isolation --outdir dist
step 2  python/install-wheel  (library 1, sha512:...)
        cwd  /build/foo-1.0.0
        argv /usr/bin/python3.10 -m pip install --root /stage --prefix /usr \
               --no-index --no-deps dist/foo-1.0.0-py3-none-any.whl
```

The 3.11 and 3.12 cells are identical except for the interpreter and the
output path. Each cell has its own build root and its own resolved
dependencies; the source snapshot is shared.

The output selector covers the package and its metadata:

```yaml
files:
  include:
    - "${matrix.python.site}/foo/**"
    - "${matrix.python.site}/foo-*.dist-info/**"
```

Dropping `.dist-info` would still pass `import foo`, but it would break
`importlib.metadata`, entry points and `Requires-Dist`.

## 5. Tests

```
installed: /usr/bin/python3.10 -c "import foo"
installed: /usr/bin/python3.11 -c "import foo"
installed: /usr/bin/python3.12 -c "import foo"
```

Each test installs only its own output plus that interpreter's runtime. A
missing interpreter or `-devel` package fails before the build.

## 6. Lock

The lock records the shared source plus per-cell entries:

```json
{
  "matrix": {
    "axes": {"python": ["3.10", "3.11", "3.12"]},
    "cells": [
      {"id": "python=3.10", "slug": "py310", "release": "1.py310",
       "outputs": ["python3.10-foo"], "srpm_digest": "sha512:..."},
      {"id": "python=3.11", "slug": "py311", "release": "1.py311",
       "outputs": ["python3.11-foo"], "srpm_digest": "sha512:..."},
      {"id": "python=3.12", "slug": "py312", "release": "1.py312",
       "outputs": ["python3.12-foo"], "srpm_digest": "sha512:..."}
    ]
  }
}
```

## 7. What this demonstrates

- A matrix is data, not `%if` conditionals.
- Each cell has a distinct NEVRA, output manifest, dependency set and SRPM.
- The source snapshot is shared and content-addressed.
- `--matrix python=3.11` builds one cell; release builds build all locked cells.
- Naming collisions are planner errors, not late RPM build failures.
