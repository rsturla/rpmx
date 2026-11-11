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
shell. The debug shell shares native build-step sandbox construction and
inherits standard input, output and error, including terminal handles. Debug
sessions have no build-step deadline or captured build logs.

Bubblewrap runs commands as UID/GID 65534 inside a full subordinate-ID namespace.
That identity maps to your host account, so workspace writes remain yours.
Commands have no capabilities, cannot create nested user namespaces, and
cannot gain privileges by executing a setuid program. Mapping other IDs
keeps permission failures meaningful: an unprivileged `setuid(0)` fails with
`EPERM`, rather than `EINVAL` caused by an unmapped root ID.

If `rpmx doctor --strict` reports a transaction capability failure, check the
sandbox security label printed with the error and the host kernel audit log
(`sudo journalctl -k`). Ubuntu's
[`bwrap-userns-restrict` AppArmor profile](https://gitlab.com/apparmor/apparmor/-/blob/master/profiles/apparmor/profiles/extras/bwrap-userns-restrict)
stacks child processes with `unpriv_bwrap`, which explicitly denies capabilities.
This can deny access even when `/proc/self/status` lists `CAP_DAC_OVERRIDE` and
`kernel.apparmor_restrict_unprivileged_userns=0`. The administrator must provide
a policy that permits this capability for rpmx transactions. A local allow rule
cannot override the profile's explicit deny. rpmx does not change host policy.

`--debug` echoes the resolved command and sets `set -x` for `runs:` steps.

---
