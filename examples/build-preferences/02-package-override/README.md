# Example 2: package override

**Complexity:** medium. **Mechanism:** one distro directory plus one short
per-package file.

## Scenario

`libfoo` is an Autotools library with a mutually exclusive TLS backend
(`ssl` or `gnutls`) and an optional `idn` capability. Acme Linux needs a
CA-bundle patch, a FIPS configure argument, a post-check step and an extra
hardening flag. Community Linux selects GnuTLS and needs no package override.

The recipe never mentions Acme, Autotools slots or FIPS.

## Files

| File | Purpose |
|---|---|
| `package.yaml` | Distro-neutral recipe with features and a TLS choice |
| `distros/fedora-44/distro.yaml` | Shared base |
| `distros/acme/distro.yaml` | Acme's distro-wide preferences |
| `distros/community/distro.yaml` | Community's distro-wide preferences |
| `distros/acme/packages/libfoo.yaml` | Acme's entire libfoo change |
| `distros/acme/packages/libfoo/0001-ca-bundle.patch` | Acme's patch |
| `patches/0001-libfoo-pkgconfig-prefix.patch` | Recipe-owned shared patch |

## The entire Acme libfoo change

```yaml
document: rpmx-package-override
version: 1
package: libfoo

patches:
  - file: libfoo/0001-ca-bundle.patch
    reason: Use the Acme CA bundle path
    resolves: [CVE-2024-1234]

flags:
  cflags:
    append: [-D_FORTIFY_SOURCE=3]

configure:
  args:
    append: [--enable-fips]

after:
  check:
    - name: fips-postcheck
      argv: [/usr/libexec/fips-postcheck, ${stage}/usr/lib/libfoo.so.2]
```

That is the whole file. No digest to paste, no `slot:configure`, no `pipeline:
modify`, no delta document.

## Add it with one command

```console
$ rpmx distro override acme libfoo
created distros/acme/packages/libfoo.yaml
```

Then edit the skeleton, or use the targeted commands:

```console
$ rpmx distro patch acme libfoo 0001-ca-bundle.patch \
    --reason "Use the Acme CA bundle path"
$ rpmx distro configure acme libfoo --append --enable-fips
$ rpmx distro after acme libfoo check \
    --name fips-postcheck --argv /usr/libexec/fips-postcheck
```

Each command validates the phase and prints the plan diff before writing.

## Expected transcript

Plan Acme's build:

```console
$ rpmx plan --distro acme

distro     acme (.acme1)  extends fedora-44
features   ssl, tls
override   distros/acme/packages/libfoo.yaml

patches
  0001-libfoo-pkgconfig-prefix.patch   recipe
  0001-ca-bundle.patch                 override acme/libfoo
                                       reason "Use the Acme CA bundle path"

configure  args --with-openssl --enable-fips
           base recipe + feature ssl + override acme/libfoo
flags      cflags -O2 -g -pipe -D_FORTIFY_SOURCE=3
after      check -> fips-postcheck (override acme/libfoo)
```

Community uses the same recipe with no override:

```console
$ rpmx plan --distro community --with idn

distro     community (.com1)
features   gnutls, tls, idn
override   none

configure  args --with-gnutls --with-libidn2
```

Compare the two:

```console
$ rpmx diff --distro acme --distro community

~ features   ssl -> gnutls
             + idn (cli)
~ configure  --with-openssl -> --with-gnutls --with-libidn2
             + --enable-fips (override acme/libfoo)
~ cflags     -D_FORTIFY_SOURCE=3 removed
+ patches    0001-ca-bundle.patch removed
```

An invalid choice is rejected before the build:

```console
$ rpmx plan --distro acme --with gnutls

error[RPMX-FEATURE-014]: choice 'tls' accepts one of ssl, gnutls
  ssl     distro acme: features.enable
  gnutls  cli --with gnutls
  help: rpmx plan --distro acme --without ssl --with gnutls
```

## If the recipe has no configure phase

The override has no target for `configure.args`. The tool says so and suggests
the two easy alternatives:

```console
$ rpmx plan --distro acme

error[RPMX-OVERRIDE-004]: libfoo has no configure phase
  override  distros/acme/packages/libfoo.yaml: configure.args
  recipe    package.yaml: uses autotools
  help: use `after: check` to add a step, or `rpmx distro fork acme libfoo`
        to change the procedure
```

The packager does not need to know that Autotools maps `configure` to
`autotools/configure`. If the recipe uses CMake or Meson, `configure.args`
still works. If the recipe has no such phase, the error names the alternatives.

## Why this is easier than the previous delta

| Previous | Now |
|---|---|
| `deltas/acme/libfoo.yaml` | `distros/acme/packages/libfoo.yaml` |
| `at: slot:configure` + `inputs.args.append` | `configure.args.append` |
| `pipeline.insert.after: slot:check` | `after.check` |
| `recipe-digest` pasted by hand | recorded in the lock; `--locked` catches drift |
| `--delta` selection | auto-discovered by package name |

## Review questions

1. Is `configure:` obvious enough, or does it hide too much?
2. Is `after: check:` the natural way to add a validation step?
3. Should the override file include a `package:` field, or should the filename
   be the only identity?
4. Is auto-discovery by package name the right default, or should `plan` require
   `--override libfoo`?
5. Are the `rpmx distro patch` / `configure` / `after` commands useful, or is
   editing the short YAML faster?
