# Example 1: distro-only

**Complexity:** simple. **Mechanism:** one `distro.yaml` per distro.

## Scenario

`hello-color` is a tiny Makefile project with one optional capability: `color`,
which compiles in ANSI output. Acme wants optimisation and color. Tiny Linux
wants size, no color, and no test requirement.

Neither distro edits the recipe. Neither needs a package override.

## Files

| File | Purpose |
|---|---|
| `package.yaml` | Distro-neutral recipe; no root, no distro name |
| `distros/fedora-44/distro.yaml` | Shared base: repositories, root tools, policy |
| `distros/acme/distro.yaml` | Acme's 8-line delta from the base |
| `distros/tiny/distro.yaml` | Tiny's 8-line delta from the base |

## The entire Acme distro

```yaml
document: rpmx-distro
version: 1
name: acme
extends: fedora-44
dist-tag: .acme1

flags:
  cflags: [-O2, -g, -pipe]
  ldflags: [-Wl,-z,relro, -Wl,-z,now]

features:
  enable: [color]
  disable: [verbose]

policy:
  require-tests: true
  sbom: true
```

That is the whole file. Repositories, the root package list and the rest of the
policy come from `fedora-44`.

## Create a new distro

```console
$ rpmx distro init newdistro --from fedora-44
created distros/newdistro/distro.yaml
```

```yaml
document: rpmx-distro
version: 1
name: newdistro
extends: fedora-44
dist-tag: .new1
```

Then add only the preferences that differ:

```console
$ rpmx distro enable newdistro ssl
$ rpmx distro disable newdistro gnutls
$ rpmx distro set newdistro cflags -- '-O2 -g -pipe'
```

Each command validates the value, writes the file and prints the resulting plan
diff.

## Expected transcript

Discover what can change:

```console
$ rpmx distro show acme

distro    acme (.acme1)  extends fedora-44
flags     cflags  -O2 -g -pipe
          ldflags -Wl,-z,relro -Wl,-z,now
features  color    on   distro enable
          verbose  off  distro disable
policy    require-tests true
          sbom          true
packages  no overrides
```

Build with the distro defaults:

```console
$ rpmx build --distro acme

distro     acme (.acme1)
features   color
flags      cflags=-O2 -g -pipe -DUSE_COLOR
...
```

See another distro's result without building it:

```console
$ rpmx diff --distro acme --distro tiny

~ flags    cflags  -O2 -g -pipe  ->  -Os -g0
- color    distro tiny disables it
- tests    require-tests true -> false
~ sbom     true -> false
```

One-off override:

```console
$ rpmx build --distro tiny --with verbose

distro     tiny (.tiny1)
features   verbose
flags      cflags=-Os -g0 -DVERBOSE
```

The one-off is not written to the distro file. `--locked` treats it as an
assertion against the lock.

## What this example tests

- A distro file can be tiny because it extends a shared base.
- The recipe does not know which distro builds it.
- Flags, feature defaults and policy live in one place.
- No package override, no fork, no new vocabulary.
- `rpmx distro init` makes the first step one command.

## Review questions

1. Is an 8-line `distro.yaml` obviously enough for a new distro?
2. Is `extends: fedora-44` clear, or should the base be a path?
3. Is `rpmx distro set ...` useful, or is editing YAML faster?
4. Should `--distro` be required, or should `./distro.yaml` be the default?
