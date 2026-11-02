# Build-preference examples

These are **design sketches**, not buildable recipes. They use fields the
current `rpmx` does not implement yet.

## The easy version

One noun: **distro**.

```
distros/
  fedora-44/
    distro.yaml                  # distro-wide defaults, root, flags, policy
  acme/
    distro.yaml                  # extends fedora-44, changes what Acme cares about
    packages/
      libfoo.yaml                # Acme's per-package changes
      toolbox/
        package.yaml             # a fork: a full recipe for one package
```

Three levels, learned only when needed:

| Level | File | Use when |
|---|---|---|
| 1 | `distros/<name>/distro.yaml` | Flags, feature defaults, policy, root, dist tag. This is the 80%. |
| 2 | `distros/<name>/packages/<pkg>.yaml` | A patch, a configure argument, a flag, or an extra step for one package. |
| 3 | `distros/<name>/packages/<pkg>/package.yaml` | The package needs a different build procedure. A fork, not a delta. |

The recipe does not name a distro. The distro is selected at build time:

```console
rpmx build --distro acme
rpmx plan  --distro acme --with idn
rpmx diff  --distro acme --distro fedora-44
```

There is no `profile`, `delta`, `slot`, `variant` or `preset` in the
user-facing model. A package override is just part of the distro. A fork is
just a recipe in the distro directory.

## The commands that make it easy

```console
rpmx distro init acme --from fedora-44       # creates distros/acme/distro.yaml
rpmx distro enable acme ssl                  # edits distro.yaml
rpmx distro disable acme gnutls              # edits distro.yaml
rpmx distro override acme libfoo             # creates packages/libfoo.yaml skeleton
rpmx distro patch acme libfoo 0001-fix.patch # adds a patch with a reason prompt
rpmx distro fork acme toolbox                # creates packages/toolbox/package.yaml
rpmx distro show acme                        # what can change, and what is selected
```

The YAML is still the source of truth; the commands are the fast path. Every
command shows the resulting plan diff before writing.

## The three examples

| # | Example | Complexity | What it tests |
|---|---|---|---|
| 1 | [`01-distro-only/`](01-distro-only/) | Simple | Two distros change flags, feature defaults and policy with an 8-line `distro.yaml`. No package overrides. |
| 2 | [`02-package-override/`](02-package-override/) | Medium | A distro adds a patch, a configure argument, a flag and an extra step in one short package file. No slots, no delta. |
| 3 | [`03-fork/`](03-fork/) | Complex | The package has one opaque build script. A per-package override cannot reach inside it; `rpmx distro fork` creates a full recipe in the distro directory. |

Start with each example's `README.md`.

## What changed from the previous sketches

[`BEFORE-AFTER.md`](BEFORE-AFTER.md) shows the same libfoo change in the old
delta/slot model and in the new package-override model.

The short version:

- `profiles/` + `deltas/` became `distros/<name>/` + `packages/`.
- `delta` became “package override.”
- `slot:configure` became `configure:`.
- `pipeline: replace` disappeared. Structural change is `rpmx distro fork`.
- The recipe no longer names a profile or a build root; `--distro` supplies it.

## Feedback questions

1. Can a distro packager make the example 1 change without documentation?
2. Is `distros/acme/packages/libfoo.yaml` the obvious place for example 2?
3. In example 3, is “fork the package” easier to understand than “replace the
   pipeline”?
4. Are `distro`, `package override` and `fork` the right words?
5. Which command is missing from the “easy path”?
