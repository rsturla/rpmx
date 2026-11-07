# Bootstrap ladder example

This is a small three-stage ladder. The recipes are metapackages so the plan can
be exercised without downloading a distribution, but the graph, profiles and
fixed-point stages are real.

```console
rpmx bootstrap plan examples/bootstrap/bootstrap.yaml
cat examples/bootstrap/plan.txt
rpmx bootstrap build examples/bootstrap/bootstrap.yaml --stage stage0
rpmx bootstrap build examples/bootstrap/bootstrap.yaml
```

Stage 0 is a seed stage: its profile uses `cross.bootstrap: true`, so the seed
build root and the target sysroot are separate even though both are `x86_64`.
It must name its packages explicitly and may not use `packages: all`. Stage 1
and stage 2 rebuild the toolchain from the previous stage's repository. The
`fixed-point` packages are rebuilt against a root that already contains the
stage output, so the stage converges on its own compiler and C library.

The `seed` profile points at `example.invalid`; a real ladder replaces that
with the seed repository or mirror. The first stage needs network access, a
working runner (`bwrap` or `podman`), and a seed repository that contains the
declared tools. Later stages only need the previous stage's output repository
and the locks committed next to the manifest.

Stage roots and locks are written under `roots/` and `locks/`, and stage
repositories under `out/`. They are intentionally not checked in. Commit the
locks after the first successful build, then use `--locked`:

```console
rpmx bootstrap build examples/bootstrap/bootstrap.yaml --locked
```

To rebuild one package from stage 0 against stage 1, keep plain `rpmx rebuild`
strict and use the bootstrap replacement command:

```console
rpmx bootstrap rebuild package.src.rpm \
  --root-lock examples/bootstrap/locks/stage1.lock.yaml \
  --root examples/bootstrap/roots/stage1/root \
  --output out/stage1/x86_64 \
  --srpm-out out/stage1/src
```
