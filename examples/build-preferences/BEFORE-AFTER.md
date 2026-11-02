# Before and after

The same Acme `libfoo` change, expressed in the previous model and in the
simplified model.

## Before: a delta targeting a semantic slot

```yaml
# deltas/acme/libfoo.yaml
document: rpmx-delta
version: 1
for: acme
package: libfoo
recipe-digest: sha512:...

patches:
  - file: deltas/acme/libfoo/0001-ca-bundle.patch
    reason: Use the Acme CA bundle path

pipeline:
  modify:
    - at: slot:configure
      inputs:
        args:
          append: [--enable-fips]
  insert:
    - after: slot:check
      name: fips-postcheck
      argv: [/usr/libexec/fips-postcheck, ${stage}/usr/lib/libfoo.so.2]

flags:
  cflags:
    append: [-D_FORTIFY_SOURCE=3]
```

Concepts the packager had to know: delta, slot, `modify`, `inputs`, `args`,
`append`, `insert`, `after`.

## After: a package override

```yaml
# distros/acme/packages/libfoo.yaml
document: rpmx-package-override
version: 1
package: libfoo

patches:
  - file: libfoo/0001-ca-bundle.patch
    reason: Use the Acme CA bundle path

configure:
  args:
    append: [--enable-fips]

after:
  check:
    - name: fips-postcheck
      argv: [/usr/libexec/fips-postcheck, ${stage}/usr/lib/libfoo.so.2]

flags:
  cflags:
    append: [-D_FORTIFY_SOURCE=3]
```

Concepts: package override, patch, configure, after, flags.

## Before: structural change as a delta

```yaml
# deltas/acme/toolbox.yaml
pipeline:
  replace:
    - name: configure
      slot: configure
      uses: autotools/configure
    ...
identity:
  release-suffix: fips
```

Concepts: delta, pipeline replace, slot, identity.

## After: structural change as a fork

```console
$ rpmx distro fork acme toolbox
created distros/acme/packages/toolbox/package.yaml
```

Then edit a normal `package.yaml`. No special replace syntax, no identity field
in a delta. If the fork ships beside the upstream package, change its `release`
or its output names in the fork recipe.

## Why this is easier

| Before | After |
|---|---|
| Two files per distro: profile + delta | One distro directory |
| A delta is separate from the distro | A package override is part of the distro |
| Target `slot:configure` | Write `configure:` |
| `pipeline: replace` escape hatch | `rpmx distro fork` |
| `--delta` to select a delta | Auto-discovered under `packages/` |
| Recipe names a profile | `--distro` selects the distro |

The flexibility is still there. It is just hidden behind three levels instead of
exposed as five concepts in every file.
