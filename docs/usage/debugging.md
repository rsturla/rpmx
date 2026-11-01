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
rpmx explain root --json
rpmx steps list
rpmx steps show cmake/configure
rpmx doctor
```

`explain policy` shows the effective dist tag (including one from
`build.root.profile`), build release, profile, release and source date epoch.
`explain root` shows the merged package set and requirement digest.

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
