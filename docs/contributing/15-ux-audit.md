# rpmx UX audit and Melange-parity proposals

Status: audit + proposal, 2026-11-01.
Audited tree: started at `d15a310`; advanced to `d0beb39` while the audit ran.
The observed UX was produced by binaries built at ~10:21 (`target/debug/rpmx`)
and 04:52 (`target/release/rpmx`), before the last three CLI/runner refactors.
Those commits change argument ownership and timeout plumbing, not the audited
surfaces, but a fresh build may differ in detail.
Melange reference: melange **v0.63.2** (run first-hand) plus
`chainguard-dev/melange@main` docs as of 2026-11-01.

This document is deliberately opinionated. The project has no users yet, so every
proposal here is allowed to break the current CLI and schema. The only
constraint worth keeping is the thing that makes rpmx different from Melange:
strict, typed build definitions and a native SRPM that can be rebuilt without
`rpmbuild`.

## Implementation status

The P0 and most P1 proposals below are implemented: recipe-owned and cached
build roots, `package.lock.json` and `--locked`, a real linter with multiple
diagnostics and stage exit codes, `rpmx test` with selection and JUnit,
self-sufficient rebuilds, a versioned JSON contract, `init`, `doctor`,
`explain`, `steps`, `shell`, `--keep-workdir`, `--debug`, default
`package.yaml`/`packages/<arch>`, repository indexing, source extraction for
gzip/xz/zstd, SHA-256 sources, package policy checks, Melange-style step
feedback, and Wolfi-shaped tests: per-test
`environment.contents.packages`/`environment.environment`, plus output-level
`tests:` blocks that install only the tested output and its runtime siblings.

Still open from the proposal set: `features`, typed `actions` (systemd,
sysusers, tmpfiles), `update` metadata, and a config file. `rpmx bump` and
`rpmx test --install-dir` are implemented. JUnit reports aggregate across
matrix cells. Local `file://` repositories are mounted into the resolver.

---

## 1. Executive summary

rpmx has a stronger **model** than Melange and a much worse **loop**.

Melange wins because a packager can go from "I have an upstream tarball" to a
built package in one command, with one file, and can see what each step did,
re-run one test, drop into the sandbox when a step fails, and get a repository
index at the end. Almost none of that loop exists in rpmx today.

The core finding is not "add more features". It is:

> rpmx makes the *first build* and the *second build after a failure* cost far
> more than they should, while spending its complexity on guarantees the user
> cannot see until after the build succeeds.

Three structural causes:

1. **The recipe does not own its build environment.** `build.dependencies.tools`
   is checked against a root that the user must resolve and assemble separately
   from two profile files. Declaring `gcc` in a recipe has no effect on root
   creation. This makes a one-command build impossible and is the single
   biggest gap to Melange.
2. **Execution is silent.** Step output is inherited straight to the terminal
   with no step name, phase, timing, or failure context. A failing step prints
   `RPMX-SANDBOX-002: step 'compile' failed with exit 3` and nothing else.
3. **The current implementation is a plumbing toolkit, not a task-oriented
   CLI.** `root resolve`, `root assemble`, `source`, `srpm`, and
   `rebuild --root <dir>` are all exposed as user-facing steps. The design
   documents promise `init`, `lock`, `explain`, `diff`, `test`, `shell`,
   `verify-srpm`, and `clean`; none exist.

### 1.1 The six changes that matter most

1. Let the recipe own its build root and merge `build.dependencies` + step
   `needs` into resolution (P0.1).
2. Report phases, named steps, timings, and a real failure bundle; add
   `--keep-workdir` and `--interactive` (P0.2, P1.3).
3. Make `lint` a linter and give diagnostics multiple errors, suggestions, and
   real exit codes (P0.3).
4. Ship a reviewable `package.lock.json` and `build --locked` (P0.4).
5. Add a first-class `rpmx test` with selection and per-test environments
   (P0.5).
6. Let `rebuild` use the root lock already inside the SRPM (P0.6).

Everything else in this document is either enabling work for those six or
polish.

### 1.2 Scorecard

Grades are relative to "would a new packager finish the job without asking for
help". A is best-in-class, F is unusable.

| Dimension | rpmx | Melange | Why |
|---|---|---|---|
| Time to first package | **D** | A | rpmx: write recipe + 2 profiles, resolve, assemble, build. Melange: `melange build x.yaml`. |
| Single source of truth | **D** | A− | rpmx: recipe + runtime profile + build profile + root lock + no package lock. Melange: one YAML. |
| CLI discoverability | **C+** | B+ | rpmx help is clean and short; but no completions, no `doctor`, no `explain`, plumbing verbs. |
| Build feedback | **F** | B+ | rpmx: unattributed output, no progress, no phase, no timing, no log, no tail. Melange: step names, stdout/stderr levels, `--debug`, preserved env; but no tail or log path. |
| Diagnostics | **C−** | B | Stable codes and line/column are good; one error per run, no suggestions, all errors exit 2. Melange rejects unknown fields with a line too, but also exits 1 and reports one error. |
| Test loop | **D** | B+ | No `rpmx test`; no test names in output; no per-test deps/env; no selection. Melange has per-test env and reuse, with confusing repo flags and no shell default. |
| Iteration/debug | **F** | B+ | No `--interactive`, `--debug`, `--shell`, `--keep-workdir`, workspace persistence. Melange has all of these (`--debug` verified; `--interactive` documented). |
| Machine interface | **D** | B− | `--json` is polluted by step stdout; unversioned; exit codes collapsed to 2. |
| Release workflow | **C** | A− | rpmx repo/sign/SBOM are good; no default index, no bump/update metadata. |
| Reproducibility/audit | **A−** | B | Native SRPM, CAS, root digest, per-file manifest. This is rpmx's real advantage. |
| Spec fidelity | **D** | n/a | Many documented commands, flags, and schema fields are absent. |

### 1.3 What rpmx gets right and must not lose

- Strict YAML with unknown fields rejected, and a closed template scope with no
  conditionals or shell in the build graph. Melange is also strict about unknown
  fields, but its `if:` expressions and shell templating are not analyzable;
  rpmx's closed scope is the genuine improvement.
- Native SRPM as the build contract, with the recipe, lock, steps, and sources
  embedded. Melange's source-package companion is the same idea, less mature.
- Content-addressed sources and an assembled-root digest.
- Explicit file ownership with totality checks. Melange's `split/*` pipelines
  hide ownership; rpmx can prove it.
- No `rpmbuild`, no spec, no macro evaluation.
- Offline builds from a lock.

The proposals below keep all of that. They change the surface, not the model.

---

## 2. Method and evidence

Every claim marked **[verified]** was produced by running a binary, not by
reading docs. rpmx transcripts are in Appendix A; first-hand Melange
transcripts are in Appendix C.

- Resolved and assembled `examples/fedora-44-build.profile.yaml`
  (209 packages; resolve 17 s, assemble 15 s). **[verified]**
- Built `examples/c-hello/package.yaml` end to end, with and without
  `--run-tests`, including installed-stage tests. **[verified]**
- Rebuilt the resulting SRPM with `--root`. **[verified]**
- Ran failing builds, weak recipes, `--json`, and every error path I could
  trigger without network credentials. **[verified]**
- Read Melange's command reference and build/test/pipeline/lint/update docs.
- Downloaded and ran **melange v0.63.2** (linux/amd64) against purpose-built
  probe configs with the bubblewrap runner: a successful build, a failing
  build, `--debug`, `test`, `compile`, `query`,
  `package-version`, `lint`, and `bump`. Transcripts in Appendix C.

Caveat: the tree moved from `d15a310` to `d0beb39` during the audit. The
observed behaviour matches the source I read at the time. I rebuilt at
`d0beb39` and re-checked the four findings that drive the proposals: `--json`
is still corrupted by step stdout, `lint` still accepts the weak recipe,
`plan` still reports `release=1` where `build` reports `1.fc44`, and a failed
step still exits 2 with no context. Re-run the probes before treating any
single transcript as a regression test.

The first-hand Melange run corrected two assumptions made from the docs:

- Melange **does** reject unknown fields (`field frobnicate not found in type
  config.Package`, with a line number). rpmx's strictness is not unique; what
  is unique is the closed template scope and the absence of shell-in-YAML
  conditionals.
- Melange's failure output is **not** a full log bundle. It names the step and
  preserves the environment, but the final error is still `exit status N` with
  no tail and no log path. The gap is real but narrower than the docs imply.

---

## 3. What Melange actually does that makes it good

This is not a feature list. It is the set of decisions that produce the
experience, in rough order of impact.

### 3.1 One file, one command

`melange build config.yaml` works with no other input. The config contains:

```yaml
environment:
  contents:
    repositories:
      - https://packages.wolfi.dev/os
    keyring:
      - https://packages.wolfi.dev/os/wolfi-signing.rsa.pub
    packages:
      - build-base
      - busybox
      - go
```

Those packages are the build dependencies. There is no separate root profile,
no resolve step, and no assemble step. The runner (bubblewrap, Docker, QEMU) is
selected automatically; `--runner` overrides it.

Verified: the probe config built in **3.1 s** from a cold start, including
fetching and installing nine packages, and `melange build` with no argument
resolved `./melange.yaml` in the current directory. **[verified]**

### 3.2 Pipelines contribute their own build dependencies

Every `uses:` step can declare `needs.packages`. Before the build, Melange walks
the whole pipeline tree, merges those packages into `environment.contents`, and
builds the guest root from the union. A recipe that says `uses: autoconf/make`
does not have to remember `make`.

rpmx's step model already has `needs.tools` (`crates/rpmx-core/src/steps.rs`,
`pipelines/*.yaml`), but the planner only records it. It never feeds root
resolution.

### 3.3 The source root is the workspace

`fetch` and `git-checkout` put the source at the workspace root (strip one
component by default). Pipelines are written as if they run in the source tree:

```yaml
pipeline:
  - uses: autoconf/configure
  - uses: autoconf/make
  - uses: autoconf/make-install
```

Melange's `fetch` pipeline accepts `expected-sha256` **or** `expected-sha512`,
extracts by default with `--strip-components=1`, supports a target `directory`,
`extract: false`, deletion after unpack, retries, and timeouts. **[verified]**

rpmx unpacks archives verbatim (`crates/rpmx-core/src/source.rs`,
`materialize`) and only if the file name ends in `.tar.gz`/`.tgz`
(`fetch/mod.rs`, `is_archive`). Every recipe therefore starts with
`cd zstd-1.5.7`, baking the version into the shell.

### 3.4 Subpackages are split by where you install, not by globs

Melange has no exhaustive `files.include`. The main package writes into
`${{targets.destdir}}`; a subpackage moves files into
`${{targets.subpkgdir}}` or runs a `split/dev`, `split/manpages`,
`split/static` pipeline. The common case is one line.

rpmx's strict selectors are a strength, but the authoring cost is real: every
installed path must be listed twice (install + selector), and an unowned file
fails the build only after it has run.

### 3.5 Tests are first-class and scoped

Tests live next to the package or subpackage:

```yaml
test:
  packages:
    - busybox
    - python-3
  pipeline:
    - runs: |
        python3 -c 'import pandas'
```

`melange test config.yaml [package[=version]]` runs them, with a fresh guest
per test, the package under test installed, and per-test dependencies. You can
iterate on tests without rebuilding the package.

### 3.6 The build tells you what it is doing

Melange logs each step with its name (`running step "compile"`), prefixes child
stdout as `INFO` and stderr as `WARN`, runs `runs:` with `set -x` under
`--debug`, and attaches an interactive TTY on failure with `--interactive`. It
also writes an optional dependency log and build log, and preserves the test
environment when a test fails.

It is not perfect: the final failure is still `exit status 3` with no tail and
no log path, and the "preserved" environment path is not printed. **[verified]**
But it names the step and keeps the environment, which is already most of the
debugging loop.

### 3.7 Post-build linting is on by default

After the package is assembled, Melange runs linters (`dev`, `infodir`,
`setuidgid`, `tempdir`, `usrmerge`, `varempty`, `worldwrite`, plus a warn set
including `strip`, `lddcheck`, `pkgconf`, `python/*`, `sbom`). Failures are part
of the build. A recipe disables one with a justification:

```yaml
package:
  checks:
    disabled:
      - setuidgid  # Package is meant to have setuid binaries
```

rpmx's `lint` runs before the build and cannot see the package, so it cannot
catch any of this.

### 3.8 The output directory is a repository

`melange build` writes to `./packages/` and generates `APKINDEX.tar.gz` by
default (`--generate-index`, default true). The result is immediately
consumable by `apko`. `melange index` can rebuild or merge an index from a list
of package files.

### 3.9 Version bumps and update metadata are part of the config

`update:` declares where to look for new versions (release-monitoring, GitHub,
git, OCI), with tag filters and version transforms. `melange bump config.yaml
1.2.3 --expected-commit <sha>` rewrites the version and, for a git source, the
recorded commit. A packager does not hand-edit commit pins. (Fetch checksums
still need their own update, which is why rpmx's `bump` should cover both.)

### 3.10 The lock is a compiled config, not a hidden artifact

`melange compile config.yaml` resolves template substitutions, expands
`uses:`, applies conditionals, and merges step dependencies into the recorded
configuration. It is reviewable and diffable.

Verified: `melange compile probe.yaml` printed a single-line JSON document with
the expanded `${{targets.destdir}}`, the default cache environment variables,
and the test environment's package list (which automatically includes the
package under test). It wrote no file. **[verified]**

### 3.11 What Melange gets wrong

A parity plan should copy the loop, not the flaws. First-hand findings:

- **`bump` reformats the file and shows no diff.** `melange bump bump.yaml
  1.0.1` removed blank lines throughout the YAML and printed only
  `attempting to bump version to 1.0.1`. A packager has to `git diff` to see
  what changed. **[verified]**
- **Failures have no tail or log path.** The last useful line is
  `running step "compile"`; the error is `exit status 3`. There is no command,
  cwd, or saved log. **[verified]**
- **License checking lints the whole workspace.** The probe build fetched and
  installed packages, then warned that the downloaded melange tarball's
  `LICENSE` was `Apache-2.0 not found` in a recipe that declares `MIT`.
  Workspace noise becomes a license warning. **[verified]**
- **Test repository flags are confusing.** `melange test --repository-append
  ./packages` printed `repositories: []` and failed to find the local package;
  the repository had to be put in `test.environment.contents.repositories`
  instead. **[verified]**
- **A test environment without a shell fails obscurely.** The first test run
  failed with `bwrap: execvp /bin/sh: No such file or directory` because
  `busybox` was not listed in the test environment. The fix is obvious in
  hindsight, but the error does not say "add a shell package". **[verified]**
- **The flag surface is enormous.** `melange build --help` lists 50 flags,
  most of which are runner or service plumbing. rpmx should not reproduce that
  as its primary interface.
- **"Preserved" state is not discoverable.** After a failed test, Melange says
  the environment "has been preserved" but does not print the path.

---

## 4. rpmx today, command by command

### 4.1 Command surface

| Command | What it really is | Verdict |
|---|---|---|
| `lint FILE` | Parse + plan. No lint rules. | Misnamed; see 4.6. |
| `plan FILE` | Expanded commands, outputs, tests. | Useful, but leaks container paths and wrong release. |
| `root resolve PROFILE` | dnf5 resolve in a builder container. | Plumbing. Should be automatic. |
| `root assemble LOCK` | Offline root build. | Plumbing. Should be automatic. |
| `root digest ROOT` | Tree digest. | Broken on roots it just assembled. |
| `root export/import` | libpkgmanifest bridge. | Fine, advanced. |
| `fetch FILE` | Populate CAS. | Plumbing; digest must already be known. |
| `source FILE` | Assemble source snapshot. | Plumbing. |
| `srpm FILE --sources DIR` | Write SRPM from a snapshot you must create first. | Plumbing. |
| `build FILE` | Full pipeline **or** `--stage` packaging. | Two commands wearing one name. |
| `rebuild SRPM` | Rebuild from SRPM. | Cannot assemble its own embedded root. |
| `repo --rpms DIR --output DIR` | Copy RPMs + write repodata. | Good, but no index default. |
| `cache gc` | GC with explicit refs. | No `info`, no `verify`. |
| `keygen` / `sign` | OpenPGP. | Fine. |

Missing versus `docs/03-design.md` §6: `init`, `fmt`, `lock`, `explain`,
`graph`, `diff`, `sbom`, `attest`, `import-spec`, `shell`, `clean`, `test`,
`inspect-srpm`, `verify-srpm`, `features`, `migrate`.

Missing versus the design's common flags: `--locked`, `--features`, `--target`,
`--no-sandbox`, `--keep-workdir`.

### 4.2 The first-build path

The README quickstart is three commands:

```console
rpmx root resolve examples/fedora-44-build.profile.yaml --output root.lock.yaml
rpmx root assemble root.lock.yaml --root ./root --output root.assembled.yaml
rpmx build examples/pyhello/package.yaml --root-lock root.assembled.yaml \
  --root ./root --output ./out --run-tests
```

That is the documented happy path. In practice a user must also know:

- which of the two profile files to use (`fedora-44.profile.yaml` only installs
  `bash`; it is not enough to build anything);
- that `build.dependencies.tools` in the recipe does **not** add packages to
  the root, so the build profile must already contain the union of every tool
  every package needs;
- that `--profile` on `build` is not the root profile; it only supplies a dist
  tag;
- that `debuginfo` and `sbom` come from the root lock, not from `--profile`;
- that `--run-tests` is required for tests to run at all.

A clean-checkout `rpmx build examples/zstd/package.yaml` fails with:

```
RPMX-ROOT-016: a full build requires --root-lock and --root for source assembly; pass --unsafe-host-root to use the host root
```

**[verified]**

### 4.3 What a successful build looks like

```
cc -O2 -Wall -Wextra -Werror -o c-hello hello.c
./c-hello
hello from a locked root
install -D -m 0755 c-hello /tmp/stage/usr/bin/c-hello
ln -sf c-hello /tmp/stage/usr/bin/c-hello-alias
hello from a locked root
hello from a locked root
/tmp/out2/src/c-hello-1.0-1.fc44.src.rpm
/tmp/out2/c-hello-1.0-1.fc44.x86_64.rpm
```

**[verified]**

Observations:

- No phase names (`fetch`, `source`, `srpm`, `build`, `package`, `test`), no
  step names, no timing, no `ok` markers.
- The two `hello from a locked root` lines are the build-stage and
  installed-stage tests. Nothing says which test ran or whether it passed.
- The build-level SBOM and `test-root.lock.json` are written but not printed.
  The README claims four output lines; the tool prints two.
- The SRPM path is printed before the binary RPM path, but the JSON groups are
  `rpms` then `srpms`. Inconsistent grouping.

### 4.4 What a failure looks like

Recipe with `exit 3` in a named step:

```
compiling...
something went wrong
RPMX-SANDBOX-002: step 'compile' failed with exit 3
```

**[verified]**

No command, no phase, no `step N of M`, no log path, no hint, no exit-code
taxonomy (the process exits 2, which the design reserves for recipe errors).

### 4.5 `--json` is not machine-readable

```
$ rpmx build ... --json 2>/dev/null | jq -r .ok
cc -O2 -Wall -Wextra -Werror -o c-hello hello.c
./c-hello
hello from a locked root
jq: parse error: Invalid numeric literal at line 1, column 3
```

**[verified]**

Step stdout is inherited to the process stdout. Any consumer that pipes
`rpmx build --json` into `jq` gets a parse error. This is a release blocker for
CI use. Reproduced with a fresh build at `d0beb39`: `--json >stdout.json`
writes five lines of compiler/test output before the JSON object.

### 4.6 `lint` does not lint

`rpmx lint` calls `load_plan` and prints `ok: N cell(s)`. **[verified]**

It accepts all of the following without a warning:

```yaml
package:
  release: 0          # zero release
  summary: "a"        # one character
  license: "MIT OR"   # invalid SPDX expression
build:
  packages: [gcc, gcc] # duplicate
```

**[verified]**

It also cannot see the built package, so it cannot check ownership, scriptlet
safety, `/usr/local`, setuid bits, or missing license files.

### 4.7 `plan` leaks ephemeral paths and the wrong release

`rpmx plan examples/c-hello/package.yaml` prints:

```
default  slug= release=1 package=c-hello
```

but the same recipe built against the Fedora root produces
`c-hello-1.0-1.fc44`. **[verified]**

The plan keeps `${stage}`, `${source.dir}` and the other executor paths as
placeholders; only the sandbox resolves them. A reviewer sees the build
contract, not a container-internal path.

`plan` prints the full inline `runs:` body for every step, including a 30-line
`strip` shell script. That is honest, but it is not readable, and it omits the
step digest, the step's `needs`, and the input bindings that
`docs/13-pipelines.md` §8 promises.

### 4.8 Policy is split across three places

- `--profile FILE` on `build`/`lint`/`plan` only reads `dist-tag` and
  `--build-release` (`policy.rs`).
- `debuginfo` and `sbom` come from the root lock (`rebuild/mod.rs`).
- `--dist-tag` on the CLI wins over the profile, but the lock is authoritative
  at rebuild time (`srpm.rs`, `policy.rs::reconcile_policy`).

The result: `rpmx plan` and `rpmx build` can disagree, and there is no single
command that prints the resolved policy. **[verified]**

### 4.9 `rebuild` is not self-sufficient

Every SRPM embeds `root.lock.json`, but `rpmx rebuild foo.src.rpm` refuses to
use it:

```
RPMX-ROOT-016: a root lock is available; pass --root with the assembled root directory
```

**[verified]**

The archival promise ("rebuild from the SRPM without the recipe") requires the
user to extract `root.lock.json`, run `root assemble`, and pass `--root`. There
is no documented command for the extraction. This is exactly the workflow
rpmx claims as its differentiator.

### 4.10 `root digest` fails on the root it just produced

```
$ rpmx root assemble build.lock.yaml --root /tmp/rpmx-root --output out.yaml
... root_digest=sha512:bb44...
$ rpmx root digest /tmp/rpmx-root
RPMX-ROOT-014: cannot read /tmp/rpmx-root/etc/gshadow: Permission denied
```

**[verified]**

`root assemble` uses `digest_root`, which falls back to `unshare -Ur` or
`podman unshare` for mode-000 files. The `root digest` command calls
`tree_digest` directly (`commands/root.rs`), so it cannot read `/etc/shadow`.
`build` and `rebuild` still work because they use the fallback. The standalone
verification command is therefore unusable on a normal root.

### 4.11 Recipe gaps that matter immediately

The model (`crates/rpmx-core/src/model.rs`) is much smaller than the design.

| Design field | Implemented | Consequence |
|---|---|---|
| `sources[].extract.format/strip-components/destination` | No | Archive layout is name-sniffed; only gzip is unpacked. |
| `sources[].signature`, `mirrors` | No | No upstream signature verification. |
| `patches[].when`, `resolves` | No | No feature-conditional patches; no CVE metadata. |
| `features` | No | A documented core concept is missing. `examples/curl` uses it and does not parse. |
| `outputs[].actions` (systemd, sysusers, tmpfiles) | No | Raw `scriptlets` only. `hello-daemon` uses raw scriptlets. |
| `outputs[].generators` | No | No automatic ELF/pkgconfig dependency generation. |
| `outputs[].when`, `auto-split` | No | Manual selectors only. |
| `package.maintainers` required | Optional | Lint accepts none. |
| `package.architectures`, `unowned-files`, `ignore-files` | No | No arch policy; unowned files are always fatal. |
| `package.changelog`, `release.auto: git` | No | Release is a literal string. |
| `tests[].dependencies`, `kind: abi/reproduce` | No | Tests share one global tool list; only commands. |
| `build.dependencies.extra` with `when/reason` | No | No conditional dependencies. |
| Profiles with `paths.prefix/libdir`, policy flags | Partial | Prefix/libdir are wired through the lock and steps; policy flags are not. |

`examples/curl/package.yaml` (a design artifact) does not parse:
`RPMX-YAML-001: package: unknown field 'unowned-files'`. That is fine as a
design example, but the README links to it as if it were usable.

### 4.12 Small but constant friction

- `<FILE>` is mandatory everywhere. `rpmx build` in a package directory should
  default to `package.yaml`.
- `--output` defaults to `.`, so builds pollute the recipe directory. Melange
  defaults to `./packages/`.
- `--jobs` defaults to 1. Builds are serial by default.
- `--cache` everywhere; Melange calls the same concept `--cache-dir`. Prefer
  the explicit name and keep the XDG default.
- `--runner` accepts `auto|bwrap|qemu`; podman is a real backend but not
  selectable, and it is chosen implicitly by `--root-image`.
- `--arch` is an unvalidated `String`; an unknown arch fails later with a
  runner error.
- Every subcommand repeats `--json`; there is no global `--json`, `--quiet`,
  `--verbose`, `--color`, or `--log-level`.
- `rpmx help` is clear, but there are no shell completions and no man pages.
- `rpmx --version` prints `rpmx 0.0.0`, which is not useful in a bug report.
- No `doctor` command, despite depending on bwrap, podman, dnf5, qemu-system,
  qemu-user binfmt, ssh, mke2fs, and debugfs.

### 4.13 Filesystem and state hygiene

- `build --stage` conflicts with 9 other flags and is a second mode in the same
  command (`args.rs`). Its help is a wall of conflicts.
- `source` writes `RPMX-SOURCE.tar.gz` to `--output` (default `.`).
- `root resolve` writes a lock to stdout by default, but prints a summary only
  when `--output` is set. Piping to a file gives no progress at all.
- Work directories are `tempfile` dirs with no `--keep-workdir`; after a
  failure there is nothing to inspect.
- The package lock exists only inside the SRPM. There is no reviewable
  `package.lock.json` in the repository, so a source/step/dependency change is
  invisible until someone builds and extracts the SRPM.

---

## 5. The gap, in one table

| Job to be done | Melange | rpmx today | Gap |
|---|---|---|---|
| Start a package | copy nearest YAML | write recipe + 2 profiles | **High** |
| Build it | `melange build x.yaml` | 3 commands + 4 flags | **High** |
| Add a build tool | edit the recipe | edit the shared build profile, re-resolve, re-assemble | **High** |
| See what is running | per-step logs | raw interleaved output | **High** |
| Fix a failed step | `--interactive`, `--debug` | edit and rebuild blind | **High** |
| Run one test | `melange test x.yaml pkg` | `build --run-tests` (all) | **High** |
| Iterate on tests | tested against a repo package | rebuild everything | **High** |
| Update a version | `melange bump` | manual, digest copied from an error | **Medium** |
| Review a lock | `melange compile` | none in the repo | **High** |
| Get a repo | index by default | `rpmx repo` manually, no index default | **Low** |
| Know why it failed | named step, preserved env, but `exit status N` | one code, one line, deleted workdir | **High** |
| Script it | JSON logs + flags | `--json` corrupted by logs | **High** |
| Rebuild from archive | source package + cache | manual root extraction | **High** |

---

## 6. Proposals

Priorities are about **user-visible loop impact**, not implementation size.

- **P0**: without these, rpmx is not in Melange's ballpark.
- **P1**: closes the day-to-day iteration gap.
- **P2**: schema capability needed for real packages.
- **P3**: polish.

Each proposal has a problem, a concrete design, acceptance criteria, and notes.
All of them assume breaking changes are allowed.

---

### P0.1 — One-command build: the recipe owns its root

**Problem.** `rpmx build` requires `--root-lock` and `--root`, and the recipe's
`build.dependencies.tools/host/target` and step `needs` never affect root
resolution. Adding a tool means editing a shared profile, re-resolving, and
re-assembling. This is the single largest gap to Melange.

**Design.** Add a `build.root` block to the recipe. It is either a profile
reference (name or path) or an inline profile body:

```yaml
build:
  # Option A: named profile, discovered by convention
  root:
    profile: fedora-44

  # Option B: self-contained, Melange-style
  root:
    releasever: "44"
    repositories:
      - id: fedora
        metalink: https://mirrors.fedoraproject.org/metalink?repo=fedora-$releasever&arch=$basearch
      - id: updates
        metalink: https://mirrors.fedoraproject.org/metalink?repo=updates-released-f$releasever&arch=$basearch
    keyring:
      - https://fedoraproject.org/fedora.gpg
    packages: [bash, coreutils]   # optional; merged with dependencies

  packages: [gcc, make, sh]
  sysroot: [zlib-devel]
```

Resolution rules (this is the contract that must be written down before code):

1. **Requirement set** = profile `packages.install` ∪ `build.dependencies.*` ∪
   the transitive `needs.tools/host` of every resolved step in
   `source-assembly`, `build`, and every test ∪ `--package-append`.
2. If `build.root` is absent, use `--root-profile`, then
   `./profiles/<name>.yaml`, `$XDG_CONFIG_HOME/rpmx/profiles/`, then a built-in
   `fedora-44` profile. Print the chosen profile and why.
3. Canonicalise the requirement set and hash it
   (`requirement_digest`). Cache the resolved closure and assembled root under
   `$XDG_CACHE_HOME/rpmx/roots/<closure_digest>/`.
4. On a miss, resolve with dnf5 and assemble. On a hit, skip both.
5. `--root-lock`/`--root` remain as explicit overrides, mutually exclusive with
   `build.root`.
6. `--offline` fails if the required root is not cached, naming the digest.
7. `--refresh-root` forces re-resolution; `--print-root` prints the closure.

Expose the resolved result in the plan:

```
root    fedora-44  x86_64  closure=sha512:8255...  source=cache
packages 209 (bash, coreutils, gcc, make, ...)
```

`rpmx root resolve|assemble` stay as debugging verbs, not the primary path.

**Acceptance.**

- From a clean checkout with no package-owned compiler policy:
  `rpmx build examples/zstd/package.yaml` succeeds.
- Adding `strace` to `build.dependencies.tools` changes the resolved root
  requirement digest and installs `strace` on the next build.
- The second identical build prints `root ... source=cache` and spends < 2 s
  before the first step.
- `rpmx build --offline` on a cold cache exits non-zero with the missing
  closure digest and the command to warm it.

**Notes.** Reuse the existing `Profile` schema for the inline body so there is
one root vocabulary. Do not call the block `environment`: `build.environment`
already means environment variables, and Melange's `environment.contents` is a
different concept. `build.root` is clearer.

---

### P0.2 — Execution feedback: phases, steps, failure context

**Problem.** Output is inherited directly from the child process. Nothing says
which phase or step produced it. A failure has no command, no log, no tail, no
hint, and no sandbox to inspect.

Melange already names the step and preserves the environment; its weakness is
that it stops there (`exit status 3`, no tail, no path). rpmx currently does not
even name the step in the success path, and deletes the workdir on failure.

**Design.** Introduce a `Reporter` with three modes: `interactive` (TTY),
`plain` (CI), and `json`.

Interactive:

```
rpmx build examples/zstd/package.yaml
  resolve  fedora-44 x86_64  209 packages  cache hit            0.1s
  fetch    3 sources                                            0.4s
  source   RPMX-SOURCE.tar.gz  sha512:5c1e...                   1.2s
  srpm     zstd-1.5.7-1.fc44.src.rpm                            0.3s
  build    [1/3] make        make -j8 ...                      12.4s
           [2/3] strip       find /tmp/stage ...                0.9s
  package  libzstd, libzstd-devel, zstd                         0.2s
  test     [1/2] smoke (build)     ok                           0.1s
           [2/2] installed         ok                           0.6s
  done     4 artifacts in ./packages/x86_64                     15.5s
```

Plain (non-TTY or `--progress=plain`), one line per event:

```
rpmx: resolve fedora-44 x86_64: 209 packages (cache)
rpmx: build step 1/3 make: ok (12.4s)
rpmx: build step 2/3 strip: ok (0.9s)
rpmx: test smoke (build): ok (0.1s)
rpmx: done: 4 artifacts in ./packages/x86_64
```

Step stdout is captured to `.rpmx/logs/<build-id>/<NN>-<step>.log` and, in
interactive mode, streamed with a dim prefix or under a step header. On failure:

```
error[RPMX-SANDBOX-002]: step 'compile' failed with exit 3
  phase:   build (step 2 of 4)
  command: sh -euo pipefail -c 'make -j8'
  cwd:     /tmp/rpmx-abc/build/zstd-1.5.7
  log:     .rpmx/logs/2026-11-01T10-22-01/02-compile.log
  ---- last 20 lines ----
  compiling...
  something went wrong
  ------------------------
  help: re-run with --keep-workdir --interactive to inspect the sandbox
```

New flags, global: `--verbose`, `--quiet`, `--debug` (pass `set -x` to
`runs:`), `--progress=auto|plain|none`, `--color=auto|always|never`,
`--keep-workdir`, `--interactive`, `--log-file` (JSON Lines).

**Acceptance.**

- A failing named step prints phase, step index, command, cwd, log path, tail,
  and a hint. Test: `tests/ux/failure_output.rs` asserts all fields.
- `--quiet` prints only artifact paths and errors.
- `--debug` makes the step shell echo commands.
- `--keep-workdir` leaves the temp dir and prints its path.
- No step output reaches stdout when `--json` is set (see P0.7).

**Notes.** `process.rs` currently calls `Command::status()`. Switch to piped
stdio with a reader thread per stream, a `StepLog` writer, and a bounded ring
buffer for the failure tail. Keep the child's stdout and stderr separate in the
log file.

---

### P0.3 — `lint` becomes a real linter; diagnostics become useful

**Problem.** `lint` is `plan` with a different print. It has no rules, one error
per run, no warnings, no suggestions, no SPDX validation, no policy checks.
Every error exits 2.

**Design.**

1. **Collect, don't abort.** Validation returns `Vec<Diagnostic>`, not
   `Result<(), Error>`. `lint` prints all of them (default `--max-errors 20`).
2. **Diagnostic shape.** Text with a source span, related spans, notes, and
   suggestions; JSON with the same fields.
3. **Rules.** At minimum:
   - unknown field with a "did you mean" suggestion (serde gives the field list
     today; add fuzzy matching);
   - invalid SPDX expression (`license`, `source-license`);
   - empty/one-character `summary`; summary over 80 chars (warning);
   - release `0`; non-integer release; missing `source-date-epoch` (warning);
   - duplicate dependency atoms; duplicate tools; unused `vars`; unused matrix
     axes (warning);
   - source digest missing, wrong algorithm, or malformed;
   - locator that is not `https`/`file` (warning for non-https);
   - patch file missing on disk; patch with no `reason` (warning);
   - output name that collides after expansion; output with no files and no
     `metapackage`;
   - test with an empty `argv` and empty `pipeline`;
   - `runs:` step without `shellcheck` clean (when `shellcheck` is on PATH);
   - step `needs` not satisfiable by the selected root, when `--root` is given;
   - inline `runs` count per recipe (warning above a threshold).
4. **Exit codes.** Implement the design's table and use it everywhere:
   `0` ok, `2` recipe/schema, `3` lock drift, `4` resolution, `5` build,
   `6` test, `7` policy, `8` evidence, `9` internal. Map code domains to codes
   in one function in `main.rs`; never hardcode `2`.
5. **`--strict`** turns warnings into errors for CI. `--format=text|json|github`
   for CI annotations.

Example output:

```
error[RPMX-OUTPUT-011]: file /usr/lib/libcurl.so.4 is claimed by both 'libcurl' and 'libcurl-devel'
  --> package.yaml:142:9
   |
142|         - /usr/lib/libcurl.so.4*
   |           ^^^^^^^^^^^^^^^^^^^^^^^ claimed here
143|
   = note: a path may be claimed by exactly one output
   = help: remove the path from one output, or use `auto-split: [libs, devel]`
```

JSON:

```json
{
  "schema": 1,
  "summary": { "errors": 1, "warnings": 2 },
  "diagnostics": [
    {
      "code": "RPMX-OUTPUT-011",
      "severity": "error",
      "message": "file /usr/lib/libcurl.so.4 is claimed by both 'libcurl' and 'libcurl-devel'",
      "file": "package.yaml",
      "line": 142,
      "column": 9,
      "field": "outputs[2].files.include",
      "suggestions": ["remove the path from one output", "use auto-split: [libs, devel]"],
      "related": [{ "file": "package.yaml", "line": 151, "message": "also claimed here" }]
    }
  ]
}
```

**Acceptance.**

- A recipe with three independent mistakes reports three diagnostics in one
  run and exits 2.
- A lock mismatch exits 3; a build-step failure exits 5; a failing installed
  test exits 6.
- `--json` emits exactly one JSON document on stdout and nothing else.
- `rpmx lint` on `examples/curl/package.yaml` reports every unsupported field at
  once, with the schema version that would accept it.

---

### P0.4 — A reviewable lockfile

**Problem.** There is no `package.lock.json` in the repository. Sources, step
digests, and policy are only recorded inside the SRPM after a build. A reviewer
cannot see a dependency or source change in a diff. `rpmx lock` is documented
and absent.

**Design.**

```
rpmx lock package.yaml [--update] [--check] [--output package.lock.json]
rpmx build --locked
rpmx plan --locked
```

- `rpmx lock` writes a deterministic JSON lock next to the recipe (default
  `package.lock.json`), or prints it with `--output -`.
- Contents: recipe digest, schema version, resolved matrix cells, package
  identity, policy (dist tag, build-release, debuginfo, sbom), source list with
  digest/size/locator, step list with name/version/digest, root requirement
  digest and closure digest, source-date-epoch, output names and manifest
  digests if known.
- `rpmx lock --check` verifies without writing (for CI).
- `rpmx lock --update` re-resolves and prints a unified diff; it writes only
  with `--write`, matching the design's "never silently accept".
- `rpmx build --locked` verifies every field and exits 3 on drift, naming the
  changed field.
- `rpmx plan` shows `locked: yes|no|drifted` at the top.

Do not make `--locked` the default. A missing lock should be a normal first run;
CI opts in.

**Acceptance.**

- Changing a source digest and running `rpmx lock --check` exits 3 and prints
  the exact field diff.
- Changing a step's YAML changes its digest and is reported.
- `rpmx build --locked` on a drifted recipe fails before running any step.
- The lock is byte-identical across two runs on the same inputs.

---

### P0.5 — A first-class `test` command with scoped environments

**Problem.** Tests only run as part of `build --run-tests`. There is no test
selection, no test names in output, no per-test dependencies or environment,
and no way to test against an already-built package. `build.dependencies.tests`
is one global list.

**Design.**

```
rpmx test package.yaml [--test NAME]... [--stage build|installed|package]
                      [--root ...] [--install-dir DIR] [--junit FILE]
                      [--json] [--keep-workdir] [--interactive]
```

- `rpmx test` runs the plan's tests without rebuilding when `--install-dir`
  points at built RPMs. Otherwise it builds (and reports that it is building).
- `--test NAME` selects one or more tests; `--stage` selects a stage.
- Output names each test and its result, including duration and the failing
  command.
- `--junit FILE` writes JUnit XML; `--json` emits structured results.
- `build --run-tests` delegates to the same runner.

Recipe: allow tests at the top level **and** under outputs, with per-test
environment and dependencies. Both scopes now use the Wolfi shape
(`test.environment.contents.packages` and `test.environment.environment`); a
separate `dependencies.run` list is still open.

```yaml
tests:
  - name: cli
    stage: installed
    packages: [jq, ca-certificates]
    environment:
      FOOBAR: "1"
    argv: [curl, --version]

outputs:
  - name: libcurl
    tests:
      - name: symbols
        stage: installed
        pipeline:
          - runs: nm -D /usr/lib64/libcurl.so.4 | grep curl_easy_init
```

An installed test declared under an output installs only that output plus its
runtime dependencies. A top-level test installs all outputs (integration test).

**Acceptance.**

- `rpmx test examples/c-hello/package.yaml --test installed` runs one test,
  names it, and does not rebuild if `--install-dir` is given.
- A failing test prints the test name, stage, command, exit code, and tail, and
  exits 6.
- `--junit` produces valid XML that a CI runner accepts.
- `rpmx test --json` is parseable (see P0.7).

---

### P0.6 — The SRPM can rebuild itself

**Problem.** Every SRPM embeds `root.lock.json`, but `rebuild` demands a
pre-assembled `--root`. The archival rebuild path is manual extraction plus
`root assemble`.

**Design.** `rpmx rebuild foo.src.rpm` should:

1. Use `--root` if given (verify it against the embedded lock).
2. Else use `--root-image` if given.
3. Else assemble the embedded root offline from the CAS into a cache entry
   keyed by `root_digest`, and use it. Print `root: assembled from SRPM lock,
   cache hit|miss`.
4. `--offline` fails with the exact missing closure digest and the command to
   warm the cache.
5. `--no-assemble-root` opts out and reproduces today's error.

Add the missing inspection verbs so the extraction is never needed:

```
rpmx inspect-srpm FILE [--json]      # header, files, lock summary, policy
rpmx verify-srpm FILE [--signature]  # manifest + digests + optional signature
rpmx extract-srpm FILE --output DIR  # explicit, documented
```

**Acceptance.**

- On a machine with the CAS warm, `rpmx rebuild foo.src.rpm` succeeds with no
  other flags.
- On a cold cache with `--offline`, it fails with the closure digest.
- `rpmx verify-srpm` detects a mutated payload file and exits 8.
- `rpmx inspect-srpm` shows the embedded recipe digest, root digest, dist tag,
  step digests, and file list.

---

### P0.7 — Fix the machine interface

**Problem.** `--json` is corrupted by step stdout; exit codes are all 2; error
JSON has no schema version, no suggestions, no related spans; artifact JSON
omits SBOMs, test-root locks, and signing info inconsistently.

**Design.**

- One JSON document per command on **stdout**. All logs, progress, and step
  output go to **stderr** (or a log file) whenever `--json` is set.
- Every document carries `"schema": 1` and a `command` field.
- Success documents list every artifact with a `kind`
  (`rpm`, `srpm`, `sbom`, `test-root-lock`, `repo-metadata`, `signature`) and a
  `path`; no more ad-hoc `rpms`/`srpms` keys.
- Error documents use the diagnostic shape from P0.3 and may contain several
  diagnostics.
- Exit codes follow P0.3.
- Add `--json` globally; keep per-command flags as aliases.
- `RPMX_LOG_FORMAT=json` emits one JSON object per log line on stderr for
  streaming CI.

**Acceptance.**

- `rpmx build ... --json | jq -e '.schema == 1 and (.artifacts | length > 0)'`
  succeeds in a full build.
- A failing build with `--json` emits one error document on stdout and exits 5.
- `rpmx lint --json` with three errors emits one document with three
  diagnostics.

---

### P1.1 — `init`, defaults, and scaffolding

**Problem.** There is no starting point. A new package needs a recipe, two
profiles, and knowledge of the root workflow.

**Design.**

```
rpmx init [DIR] [--template c|rust|go|python|metapackage] [--profile fedora-44]
rpmx build            # defaults to ./package.yaml
rpmx lint             # defaults to ./package.yaml
rpmx build .          # directory form
```

`init` writes:

```
package.yaml            # valid, lints clean, builds
profiles/fedora-44.yaml # if the recipe references it
pipelines/              # empty, documented
tests/                  # optional fixtures
.gitignore              # .rpmx/, packages/
README.md               # three commands, no lore
```

`rpmx init --template c` should produce a recipe equivalent to `c-hello` that
builds without edits. This is the single best onboarding feature rpmx can add.

**Acceptance.** `rpmx init hello && cd hello && rpmx lint && rpmx build`
succeeds. Add it to CI as a smoke test.

---

### P1.2 — `explain`: show provenance, not just values

**Problem.** The design promises `rpmx explain`, and it is the natural answer to
"why is this dependency here?", "which profile set this flag?", "what did the
matrix do?". Today a user must read the lock or the source.

**Design.**

```
rpmx explain requires libcurl     # every edge and its source
rpmx explain files zstd           # selector -> files, unowned, claimed
rpmx explain policy               # resolved dist tag, release, debuginfo, sbom, root
rpmx explain root                 # requirement set and which recipe line needed each package
rpmx explain steps                # expanded steps, needs, digests, inputs
rpmx explain matrix               # axes, exclusions, cells, bindings
rpmx explain provenance FILE.rpm  # recipe/lock/source/step digests
```

Every value is annotated with where it came from: `recipe`, `profile`,
`matrix`, `step`, `default`, or `cli`. Example:

```
dist-tag: .fc44
  source: profile fedora-44 (profiles/fedora-44.yaml:5)
release:  1.fc44
  source: recipe package.release=1 + profile dist-tag
debuginfo: false
  source: root lock (profile fedora-44-build)
```

This is where rpmx can leapfrog Melange: `melange compile` shows the resolved
document, but not why a given package, flag, or step is present. rpmx already
records the inputs; it just needs a renderer.

**Acceptance.** Each subcommand prints provenance for every value and has a
`--json` form. `explain root` must name the recipe line or step that pulled in
each package.

---

### P1.3 — Iteration verbs: `shell`, `--interactive`, `--keep-workdir`

**Problem.** After a failure there is nothing to inspect. The temp workspace is
deleted. There is no way to reproduce the environment by hand.

**Design.**

```
rpmx shell package.yaml [--stage build|install|test] [--step NAME|N]
rpmx build --keep-workdir [--interactive]
rpmx test ... --interactive
```

- `--keep-workdir` prints the workspace, stage, and log paths.
- `--interactive` on failure drops into a shell in the same sandbox with the
  same mounts and environment; exiting returns the original failure.
- `rpmx shell` starts the sandbox at the requested stage without running the
  rest of the pipeline.
- `rpmx build --debug` sets `set -x` for `runs:` steps (or `PS4` equivalent),
  and `--debug-runner` prints the runner command line.

This is the highest-value debugging feature in Melange and the cheapest to
copy conceptually.

**Acceptance.** A failing build followed by `--interactive` gives a shell where
`pwd` is the step's cwd and the toolchain is present. `--keep-workdir` leaves a
directory that `--root`/workspace flags can reuse on the next run.

---

### P1.4 — `files --suggest`: keep strict ownership, remove the guesswork

**Problem.** rpmx's exhaustive file selectors are a strength, but authoring them
is the worst part of the recipe. Melange avoids it by splitting at install
time; rpmx requires the user to know the whole installed tree in advance.

**Design.**

```
rpmx files package.yaml --stage DIR
rpmx files package.yaml --stage DIR --suggest
```

- `--stage DIR` scans an existing staged tree (from `build --stage` or a kept
  workdir) and reports:
  - files claimed by no output (with a suggested output based on path class);
  - files claimed by more than one output;
  - `files.include` entries that match nothing.
- `--suggest` prints a ready-to-paste `outputs:` block using the standard
  classes (bin, libs, devel, doc, locale, config, systemd, licenses).

This turns the strict model into an advantage: the tool tells you the exact
selectors, and the build still proves totality. Optionally add
`auto-split: [libs, devel, doc, locale]` later, but `--suggest` is better
because it produces reviewable explicit selectors.

**Acceptance.** `rpmx files c-hello --stage /tmp/stage --suggest` prints a block
that, pasted into the recipe, lints clean and matches the same files.

---

### P1.5 — Make the step library visible

**Problem.** The built-in steps are embedded YAML, but there is no way to list
them, read one, see its inputs/needs, or scaffold a local one. `plan` shows the
expanded shell but not the contract.

**Design.**

```
rpmx steps list [--json]
rpmx steps show cmake/configure [--json]   # inputs, needs, digest, source
rpmx steps new acme/strip                   # scaffold pipelines/acme/strip.yaml
rpmx steps check                            # validate local steps
```

`plan` should print, per step: `uses`, version, digest, inputs bound, and
`needs`. This is already recorded; it is just not rendered
(`render.rs::step_summary`).

Also accept `--pipeline-dirs` (plural), matching Melange, and keep
`--pipeline-dir` as an alias.

**Acceptance.** `rpmx steps show python/build-wheel` prints the YAML and the
resolved needs. `plan` shows digests and inputs.

---

### P1.6 — `doctor`

**Problem.** rpmx depends on bwrap, podman, dnf5, qemu-system, qemu-user binfmt,
ssh, mke2fs, debugfs, tar, and a writable cache. When one is missing, the error
arrives deep inside a build.

**Design.**

```
rpmx doctor [--json]
```

A table of checks with `ok`/`warn`/`fail`, detected version/path, and a
remediation line. Exit non-zero only for checks required by the selected runner
or command. `rpmx build --root-image` should run the relevant subset first.

Example:

```
ok    bubblewrap      0.8.0   /usr/bin/bwrap
ok    podman          5.2.1   /usr/bin/podman
warn  qemu-system     9.1.0   foreign builds only
fail  dnf5             -       needed to resolve fedora-44
      install dnf5 in the builder image or pass --root-lock
ok    cache           /home/u/.cache/rpmx   12.4 GiB free
```

This is a small command with a large support-cost payoff.

---

### P1.7 — Cache and repository ergonomics

**Design.**

```
rpmx cache info [--json]        # size, entries, oldest/newest, referenced roots
rpmx cache verify               # re-hash blobs, report corruption
rpmx cache gc --dry-run|--apply [--older-than 24h]
```

- Rename `--cache` to `--cache-dir` everywhere and honour
  `$RPMX_CACHE_DIR`.
- `rpmx repo` accepts positional RPMs and globs (`rpmx repo packages/*.rpm -o
  repo/`), matching `melange index`.
- `rpmx build --index` defaults to **true**: after a successful build write
  repodata for the output directory. `--no-index` opts out.
- Default output directory becomes `./packages/<arch>/`; `--out-dir` is an
  alias. The current `.` default pollutes the recipe directory.
- `rpmx repo --merge` merges an existing index instead of replacing it.

**Acceptance.** A fresh `rpmx build` produces a directory that `dnf` can consume
with `baseurl=/path` and no extra command.

---

### P1.8 — `bump` and `update`

**Problem.** Version bumps are manual: edit `version`, edit the source URL,
trigger a digest mismatch, copy a 128-character hash out of an error. Melange's
`bump` is one command.

**Design.**

```
rpmx bump package.yaml 1.2.3 [--dry-run] [--commit] [--source URL]
rpmx update package.yaml          # check upstream, print available version
```

`bump` rewrites `package.version`, updates `package.release`/epoch if asked,
updates source URLs that contain the old version, re-fetches and writes the new
digest, and prints a unified diff. `--dry-run` shows the diff without writing.
It must preserve the file's formatting and comments. Melange's `bump` reformats
the whole YAML and prints no diff; that is the one part not to copy.
**[verified]**

Add an `update:` block later (P2.5) so `rpmx update` knows where to look
(GitHub, release-monitoring, git, OCI) and `rpmx bump --latest` can pick the
version. Do not copy Melange's regex `var-transforms`; use explicit version
fields.

**Acceptance.** `rpmx bump --dry-run` on `examples/zstd/package.yaml 1.5.8`
prints a diff touching exactly `package.version`, the source URL, and the
digest. `--commit` writes it and the recipe lints.

Also make `rpmx fetch --write-digest` update the recipe after a digest
mismatch, so the bootstrap path does not require copy-paste.

---

### P1.9 — A config file, and consistent global flags

**Problem.** Every command repeats `--json`; there is no way to set a default
profile, cache dir, jobs, output dir, runner, or signing key once.

**Design.** `~/.config/rpmx/config.yaml` (and `--config FILE`):

```yaml
root:
  profile: fedora-44
cache: ~/.cache/rpmx
jobs: auto
output: ./packages
runner: auto
color: auto
signing:
  key: ~/.config/rpmx/signing-key.asc
  passphrase_env: RPMX_PASSPHRASE
repos:
  - id: local
    baseurl: file:///srv/rpmx/packages
```

Rules: the config may set **paths and defaults**, never anything that changes
the meaning of a recipe (no profile overlay values, no features, no matrix
overrides). Those stay in the recipe, the profile, or an explicit flag. Print
the config path and any values used in `--verbose`.

Add global flags: `--json`, `--quiet`, `--verbose`, `--debug`, `--color`,
`--cache-dir`, `--jobs`, `--offline`, `--locked`, `--keep-workdir`,
`--config`.

---

### P2.1 — Sources: extraction, formats, digests, signatures

**Problem.** Archives are unpacked only when the file name ends in
`.tar.gz`/`.tgz`, and only gzip is supported. There is no `extract` block, no
`strip-components`, no destination, no signature verification, and sha512 is
the only accepted digest. Most upstreams publish sha256.

**Design.** Add to the source model:

```yaml
sources:
  - name: curl
    kind: archive
    digest: "sha512:..."
    locators: [...]
    signature:
      type: openpgp
      url: https://curl.se/download/curl-8.11.1.tar.xz.asc
      fingerprint: "..."
    extract:
      format: auto          # auto|tar.gz|tar.xz|tar.zst|zip|none
      strip-components: 1   # default 1, matching Melange
      destination: .
```

- Detect format by content (magic bytes), not file name. Keep `format: auto`
  as the default.
- Support gzip, xz, zstd, and zip. This is a small dependency cost and removes
  a constant source of "why is my tarball a single file?" confusion.
- Default `strip-components: 1`; recipes that need the old behaviour set `0`.
- Accept `sha256:` as a source digest **and** record the algorithm in the lock.
  Add a profile/policy `require: sha512` for distributions that need it. The
  design's SHA-512-only rule is a good default but a bad adoption barrier.
- Verify OpenPGP signatures when declared or required by policy; record the
  result in the lock and SBOM.

**Acceptance.** A `.tar.xz` source with `strip-components: 1` lands its contents
at the workspace root with no `cd`. A sha256 source builds and the lock records
`sha256:`. A bad signature fails before source assembly with a dedicated code.

---

### P2.2 — Features

**Problem.** Features are documented as a core concept (`docs/03` §4.6,
`docs/13` §10) and used by `examples/curl`, but absent from `model.rs`. A
packager who wants optional TLS/LDAP support has no supported way to express it
except duplicated recipes.

**Design.** Implement the documented minimum:

```yaml
features:
  default: [ssl]
  ssl:
    summary: TLS support
    requires: [tls]
    conflicts: [gnutls]
    dependencies:
      host: [openssl-devel]
    provides:
      run: [curl-tls]
    pipeline:
      - at: cmake/configure
        inputs:
          args:
            append: [-DCURL_USE_OPENSSL=ON]
```

- Resolve to a fixed point before planning; record the enabled set in the plan,
  lock, and SRPM.
- `rpmx build --features +idn,-http2` and `--features all`.
- `rpmx plan --features all` reports invalid combinations.
- Merge contributions into named expanded steps. Reject two features that `set`
  the same input to different values.
- `rpmx explain features` shows the dependency graph and why each is enabled.

Do **not** add `if:` expressions. Features plus matrix cells cover the real
cases, and they are checkable.

**Acceptance.** `examples/curl/package.yaml` lints and plans after the
unimplemented fields are either implemented or removed from the example. A
build with `--features +idn` adds the dependency and the configure flag; a
build with `--features all` reports the `ssl`/`gnutls` conflict.

---

### P2.3 — Typed actions, ghost files, generators, auto-split

**Problem.** The only system-integration interface is raw shell scriptlets, and
the common cases (systemd unit enablement, sysusers, tmpfiles, ldconfig) are
hand-written boilerplate. `docs/03` §4.8 specifies typed actions; none exist.

**Design.** Implement the high-value subset first:

```yaml
outputs:
  - name: hello-daemon
    actions:
      - type: systemd
        units: [hello.service]
        enable: [hello.service]
        restart-on-upgrade: true
      - type: sysusers
        file: files/hello.sysusers
      - type: tmpfiles
        file: files/hello.tmpfiles
      - type: ldconfig
    files:
      attributes:
        - path: /var/log/hello.log
          ghost: true
          mode: "0640"
```

- Emit from a typed IR, not string templates. Validate that referenced unit and
  config files exist in the output.
- Add `ghost`, `lang`, `verify`, and `artifact` file attributes.
- Keep raw `scriptlets` as the escape hatch, but lint them (shellcheck,
  forbidden commands, ownership of touched paths) and require a `reason` for
  `posttrans`/`pretrans`.
- Add per-output `generators: {elf: true, pkgconfig: true}` and surface
  generated edges in `explain requires`.
- Add `auto-split: [libs, devel, doc, locale]` only after `files --suggest`
  ships; prefer explicit selectors.

**Acceptance.** `examples/hello-daemon` is expressed with actions, not
scriptlets, and the resulting RPM has equivalent scriptlets (compare with
`rpm -qp --scripts`).

---

### P2.4 — Tests: kinds, per-output scope, reproducibility

**Design.** Extend the test model (see P0.5) with:

- `kind: command|abi|reproduce`;
- `dependencies` per test;
- `timeout` per test;
- `requires` feature gating;
- `stage: package` operating on the RPMs (reproducibility, manifest, signature);
- top-level tests (`integration`) and per-output tests.

`rpmx test --reproduce` builds twice in different roots/directories and diffs
payloads and headers, classifying expected differences (signature, build time)
versus content.

**Acceptance.** `rpmx test --reproduce examples/zstd/package.yaml` passes on a
deterministic recipe and reports the first differing path on a non-deterministic
one.

---

### P2.5 — Changelog, release auto, and update metadata

**Design.**

```yaml
package:
  release:
    auto: git       # commit count since the last version tag, plus base
    base: 1
  changelog:
    generate: git   # or an explicit list
update:
  enabled: true
  github:
    identifier: facebook/zstd
    strip-prefix: v
```

- `rpmx lock` resolves `release.auto` and writes the integer plus the inputs;
  `rpmx build --locked` reproduces it.
- The RPM changelog is emitted from the lock, never recomputed at build time.
- `rpmx update` checks the declared source and prints the available version;
  `rpmx bump --latest` applies it.
- `rpmx bump --commit` updates the git locator's `commit` pin.

**Acceptance.** A recipe with `release.auto: git` and 12 commits since the tag
gets release `13` (base 1 + 12), recorded in the lock. Two builds agree. The
changelog in the RPM matches the lock.

---

### P2.6 — Decide what a profile is

**Problem.** The design's profile is a policy overlay with `paths.prefix`,
`paths.libdir`, build flags, allowed actions, and policy booleans. The
implementation's `Profile` is still mostly a root recipe: repositories,
packages, releasever, dist tag, debuginfo, sbom. `paths.prefix` and
`paths.libdir` are now recorded in the lock and exported to steps as
`PREFIX`/`LIBDIR`; the remaining policy flags are not implemented. `--profile`
on `build` still only reads the dist tag.

**Design.** Either:

- **A. Expand the profile** to the documented model: `paths.prefix`,
  `paths.libdir`, `build.environment` defaults, `policy.*` (allowed actions,
  forbid shell, require tests, unowned files, license files), and make steps
  use `${prefix}`/`${libdir}` instead of literals. Then `--profile` means the
  same thing everywhere.
- **B. Shrink the concept**: rename the current file to a **root profile**
  (`--root-profile`), remove the policy sections from the design, and keep
  distro policy in the recipe or in explicit flags.

Recommendation: **A**, but only the parts users need now: `prefix`, `libdir`,
`build.environment` defaults, `policy.unowned-files`, `policy.require-tests`,
`policy.forbid-shell-actions`. Fix the built-in steps to use the template
variables so profiles are not a lie.

**Acceptance.** A `usr-local` profile changes the install prefix and the
built-in steps honour it. `rpmx explain policy` prints every resolved value and
its source.

---

### P2.7 — Post-build linters (the other half of `lint`)

**Problem.** `rpmx lint` runs before the build and cannot see the package.
Melange runs package linters after assembly and fails the build on them. rpmx
has the file manifest, so it can do the same checks more precisely, but it does
not.

**Design.** Two phases under one command:

```
rpmx lint package.yaml                 # recipe lint (today's parse + P0.3 rules)
rpmx lint package.yaml --stage DIR     # package lint against a staged tree
rpmx build ... [--lint=default|strict|off]
```

Post-build checks, on by default for `build`:

| Check | Failure |
|---|---|
| usrmerge (`/bin`, `/sbin`, `/lib*` are symlinks) | error |
| world-writable files | error |
| setuid/setgid files | error unless `-compat` or disabled |
| files under `/tmp`, `/var/empty`, `/usr/local`, `/opt` | error unless named `-compat` |
| `.la` files | warning |
| unstripped ELF | warning (the `strip` step is the fix) |
| no license file in the package | warning |
| config files outside `/etc` | warning |
| man pages not compressed | warning |
| Python bytecode without source | warning |
| SBOM missing when policy says `sbom: true` | error |

A recipe disables a check with a reason that is recorded in provenance:

```yaml
package:
  checks:
    disabled:
      - setuidgid
        reason: ping is intentionally setuid for unprivileged ICMP
```

This is stricter and more auditable than Melange's comment convention, and it
uses the manifest rpmx already has.

**Acceptance.** A package containing a setuid binary fails
`rpmx build` with a path and a remediation. A `checks.disabled` entry with a
reason passes and appears in `rpmx explain provenance`. `--lint=off` is
recorded in provenance and rejected by `policy.require-lint`.

---

### P3 — Polish

- `rpmx completions bash|zsh|fish` and `rpmx man`, generated by clap.
- `--version` prints the git commit and build date.
- `rpmx plan --format=dot|markdown`; `rpmx graph`.
- `rpmx sbom FILE`, `rpmx attest` (design already specifies the interfaces).
- `--env-file FILE` and `--vars name=value` / `--vars-file` for iteration.
  `plan` shows the bindings.
- `--arch` validated by clap with possible values, repeatable, and `--arch all`
  for matrix builds. Keep foreign-arch execution explicit.
- `--trace FILE` (runner command trace) and `--create-build-log` /
  `--dependency-log` (provenance).
- `rpmx fmt` (canonical YAML) and `rpmx migrate` (schema upgrades).
- `rpmx import-spec` (the design's migration path) with the documented
  confidence report.
- `rpmx clean` to remove work dirs and stale cache entries.
- Colored, clickable `file:line` diagnostics in terminals that support it.
- Auto-detect CI and switch to plain progress; never emit ANSI when not a TTY.

---

## 7. Target CLI surface

This is the surface I would ship. Plumbing verbs remain, but they are no longer
the documented path.

```
rpmx init [DIR] [--template T] [--profile P]
rpmx lint [FILE] [--strict] [--json] [--format text|json|github]
rpmx lock [FILE] [--update] [--check] [--output FILE]
rpmx plan [FILE] [--format text|json|dot|markdown] [--features SPEC]
rpmx fetch [FILE] [--write-digest] [--offline]
rpmx source [FILE]
rpmx srpm [FILE] [--sources DIR]
rpmx build [FILE] [--locked] [--run-tests] [--index|--no-index]
             [--out-dir DIR] [--jobs N|auto] [--features SPEC]
             [--interactive] [--keep-workdir] [--debug]
rpmx test [FILE] [--test NAME]... [--stage S] [--install-dir DIR]
             [--junit FILE] [--interactive] [--keep-workdir]
rpmx rebuild SRPM [--root DIR|--root-image IMG|--offline]
rpmx shell [FILE] [--stage S] [--step N]
rpmx explain requires|files|policy|root|steps|matrix|features|provenance
rpmx diff OLD NEW [--files|--deps|--json]
rpmx files [FILE] --stage DIR [--suggest]
rpmx steps list|show|new|check
rpmx root resolve|assemble|digest|show|export|import   # plumbing
rpmx repo|index ...
rpmx keygen|sign ...
rpmx inspect-srpm|verify-srpm|extract-srpm
rpmx cache info|verify|gc
rpmx doctor
rpmx bump FILE VERSION [--dry-run] [--commit]
rpmx update FILE
rpmx completions|man
```

Global flags: `--config`, `--cache-dir`, `--jobs`, `--offline`, `--locked`,
`--json`, `--quiet`, `--verbose`, `--debug`, `--color`, `--keep-workdir`,
`--interactive`, `--progress`.

Defaults:

- `<FILE>` → `./package.yaml`; a directory argument resolves to
  `package.yaml`.
- Output → `./packages/<arch>/`.
- `--jobs` → `auto` (nproc).
- `--index` → true after `build`.
- `--run-tests` → false (but `rpmx test` is first-class, and `rpmx build
  --run-tests` links to it on failure to run tests).
- Color/progress → auto by TTY.

---

## 8. Target recipe schema delta

Add:

- `build.root` (P0.1).
- `features` (P2.2).
- `outputs[].actions` (P2.3).
- `outputs[].tests` (P0.5).
- `sources[].extract`, `signature`, `mirrors` (P2.1).
- `patches[].when`, `resolves` (P2.1/P2.2).
- `tests[].environment`, `dependencies`, `kind`, `timeout`, `requires`.
- `package.changelog`, `release.auto` (P2.5).
- `package.checks.disabled` with a required `reason` (P2.7, post-build
  linting).
- `package.architectures`, `unowned-files`, `ignore-files` (design already).
- `file attributes`: `ghost`, `lang`, `verify`, `artifact`.
- `update` (P2.5).

Remove or rename:

- `outputs[].scriptlets` → keep as `scriptlets` escape hatch, but document
  `actions` as the primary interface.
- `build.dependencies.tools/host/target` stay, but now feed root resolution.
- `schema: 1` stays, but `init` writes it and `migrate` handles future majors.

Do not add: `if:` expressions, `var-transforms` regexes, arbitrary shell in
templates. Those are the parts of Melange that make recipes unanalyzable, and
rpmx's strictness is its differentiator.

---

## 9. Breaking changes to make now (there are no users)

Because there is no installed base, do these in one release rather than
carrying compatibility shims:

1. Rename `--cache` → `--cache-dir`. Keep `--step-timeout`; add `--timeout`
   as the build-wide limit.
2. Rename `--output` → `--out-dir` on build/test/rebuild/repo, with `--output`
   accepted as an alias only if it costs nothing. Prefer the Melange-familiar
   name.
3. Make `--profile` mean the root profile everywhere, or split into
   `--root-profile` and `--dist-tag`. Do not leave it as "dist tag only".
4. Split `build --stage` into `rpmx package --stage DIR` (or
   `rpmx build --from-stage`), removing the 9-way conflict list.
5. Make `srpm` do source assembly itself unless `--sources` is given; stop
   exposing `source` as a prerequisite.
6. Change all exit codes to the design table.
7. Move `--json` to a global flag; make every document versioned.
8. Default output to `./packages/<arch>/`, jobs to `auto`, file argument to
   `package.yaml`.
9. Delete `fmt` from the roadmap unless it ships; it is currently a promise
   with no implementation.
10. Update README, `docs/03`, `docs/13`, and `examples/*/plan.md` in the same
    change as any behavior change. The current docs describe a tool that does
    not exist, which is its own UX problem.

---

## 10. Roadmap

Milestones are ordered by user-visible payoff. Each has an exit criterion that
can be tested from a clean checkout.

### M1 — "It builds like Melange" (P0.1, P0.2, P0.6, P0.7)

- `build.root` and automatic root resolution with dependency merging.
- Reporter with phases, steps, timings, failure context, `--keep-workdir`.
- `rebuild` assembles its embedded root.
- Clean JSON and exit codes.

**Exit:** `rpmx build examples/zstd/package.yaml` from a clean checkout, no
flags, produces RPMs and an index; a forced failure prints command/log/tail;
`--json | jq` works; `rpmx rebuild out/src/*.src.rpm` works with no other flags.

### M2 — "It tells you what is wrong" (P0.3, P0.4, P1.2)

- Real linter with multiple diagnostics, suggestions, warnings, `--strict`.
- `package.lock.json`, `--locked`, `--check`, diff output.
- `explain` for policy/root/requires/files.

**Exit:** three mistakes are reported at once; a drifted lock exits 3; every
value in `explain policy` has a provenance line.

### M3 — "It iterates" (P1.1, P1.3, P1.4, P1.5, P1.6, P0.5)

- `init`, `shell`, `--interactive`, `files --suggest`, `steps`, `doctor`.
- First-class `test` with selection, scoped environments, JUnit.

**Exit:** `rpmx init hello && rpmx build` works; a failing step can be entered
interactively; `rpmx test --test installed --install-dir` runs one test without
rebuilding; `files --suggest` output lints clean.

### M4 — "It ships" (P1.7, P1.8, P2.1–P2.7)

- Index by default, cache verbs, `bump`/`update`.
- Source extraction/formats/digests/signatures.
- Features, actions, test kinds, post-build linters, changelog/release auto,
  profile capabilities.

**Exit:** a realistic package with systemd units, a `.tar.xz` source, sha256
digest, optional feature, and per-output test builds, lints, tests, and
publishes a signed repo in one command chain.

### M5 — Polish (P3)

Completions, man pages, graph/sbom/attest, fmt/migrate/import-spec, CI
annotations.

---

## 11. What not to copy from Melange

- **`if:` expressions in steps.** They make recipes unanalyzable and reintroduce
  `%if`. Use features and matrix cells.
- **Shell templating as the primary interface.** Shell is an escape hatch; typed
  actions and steps are the interface.
- **Regex `var-transforms`.** Version mangling should be explicit fields.
- **Exhaustive `-compat` lint culture.** Adopt the useful linters, not the
  naming dogma.
- **A 50-flag build command.** Melange's surface is wide because it supports
  many runners and remote services. rpmx should expose a small set of
  task-oriented flags and a config file, and hide runner plumbing behind
  `doctor` and automatic selection.
- **Hidden state.** Melange's automatic runner and apko environment are
  convenient but hard to audit. rpmx's root digest and lock are the antidote;
  keep them visible in `plan` and `explain`.

---

## Appendix A — Evidence transcripts

### A.1 Clean-checkout build fails

```
$ rpmx build examples/zstd/package.yaml
RPMX-ROOT-016: a full build requires --root-lock and --root for source assembly; pass --unsafe-host-root to use the host root
EXIT=2
```

### A.2 Successful build output has no structure

```
$ rpmx build examples/c-hello/package.yaml --root-lock /tmp/build.assembled.yaml \
    --root /tmp/rpmx-root --output /tmp/out2 --run-tests
cc -O2 -Wall -Wextra -Werror -o c-hello hello.c
./c-hello
hello from a locked root
install -D -m 0755 c-hello /tmp/stage/usr/bin/c-hello
ln -sf c-hello /tmp/stage/usr/bin/c-hello-alias
hello from a locked root
hello from a locked root
/tmp/out2/src/c-hello-1.0-1.fc44.src.rpm
/tmp/out2/c-hello-1.0-1.fc44.x86_64.rpm
EXIT=0
```

### A.3 Failure has no context

```
$ rpmx build fail.yaml --root-lock ... --root ...
compiling...
something went wrong
RPMX-SANDBOX-002: step 'compile' failed with exit 3
EXIT=2
```

### A.4 `--json` is corrupted by step stdout

```
$ rpmx build ... --json 2>/dev/null | jq -r .ok
cc -O2 -Wall -Wextra -Werror -o c-hello hello.c
./c-hello
hello from a locked root
jq: parse error: Invalid numeric literal at line 1, column 3
```

### A.5 `lint` accepts invalid metadata

```
$ rpmx lint weak.yaml
ok: 1 cell(s): default
EXIT=0
```

Recipe: `release: 0`, `summary: "a"`, `license: "MIT OR"`, `tools: [gcc, gcc]`.

### A.6 `plan` disagrees with `build` on release

```
$ rpmx plan examples/c-hello/package.yaml
default  slug= release=1 package=c-hello

$ rpmx build ... --root-lock /tmp/build.assembled.yaml ...
/tmp/out2/c-hello-1.0-1.fc44.x86_64.rpm
```

### A.7 `rebuild` cannot use its embedded root

```
$ rpmx rebuild /tmp/out/src/c-hello-1.0-1.fc44.src.rpm --output /tmp/rebuilt
RPMX-ROOT-016: a root lock is available; pass --root with the assembled root directory
EXIT=2
```

### A.8 `root digest` fails on an assembled root

```
$ rpmx root assemble /tmp/build.lock.yaml --root /tmp/rpmx-root2 --output /tmp/out.yaml
... root_digest=sha512:bb44...
$ rpmx root digest /tmp/rpmx-root2
RPMX-ROOT-014: cannot read /tmp/rpmx-root2/etc/gshadow: Permission denied (os error 13)
EXIT=2
```

### A.9 Design example does not parse

```
$ rpmx lint examples/curl/package.yaml
RPMX-YAML-001: package: unknown field `unowned-files`, expected one of ...
EXIT=2
```

### A.10 Error JSON has no suggestions and one diagnostic

```
$ rpmx lint --json /tmp/bad.yaml
{"code":"RPMX-YAML-001","column":3,"line":8,"message":"package: unknown field ...","path":null,"severity":"error"}
EXIT=2
```

### A.11 Root workflow timing

```
$ time rpmx root resolve examples/fedora-44-build.profile.yaml --output /tmp/build.lock.yaml
fedora-44-build x86_64 209 packages closure=sha512:8255...
real 0m17.159s

$ time rpmx root assemble /tmp/build.lock.yaml --root /tmp/rpmx-root --output /tmp/build.assembled.yaml
fedora-44-build x86_64 209 packages root=/tmp/rpmx-root root_digest=sha512:bb44...
real 0m14.540s
```

---

## Appendix B — Melange UX inventory (source material)

Inventory is from melange **v0.63.2** (the release tested in Appendix C) plus
the `main` docs.

Commands: `build`, `test`, `bump`, `compile`, `query`, `package-version`,
`lint`, `index`, `sign`, `sign-index`, `keygen`, `keygen`, `scan`,
`update-cache`, `source get|pop`, `convert apkbuild|gem|python`, `initramfs`,
`completion`, `version`.

Build flags that shape the loop: `--out-dir` (default `./packages/`),
`--cache-dir`, `--pipeline-dir(s)`, `--runner`, `--arch`,
`--repository-append`, `--keyring-append`, `--package-append`,
`--env-file`, `--vars-file`, `--interactive`, `--debug`, `--debug-runner`,
`--trace`, `--create-build-log`, `--dependency-log`, `--generate-index`
(default true), `--generate-provenance`, `--lint-require`, `--lint-warn`,
`--fail-on-lint-warning`, `--log-level`, `--timeout`, `--memory`, `--cpu`,
`--disk`, `--source-dir`, `--workspace-dir`, `--empty-workspace`.

Testing flags: `--test-option`, `--test-package-append`,
`--pipeline-dirs`, `--source-dir`, selection of the package under test by
`name` or `name=version`.

Config concepts: `package`, `environment.contents`
(repositories/keyring/packages), `pipeline`, `subpackages`, `test`, `data`,
`vars`, `var-transforms`, `options`, `update`, `copyright`, `scriptlets`,
`resources`, `checks.disabled`, `capabilities`.

Pipeline concepts: `uses`, `runs`, `with`, `if`, `working-directory`,
`environment`, `needs.packages`, `needs.capabilities`, `assertions`,
`label`, nested `pipeline`.

Split helpers: `split/dev`, `split/manpages`, `split/static`, `split/infodir`,
`split/locales`, and `${{targets.destdir}}` / `${{targets.subpkgdir}}`.

Templates: `${{package.*}}`, `${{targets.*}}`, `${{subpkg.*}}`,
`${{build.arch}}`, `${{vars.*}}`, `${{range.*}}`.

Post-build linters: required `dev`, `infodir`, `libtool/la-files`,
`setuidgid`, `tempdir`, `usrmerge`, `varempty`, `worldwrite`; warned
`binaryarch`, `cudaruntimelib`, `dll`, `duplicate`, `dylib`, `lddcheck`,
`maninfo`, `nonlinux`, `object`, `opt`, `pkgconf`, `python/docs`,
`python/multiple`, `python/test`, `sbom`, `srv`, `staticarchive`, `strip`,
`unsupportedarch`, `usrlocal`.

---

## Appendix C — First-hand Melange transcripts

Binary: `melange v0.63.2` linux/amd64, bubblewrap runner, Fedora 44 host.
Working directory `/tmp/melange-test`. Config `probe.yaml` installs `busybox`,
writes one file into `${{targets.destdir}}`, echoes to stdout and stderr, and
declares one test:

```yaml
package:
  name: hello-ux
  version: "1.0.0"
  epoch: 0
  description: "UX probe"
  copyright:
    - license: MIT
environment:
  contents:
    repositories:
      - https://packages.wolfi.dev/os
    keyring:
      - https://packages.wolfi.dev/os/wolfi-signing.rsa.pub
    packages:
      - busybox
pipeline:
  - runs: |
      mkdir -p "${{targets.destdir}}/usr/share/hello-ux"
      echo hello > "${{targets.destdir}}/usr/share/hello-ux/README"
      echo "this goes to stdout"
      echo "this goes to stderr" >&2
test:
  pipeline:
    - runs: |
        cat /usr/share/hello-ux/README
        echo "test output"
```

The successful test run adds a `test.environment.contents` block with
`busybox`, `hello-ux`, the Wolfi repository, and the local `./packages`
repository.

### C.1 One command, resolved environment printed

```
$ time melange build probe.yaml --arch x86_64
INFO melange v0.63.2 with runner bubblewrap is building:
INFO populating workspace /tmp/melange-workspace-2272973612 from .
INFO image configuration:
INFO   contents:
INFO     repositories: [https://packages.wolfi.dev/os]
INFO     keyring:      [https://packages.wolfi.dev/os/wolfi-signing.rsa.pub]
INFO     packages:     [busybox=1.38.0-r3 ca-certificates-bundle=20261020-r0 glibc-2.44-locale-posix=2.44-r11 glibc-2.44=2.44-r11 ld-linux-2.44=2.44-r11 libcrypt1-2.44=2.44-r11 libgcc=16.2.0-r2 libxcrypt=4.5.2-r6 wolfi-baselayout=20230201-r31]
INFO installing wolfi-baselayout (20230201-r31)
...
INFO this goes to stdout
WARN this goes to stderr
INFO running package linters for hello-ux
INFO no lint findings to persist for package hello-ux
INFO generating package hello-ux-1.0.0-r0
INFO wrote packages/x86_64/hello-ux-1.0.0-r0.apk
INFO generating apk index from packages in packages/x86_64
INFO updating index at packages/x86_64/APKINDEX.tar.gz with new packages: [hello-ux-1.0.0-r0]

real 0m3.100s
EXIT=0
```

### C.2 A failing step

```
$ melange build fail.yaml --arch x86_64
INFO running step "prepare"
INFO preparing
INFO running step "compile"
INFO compiling...
WARN something went wrong
ERRO failed to build package: unable to run package fail-ux pipeline: unable to run pipeline: exit status 3
EXIT=1
```

### C.3 `--debug` shows the commands

```
$ melange build debug.yaml --arch x86_64 --debug
WARN + mkdir -p /home/build/melange-out/hello-ux/usr/share/hello-ux
WARN + echo hello
WARN + echo 'this goes to stdout'
WARN + echo 'this goes to stderr'
EXIT=0
```

### C.4 Test without a shell in the test environment

```
$ melange test probe.yaml --arch x86_64 --repository-append ./packages --ignore-signatures
INFO     packages:     [hello-ux]
INFO installing hello-ux (1.0.0-r0)
WARN bwrap: execvp /bin/sh: No such file or directory
ERRO ERROR: failed to test package. the test environment has been preserved:
ERRO failed to test package: unable to start pod: exit status 1
EXIT=1
```

### C.5 Test success, with per-test environment

After adding `busybox`, `hello-ux`, and the repositories to
`test.environment.contents`:

```
$ melange test probe-test.yaml --arch x86_64 --ignore-signatures
INFO     repositories: [https://packages.wolfi.dev/os ./packages]
INFO     packages:     [busybox hello-ux]
INFO installing hello-ux (1.0.0-r0)
INFO running the main test pipeline
INFO hello
INFO test output
EXIT=0
```

### C.6 `compile` is a reviewable resolved document

```
$ melange compile probe.yaml --arch x86_64
{"package":{"name":"hello-ux","version":"1.0.0","epoch":0,...},
 "environment":{"contents":{...},"environment":{"GOMODCACHE":"/var/cache/melange/gomodcache",...}},
 "pipeline":[{"runs":"mkdir -p \"/home/build/melange-out/hello-ux/usr/share/hello-ux\"\n..."}],
 "test":{"environment":{"contents":{"packages":["hello-ux"]}},"pipeline":[...]}}
EXIT=0
```

### C.7 Quick queries

```
$ melange query probe.yaml '{{ .Package.Name }}-{{ .Package.Version }}-r{{ .Package.Epoch }}'
hello-ux-1.0.0-r0
$ melange package-version probe.yaml
hello-ux-1.0.0-r0
```

### C.8 `bump` rewrites formatting and hides the diff

```
$ melange bump bump.yaml 1.0.1
INFO attempting to bump version to 1.0.1
EXIT=0

$ diff -u probe.yaml bump.yaml
@@ -1,11 +1,10 @@
 package:
   name: hello-ux
-  version: "1.0.0"
+  version: "1.0.1"
   epoch: 0
   description: "UX probe"
   copyright:
     - license: MIT
-
 environment:
 ...
```

(Blank lines throughout the document were removed.)

### C.9 Unknown fields are rejected

```
$ melange compile strict.yaml --arch x86_64
ERRO failed to load configuration: unable to decode configuration file "strict.yaml": yaml: unmarshal errors:
  line 8: field frobnicate not found in type config.Package
EXIT=1
```

### C.10 Default config and default index

```
$ cd defaultcfg   # contains melange.yaml
$ melange build --arch x86_64
INFO melange v0.63.2 with runner bubblewrap is building:
INFO wrote packages/x86_64/hello-ux-1.0.0-r0.apk
INFO generating apk index from packages in packages/x86_64
EXIT=0
$ ls packages/x86_64
APKINDEX.tar.gz  hello-ux-1.0.0-r0.apk
```
