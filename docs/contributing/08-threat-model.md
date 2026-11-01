# Security threat model: proxies, SRPMs and build inputs

## 1. Scope

This covers the path from upstream source to installed binary:

```
upstream source / Go module proxy / registry
                    |
              egress proxy
             (untrusted cache)
                    |
                rpmx fetch
          (verify digest/signature)
                    |
                local CAS
                    |
              SRPM assembly
           (embed verified bytes)
                    |
              SRPM storage
          (signed, untrusted until verified)
                    |
              build sandbox
          (no network, no secrets)
                    |
                binary RPMs
             (signed, repo metadata)
                    |
                 consumer
```

The question is not "is the proxy secure?" The question is:

> **If the proxy, the upstream, the SRPM, or the build environment is compromised, what stops a malicious artifact from reaching a user?**

## 2. Assets

- The source bytes and their identity (digest).
- The recipe and lock (the intended build).
- The SRPM and its signature.
- The binary RPMs and their signatures.
- Signing keys and repository metadata.
- Build secrets and credentials (ideally none in the sandbox).
- SBOM, provenance and reproducibility evidence.
- Consumer systems that run package scriptlets as root.

## 3. Trust boundaries and trust anchors

| Boundary | Trusted? | Anchor |
|---|---|---|
| Upstream project | Not inherently | Upstream signature, release process, review, reputation |
| Egress/content proxy | **No** | Digest and signature verification after fetch |
| Local CAS | **No** | Re-hash before use |
| Recipe/lock in Git | Only after review | Signed commits/tags, protected branch, review |
| SRPM | **No** until verified | SRPM signature + manifest + internal digests |
| Build sandbox | Assumed hostile | Hermetic isolation, no network, no secrets |
| Build toolchain | Not inherently | Pinned toolchain, provenance, independent rebuilds |
| Signing service/key | **Yes**, by definition | HSM/offline key, threshold, audit, transparency |
| Binary RPM/repo | **No** until verified | RPM signature, signed repo metadata |
| Consumer | Must verify | Package manager signature policy |

The most important sentence in this document:

> **Content addressing protects integrity only if the expected hash is trusted independently of the content being fetched.**

If the proxy can serve both the source and the lock that contains its hash, the hash proves nothing.

## 4. Threats and controls

### T1. Proxy substitution or MITM

**Threat:** The proxy, a network attacker, or a compromised mirror serves different bytes than the upstream.

**Impact:** Malicious source code, backdoored binary, or a denial of service.

**Controls:**

- TLS to the proxy; authenticated proxy access where appropriate.
- Verify the digest from the trusted lock **after** fetch, before inserting into the CAS.
- Verify upstream signatures where available.
- Never accept a digest supplied by the proxy as authoritative.
- Do not fetch the recipe or lock from the same untrusted proxy unless they are independently signed and verified.
- Pin the lock in Git and review changes.

**Residual risk:** A proxy can still deny service, observe metadata, or serve a malicious file that collides with the hash (infeasible for SHA-256/SHA-512 with current practice).

### T2. Compromised recipe/lock source

**Threat:** An attacker modifies `package.yaml` or `package.lock.json` in the package repository, changing the source digest or adding a malicious inline step.

**Impact:** Arbitrary code execution in the build, backdoored output, or a malicious package.

**Controls:**

- Signed commits/tags and protected branches.
- Mandatory review for recipe, lock, patch and source changes.
- `rpmx lock --update` must produce a reviewable diff; digest changes are a review event.
- Inline steps are visible in the plan and counted; profiles may require `argv` for audit reasons.
- Reproducible builds and independent rebuilders catch unexplained output changes.
- Separate signing authority from the ability to merge recipe changes.

**Residual risk:** A malicious maintainer with merge rights can still introduce a malicious recipe. This is a governance problem, not a format problem.

### T3. Malicious upstream at first fetch (TOFU)

**Threat:** The first time a source is pinned, upstream is already compromised, or the packager pins a malicious release without reviewing it.

**Impact:** The lock now "correctly" pins malicious content.

**Controls:**

- Verify upstream signatures and release keys, not just hashes.
- Prefer upstream VCS tags/commits with signed tags for review.
- Compare the source against the previous release; review the diff.
- Use multiple independent sources or a trusted mirror.
- Run tests, static analysis and reproducible builds.
- For Go, use the checksum database and `go.sum`; understand that they prove consistency, not benevolence.
- Require human approval for new source hashes and version updates.

**Residual risk:** Hash pinning is **trust-on-first-use** unless there is an independent signature or reproducibility attestation. A malicious upstream release can be pinned.

### T4. Compromised maintainer or malicious recipe

**Threat:** A maintainer adds an inline build step that exfiltrates data, injects a backdoor, or produces a malicious scriptlet.

**Impact:** Compromised binaries, leaked secrets, or root compromise on install.

**Controls:**

- Build sandbox with no network, no credentials and a minimal filesystem.
- Least-privilege build user; dropped capabilities; `no_new_privs`; seccomp where practical.
- Typed actions instead of arbitrary scriptlets; raw shell actions forbidden by default.
- Review of all hooks and actions; policy allowlists.
- Reproducible builds and independent rebuilds.
- Separation of duties: the person who can merge a recipe cannot unilaterally sign the result.
- Secret scanning of recipe, lock, patches and SRPM.

**Residual risk:** A malicious recipe can still produce a malicious binary. Review, tests and reproducibility are the only defenses; the format cannot prove intent.

### T4b. Malicious source generator or vendoring script

**Threat:** A source-assembly step (`make dist`, `go generate`, an npm lifecycle script, a custom generator) executes attacker-controlled code or produces a malicious source snapshot.

**Impact:** A backdoor enters the source before the SRPM is assembled; the build looks clean.

**Controls:**

- Source assembly runs in its own sandbox, separate from the build.
- No network after fetch; no credentials or signing keys.
- Generator tools and versions are pinned in the lock and recorded in the manifest.
- Generated output is content-addressed; nondeterministic generators fail unless explicitly marked build-time.
- Review generated diffs and generator changes as source changes.
- Treat vendored code as untrusted input; scan it and include its licenses.

**Residual risk:** A malicious generator can produce malicious but deterministic source. Review and provenance are the defenses; the sandbox only limits blast radius.

### T5. Compromised build toolchain

**Threat:** A compromised compiler, linker, build tool or language runtime injects code into the output.

**Impact:** A backdoor that is invisible in the source.

**Controls:**

- Pin the toolchain in the lock and profile; record its provenance.
- Build in a minimal, hermetic root with only declared tools.
- Reproduce with an independent toolchain or builder for high-assurance packages.
- Use SLSA/in-toto provenance and transparency logs.
- Compare binaries with `diffoscope`-style analysis.

**Residual risk:** A compromised compiler can defeat source review. This is the hardest threat in the model and is not solved by SRPMs or proxies.

### T6. Malicious SRPM

**Threat:** An attacker supplies an SRPM whose recipe, lock, sources or manifest do not match.

**Impact:** Arbitrary build commands, malicious binaries, or a false sense of provenance.

**Controls:**

- Verify the SRPM signature before extraction.
- Verify `RPMX-MANIFEST.json` against the recipe, lock, sources and patches.
- Reject SRPMs that do not contain a native recipe and lock; there is no legacy spec rebuild path.
- Verify every source and patch digest before the build.
- Reject unsigned SRPMs by default.
- Sandbox the rebuild with no secrets and no network.

**Residual risk:** A validly signed but malicious SRPM from a compromised signer is indistinguishable from a legitimate one. Signing-key protection and review are the defense.

### T7. SRPM/source extraction attacks

**Threat:** Path traversal (`../../`), absolute paths, symlink escapes, hardlink tricks, device files, decompression bombs, duplicate members.

**Impact:** Writes outside the extraction root, sandbox escape, disk exhaustion.

**Controls:**

- Use a safe extractor with path normalization.
- Reject absolute paths, `..`, duplicate members and links escaping the root.
- Do not create device nodes or setuid files from source archives.
- Enforce size, file-count and decompression limits.
- Extract into a fresh directory with a restrictive umask.
- Validate the SRPM payload before extraction.

**Residual risk:** Parser/extractor bugs remain; fuzz the extractor and keep it updated.

### T8. Install-time scriptlets and actions

**Threat:** Generated `%post`/`%preun`/triggers or typed actions execute as root and can be abused.

**Impact:** Root compromise on the consumer system, persistence, data loss.

**Controls:**

- Typed actions (`systemd`, `sysusers`, `tmpfiles`, `ldconfig`, `alternatives`) instead of arbitrary shell.
- Raw shell actions forbidden by default; allowlisted by profile with review.
- Validate paths referenced by actions; units/sysusers/tmpfiles must be in the package or a dependency.
- Sandbox/namespace scriptlets where the package manager supports it (RPM's `unshare` plugin).
- Test install, upgrade and erase paths in a disposable root.
- Sign packages and enforce signature policy.

**Residual risk:** A typed action can still do something dangerous if its inputs are malicious. Action emitters must be small, audited and quote-safe.

### T9. Signing key compromise

**Threat:** An attacker obtains the SRPM/RPM signing key and signs a malicious artifact.

**Impact:** Consumers and downstreams trust it; the entire verification chain collapses.

**Controls:**

- Keys in an HSM or offline signing service, never in the build sandbox.
- Threshold/multiple signatures for high-value repositories.
- Key rotation, revocation and expiry.
- Transparency log (e.g., Rekor) for signatures and attestations.
- Separation of duties: build, review and sign are different roles.
- Audit logs and anomaly detection for signing operations.
- Verify signatures at the consumer, not just at the proxy.

**Residual risk:** A compromised signing key is catastrophic. No package format can fully mitigate it.

### T10. Go module confusion and sumdb bypass

**Threat:** A malicious or typosquatted module path, a public/private proxy confusion, a `replace` directive to a malicious local path, or `GONOSUMDB` disabling the checksum database.

**Impact:** Malicious dependency code in the binary.

**Controls:**

- Lock exact module paths, versions and `h1:` sums.
- Enforce `GOSUMDB` for public modules; require an explicit policy for private modules.
- Allowlist `GOPROXY` hosts; do not let recipes set arbitrary proxies.
- Review `replace` and `exclude` directives; require local replacements to be in the SRPM.
- Vendor dependencies and review `vendor/modules.txt`.
- Audit licenses and module provenance.
- Treat `go generate` as build code: sandbox it, declare its tools, no network.

**Residual risk:** Private modules without a public sumdb rely on organizational trust; the lock can pin a malicious private module.

### T11. Dependency and toolchain substitution

**Threat:** A build dependency or tool is fetched from an untrusted source, or a dependency name resolves differently than intended.

**Impact:** Backdoored build tools, unexpected libraries, or a build that differs from the plan.

**Controls:**

- Build dependencies are resolved and recorded in the lock.
- Build root digest is part of the reproducibility contract.
- Verify package signatures for build-root packages.
- Prefer capability names plus explicit providers; avoid ambiguous virtual dependencies.
- No network in the build sandbox.
- `rpmx explain` shows the provenance of every dependency.

**Residual risk:** A compromised repository or build-root package can still be trusted if its signature is valid.

### T12. Cache poisoning and TOCTOU

**Threat:** An attacker with local access modifies a file in the CAS between verification and use, or replaces a cached file via a symlink.

**Impact:** A verified file becomes malicious after verification.

**Controls:**

- CAS entries are read-only and owned by the build user.
- Verify on insert and re-verify (or use an open file descriptor) on use.
- Use `O_NOFOLLOW`, restrictive directory permissions, and a per-user cache.
- Never run the build as root.
- Copy verified bytes into the sandbox; do not mount a writable shared cache.

**Residual risk:** Local root compromise defeats the cache. The threat model assumes the build host is not already root-compromised.

### T13. Build sandbox escape

**Threat:** A malicious build script exploits the kernel, container runtime or a mounted device to escape the sandbox.

**Impact:** Host compromise, theft of signing keys or credentials, tampering with other builds.

**Controls:**

- Rootless sandbox, user namespaces, minimal mounts, no host `/proc`/`/sys` beyond what is needed.
- Drop capabilities; `no_new_privs`; seccomp.
- No Docker socket, no host network, no credentials.
- Separate build hosts per trust domain; ephemeral runners.
- Keep the kernel/container runtime patched.

**Residual risk:** Kernel and runtime exploits are always possible. Defense in depth and host isolation matter.

### T14. Secret leakage

**Threat:** Credentials in source URLs, proxy auth, recipe files or build logs are exposed.

**Impact:** Repository compromise, private source disclosure, supply-chain access.

**Controls:**

- Never put credentials in `package.yaml` or the lock.
- Proxy credentials come from the fetch environment, not the recipe.
- Redact logs; do not print authorization headers.
- Scan the SRPM for secrets before signing.
- Private sources get access-controlled SRPMs.

**Residual risk:** Proxy operators see metadata (package names, module paths, source URLs) even with TLS.

### T15. Availability and retention attacks

**Threat:** The proxy is unavailable, evicts objects, or an upstream disappears; the SRPM is deleted.

**Impact:** Builds fail; historical rebuilds become impossible.

**Controls:**

- Multiple locators and mirrors.
- Local CAS and SRPM archives.
- Immutable SRPM storage with retention/backup.
- Repository replication.
- `rpmx build --offline` from SRPM.

**Residual risk:** A supply-chain outage can still delay new builds; existing SRPMs protect rebuilds.

### T16. Reproducibility evasion

**Threat:** A malicious build behaves differently depending on the builder, or introduces nondeterminism that hides differences.

**Impact:** A backdoor that only appears in some builds; comparison fails to catch it.

**Controls:**

- Normalize timestamps, paths, ordering, locale and environment.
- Build twice in different roots; compare unsigned payloads.
- Independent rebuilders with different toolchains/hosts.
- Publish reproducibility attestations.
- Investigate every unexplained difference.

**Residual risk:** A compiler backdoor or a build that detects the builder can still evade. Diverse rebuilds are the mitigation.

### T17. SBOM/provenance forgery

**Threat:** An attacker modifies the SBOM or provenance sidecar, or generates one that omits a malicious component.

**Impact:** False assurance; vulnerable or malicious components go unnoticed.

**Controls:**

- Generate SBOM from the actual payload, not just the recipe.
- Sign SBOM/provenance and bind them to the SRPM/RPM digest.
- Verify the binding at consumption time.
- Use transparency logs and multiple attestations for high-assurance packages.
- Scan the payload independently (Syft/Grype/Trivy, etc.).

**Residual risk:** A compromised builder can generate a false but validly signed attestation. Signing-key and builder trust are the anchors.

### T18. Consumer skips verification

**Threat:** A consumer installs an RPM/SRPM without checking signatures, or trusts a proxy/repository blindly.

**Impact:** All upstream controls are bypassed.

**Controls:**

- Enforce signature policy by default (RPM 6 does this).
- Signed repository metadata and TLS.
- Document the verification commands.
- Make `--nogpgcheck` loud and auditable.
- Publish key fingerprints and rotation information.

**Residual risk:** User misconfiguration or a compromised package manager.

## 5. What this design improves

Compared with a traditional lookaside cache:

- **Integrity:** the proxy is untrusted; content is verified by digest.
- **Availability:** the SRPM is self-contained; rebuilds do not need the proxy.
- **Auditability:** the lock records source identity, signatures and the locator used.
- **Consistency:** local and downstream builds use the same SRPM path.
- **Reproducibility:** the SRPM is a fixed, archived input; rebuilds can be compared.
- **Supply-chain evidence:** SBOM/provenance can bind to the SRPM digest.

## 6. What it does not solve

- **Trust-on-first-use.** A malicious upstream release can be pinned.
- **Malicious maintainers.** A valid recipe can build a malicious package.
- **Compromised compilers/tools.** A backdoor can enter below the source level.
- **Compromised signing keys.** A valid signature is not a safety proof.
- **Root scriptlets.** Install-time code is still dangerous.
- **Metadata confidentiality.** The proxy sees what you fetch.
- **Consumer negligence.** Verification must happen at the edge.

## 7. Required controls

These should be non-negotiable in the design:

1. **The lock is trusted independently of the proxy.** Fetch recipe/lock from a signed Git repo or a signed SRPM; never let the content proxy define the expected hash.
2. **The proxy is untrusted.** Verify digest and signature after every fetch; verify again before use.
3. **The SRPM is untrusted until verified.** Require a signature and a valid manifest for native rebuilds; reject unsigned by default.
4. **No network and no secrets in the build sandbox.**
5. **Safe extraction.** Reject traversal, absolute paths, escaping symlinks, duplicates and device files; enforce size limits.
6. **Typed actions by default; raw shell actions forbidden unless explicitly allowed by policy.**
7. **Signing keys never enter the build environment.** Sign after the build, with a policy check and audit log.
8. **SBOM/provenance are signed and bound to the artifact digest.**
9. **Reproducible builds for high-assurance packages; independent rebuilds where possible.**
10. **Signature verification at the consumer is mandatory.**
11. **Source assembly is separate, networkless, pinned and deterministic.** Generated sources are part of the SRPM and reviewed as source changes.

## 8. Residual risk summary

| Risk | Severity | Residual after controls |
|---|---|---|
| Malicious upstream at first pin | High | Still possible; signatures/review/reproducibility reduce it |
| Malicious maintainer | High | Governance and separation of duties only |
| Compromised compiler/toolchain | Critical | Independent rebuilds and provenance reduce it |
| Signing key compromise | Critical | HSM/threshold/transparency reduce it |
| Root scriptlet abuse | High | Typed actions and policy reduce it; not eliminated |
| Proxy DoS/metadata exposure | Medium | Mirroring and TLS; metadata exposure remains |
| SRPM extractor bug | Medium | Safe extraction, limits, fuzzing |
| Cache TOCTOU | Medium | Immutable cache and re-verification |
| Consumer skips verification | High | Enforce defaults and documentation |

## 9. Conclusion

The proxy-plus-SRPM model is **security-sound for integrity and availability** if, and only if:

- the expected hash comes from a trusted lock, not from the proxy;
- the proxy is treated as an untrusted transport;
- the SRPM is signed and verified before use;
- the build is hermetic and secret-free;
- signing keys are protected outside the build;
- install-time actions are constrained and reviewed.

It does not solve trust in upstream projects, maintainers, compilers or signing keys. Those are the real trust anchors. The proxy and SRPM are plumbing; they must not be mistaken for trust.
