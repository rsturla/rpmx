# Inspecting and debugging

## Inspecting and debugging

```console
rpmx plan
rpmx plan --json
rpmx explain policy
rpmx explain root
rpmx explain steps
rpmx explain matrix
rpmx explain requires
rpmx explain files
rpmx explain components --source /path/to/assembled/source
rpmx explain root --json
rpmx steps list
rpmx steps show cmake/configure
rpmx doctor
```

`doctor --recipe package.yaml` also reports whether that recipe's build root
and cross sysroot are already cached, which is exactly what an offline build
needs.

`explain policy` shows the effective dist tag (including one from
`build.root.profile`), build release, profile, release and source date epoch.
`explain root` shows the merged package set and requirement digest.
`explain components` shows the component inventory, evidence, scope and final
`bundled(...)` provides per output. Pass `--source` with an assembled source
workspace to discover vendor metadata; without it, only `outputs[].bundled`
declarations from the recipe are shown.

On failure:

```console
rpmx build --keep-workdir
# stderr prints: work directory: /tmp/...
rpmx shell --root /path/to/root \
  --workspace /tmp/.../build --stage /tmp/.../stage
```

`rpmx shell` uses the same bubblewrap mounts as a build step: root read-only at
`/`, workspace at `/tmp/build`, stage at `/tmp/stage`, no network, cleared
environment. `--command 'make -j8'` runs one command instead of an interactive
shell.

`--debug` echoes the resolved command and sets `set -x` for `runs:` steps.

---
