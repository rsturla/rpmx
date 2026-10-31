# The curl example

This directory is a design artifact, not a buildable package.

- `package.yaml` is the proposed recipe.
- `package.toml` is the earlier TOML draft, kept for comparison only.
- `curl.spec` is a conventional spec for the same package, for comparison.
- `plan.md` is the plan that `rpmx plan --profile fedora-44` would produce.

The source hash in `package.yaml` is a placeholder. The referenced patch and ABI
baseline are intentionally omitted; a real package would add them under
`patches/` and `abi/`. Nothing in this directory has been built or tested.
