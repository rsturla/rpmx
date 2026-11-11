# Example 3: fork

**Complexity:** complex. **Mechanism:** one fork recipe under the distro
directory.

## Scenario

`toolbox` is built by one opaque `build-everything` script. There is no
configure phase, no compile phase and no install phase. Acme Linux needs a FIPS
build with a different procedure, an installed helper, a post-check step and an
extra output. Wolfi-like Linux uses the upstream recipe unchanged.

This is the “what if `cmake/configure` isn’t used?” case. There is nothing
semantic to target. The tool should make the fork easy, not invent a way to
patch inside an opaque script.

## Files

| File | Purpose |
|---|---|
| `package.yaml` | Upstream recipe: one opaque `build-everything` step |
| `distros/fedora-44/distro.yaml` | Shared base |
| `distros/acme/distro.yaml` | Acme distro-wide preferences |
| `distros/wolfi/distro.yaml` | Wolfi-like preferences, no fork |
| `distros/acme/packages/toolbox/package.yaml` | Acme’s fork: a normal recipe |

## Try the easy path first

```console
$ rpmx distro override acme toolbox
created distros/acme/packages/toolbox.yaml
```

Add the change:

```yaml
configure:
  args: [--enable-fips]
```

The tool refuses, because there is no configure phase to attach it to:

```console
$ rpmx plan --distro acme

error[RPMX-OVERRIDE-004]: toolbox has no configure phase
  override  distros/acme/packages/toolbox.yaml: configure.args
  recipe    package.yaml: step build-everything has no phase
  help: use `after: build-everything` to add a step, or
        `rpmx distro fork acme toolbox` to change the procedure
```

An `after:` step is enough for an extra check. It cannot change what happens
inside `build-everything`. This change is structural, so fork.

## Fork it

```console
$ rpmx distro fork acme toolbox
created distros/acme/packages/toolbox/package.yaml
warning: remove distros/acme/packages/toolbox.yaml; a fork and an override
         for the same package cannot both apply
```

The fork is a full, normal `package.yaml` under the distro directory. It is
selected by package name: you still run `rpmx build --distro acme`, and the
tool uses the fork instead of the upstream `package.yaml`.

```yaml
schema: 2
package:
  name: toolbox
  version: "3.1.0"
  release: "1.fips"

build:
  pipeline:
    - uses: autotools
    - name: install-fips-helper
      argv: [install, -D, -m, "0755", tools/fips-postcheck,
             ${stage}/usr/libexec/fips-postcheck]
    - name: fips-postcheck
      argv: [${stage}/usr/libexec/fips-postcheck, ${stage}/usr/bin/toolbox]
    - uses: strip
...
outputs:
  - name: toolbox
  - name: toolbox-fips
```

There is no `pipeline: replace`, no delta document and no `identity` field in a
delta. The fork is visible in the distro directory, and the packager edits it
like any other recipe.

## Expected transcript

```console
$ rpmx plan --distro acme

distro     acme (.acme1)
fork       distros/acme/packages/toolbox/package.yaml
release    1.fips
pipeline
  uses autotools
  install-fips-helper   installs /usr/libexec/fips-postcheck
  fips-postcheck        runs the staged helper
  strip
outputs
  toolbox
  toolbox-fips
```

Wolfi-like uses the upstream recipe:

```console
$ rpmx plan --distro wolfi

distro     wolfi (.wolfi1)
recipe     package.yaml
pipeline
  build-everything   runs: make -f Makefile.dist && make check && make install
```

Compare them:

```console
$ rpmx diff --distro acme --distro wolfi

~ procedure  build-everything -> autotools + FIPS steps
~ release    1 -> 1.fips
+ output     toolbox-fips
~ flags      -Os -g0 -> -O2 -g -pipe
```

The fork is the whole story. There is no hidden delta warning, no “replace the
pipeline” escape hatch, and no chance of the two mechanisms disagreeing.

## The decision tree

| The change is… | Do this | Where |
|---|---|---|
| Distro-wide flags, feature defaults, policy | Edit the distro | `distros/<name>/distro.yaml` |
| One package: patch, flag, phase arg, extra step | Add an override | `distros/<name>/packages/<pkg>.yaml` |
| One package: different build procedure | Fork the package | `distros/<name>/packages/<pkg>/package.yaml` |
| Published alongside the original | Change release or output names in the fork | fork recipe |

## What this example tests

- An opaque recipe produces a clear error, not a silent no-op.
- `after:` can add a step, but cannot pretend to change an opaque script.
- The fork command removes the “which escape hatch?” question.
- A fork is a normal recipe, so there is no new vocabulary to learn.
- The same upstream recipe still builds for a distro without a fork.

## Review questions

1. Is `rpmx distro fork` easier to understand than `pipeline: replace`?
2. Should `distro fork` copy the upstream recipe automatically, or start from a
   blank recipe?
3. Is `release: "1.fips"` in the fork the right way to avoid a NEVRA collision,
   or should the tool derive it?
4. Should `after: build-everything` be the suggested fix for an opaque step, or
   should the tool always suggest a fork?
5. Is `distros/<name>/packages/<pkg>/package.yaml` the obvious location for a
   fork?
