# Contributor documentation

This directory is for people changing rpmx: the design study, format choices,
build model, sandbox contract and the UX audit. User-facing instructions live
in [`../usage/`](../usage/). The short contribution checklist is
[`../../CONTRIBUTING.md`](../../CONTRIBUTING.md).

## Design and implementation documents

| Document | Contents |
|---|---|
| [`00-design-study.md`](00-design-study.md) | The design study and worked examples. |
| [`01-landscape.md`](01-landscape.md) | Prior art and the packaging landscape. |
| [`02-rpm-baseline.md`](02-rpm-baseline.md) | What RPM already provides and where the gaps are. |
| [`03-design.md`](03-design.md) | The detailed model, CLI and semantics. |
| [`04-migration.md`](04-migration.md) | Importing specs and adopting the tool. |
| [`05-format-choice.md`](05-format-choice.md) | Why strict YAML, not TOML or a DSL. |
| [`06-build-inputs.md`](06-build-inputs.md) | Sources, content addressing and source assembly. |
| [`07-srpm-first.md`](07-srpm-first.md) | The SRPM-first build model. |
| [`08-threat-model.md`](08-threat-model.md) | Trust boundaries and untrusted inputs. |
| [`09-sandboxing.md`](09-sandboxing.md) | The sandbox contract. |
| [`10-container-distribution.md`](10-container-distribution.md) | How packages reach images. |
| [`11-matrices.md`](11-matrices.md) | Build matrices and cells. |
| [`12-templates.md`](12-templates.md) | The closed template scope. |
| [`13-pipelines.md`](13-pipelines.md) | Steps, bundles and the pipeline contract. |
| [`14-build-root.md`](14-build-root.md) | Root resolution and assembly. |
| [`15-ux-audit.md`](15-ux-audit.md) | UX audit against Melange and the implementation status. |
| [`16-qemu-appliance.md`](16-qemu-appliance.md) | Separating the QEMU transport appliance from the build root: design and plan. |
| [`17-ssh-runner.md`](17-ssh-runner.md) | Running locked-root builds on any SSH-reachable host, with podman, bubblewrap, chroot or explicit host execution as the adapter: design, decisions and tickets. |
| [`18-multi-arch-and-binfmt.md`](18-multi-arch-and-binfmt.md) | Multi-architecture builds, bubblewrap + binfmt execution, the qemu VM path, parallel scheduling, lock schema 2 and per-arch logging: implementation plan. |

## Working on the code

See [`../../CONTRIBUTING.md`](../../CONTRIBUTING.md) for build/test commands,
design rules and commit expectations. The acceptance scripts in `scripts/`
need `podman` and are run manually.
