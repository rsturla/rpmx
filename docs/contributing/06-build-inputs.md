# Build inputs: replacing the lookaside cache

> **SRPM-first update.** SRPMs are mandatory. The "source bundle" idea in this document is superseded by the native SRPM described in [`07-srpm-first.md`](07-srpm-first.md). The content-addressing and locator model still applies to how the SRPM is populated; the SRPM itself is the self-contained source artifact.

## 1. What the lookaside cache actually is

Fedora-style dist-git keeps the spec, patches and a `sources` file in Git, but not the source tarballs. The `sources` file records a hash per file, and `fedpkg new-sources` uploads the tarball to a separate **lookaside cache**. `fedpkg sources` downloads it again.

That design solves a real problem: dist-git stays small, and source tarballs are not duplicated in every clone. The hash manifest is also a good idea. The problem is the *storage and transport model*:

- the source content is not in the repository;
- rebuilds depend on a distro-specific service and its availability;
- the service has its own auth, URLs, retention and migration story;
- the `sources` file names files but does not describe portable locations;
- the cache is not content-addressed in a standard way;
- downstreams and CI must know about the distro's lookaside;
- source review often sees a hash, not the bytes;
- the cache is a single logical point of failure for historical builds.

The hash manifest is worth keeping. The privileged blob service is not.

## 2. Requirements for a modern build-input model

1. **Content identity is primary.** A source is identified by a cryptographic digest, not by a filename or a cache key.
2. **Location is separate from identity.** A source can be fetched from upstream, a mirror, an OCI registry, a local file, a CAS, or an SRPM. Any locator that yields the right digest is acceptable.
3. **No mandatory central service.** A distro may run a mirror or CAS, but the model must work without a bespoke lookaside API.
4. **Offline and reproducible.** `rpmx fetch --locked` populates a local cache; `rpmx build --offline` works from it. The build never needs the network.
5. **Pin and verify signatures where available.** Digest is the trust anchor; upstream signatures add publisher identity.
6. **Self-contained source artifact.** An SRPM must carry the recipe, lock, patches and sources for archival and downstream rebuilds.
7. **Large-file friendly.** Do not put 500 MB tarballs in Git. Use LFS, OCI, CAS, or SRPMs.
8. **Mirrorable and federated.** Standard HTTP/OCI/CAS protocols, not a custom cache.
9. **Auditable.** The lock records digest, size, signature result and the locator used.
10. **Migration-friendly.** Import the existing lookaside and `sources` file, but do not require them for new packages.

## 3. The options

| Option | Content-addressed | Self-contained | External service | Large files | Tooling maturity | Notes |
|---|---|---|---|---|---|---|
| **Status quo lookaside** | Hash manifest, custom store | No | Yes, bespoke | Yes | Fedora-specific | Works, but vendor lock-in and availability risk. |
| **Sources in Git** | Git blob IDs | Yes | No | No | Excellent | Fine for small files; repo bloat and clone time for tarballs. |
| **Git LFS** | LFS OID (SHA-256) | Pointers in Git, blobs elsewhere | Yes, LFS server | Medium | Good | Standard, familiar; LFS server can be a single point; bandwidth costs. |
| **git-annex** | Keyed by hash | Pointers in Git, content in remotes | Optional | Yes | Niche but mature | Very flexible special remotes (S3, rsync, web, directory); more complex UX. |
| **OCI artifact registry (ORAS)** | OCI digest | No, but portable | Yes, standard registry | Yes | Good and improving | Content-addressed, signed, replicated, auth; registry GC/retention is the risk. |
| **Generic CAS (Nix/Bazel-style)** | Yes | No, but cacheable | Optional | Yes | Good in build systems | Strong model; needs a protocol/service and GC; Nix store is the reference. |
| **Upstream URL + hash + mirrors** | Verified by hash | No | No (uses existing HTTP) | Yes | Excellent | Simplest; URL rot is handled by mirrors/SRPMs. This is what Nix/Cargo/Go do. |
| **SRPM (mandatory)** | SRPM digest | Yes | No | Yes | Mature | Portable archival artifact; contains recipe, lock, sources, patches and manifest; no spec. |
| **Debian pool model** | `.dsc` hashes | Source package | Distro archive/mirrors | Yes | Mature | Central archive, but standard mirroring; no per-package bespoke cache. |
| **IPFS** | Yes | No, pinning needed | P2P | Yes | Niche | Availability and enterprise concerns; not a primary store. |
| **Language proxies** (Go proxy, Cargo, npm, PyPI) | Content-addressed-ish | No | Yes | Yes | Mature | Great inside a language ecosystem; not a general source store. |

## 4. Analysis

### Lookaside

The good part is the hash manifest. The bad part is that it is a **private protocol for a public problem**. A source tarball is just a blob; there is no reason it needs a distro-specific upload command and cache URL in 2026.

Keep the hash. Replace the store with locators and a local cache.

### Sources in Git

Git is already content-addressed. For small patches, unit files and tiny sources, putting them in Git is ideal: a clone is self-contained, review sees the bytes, and there is no extra service.

For large tarballs it is the wrong tool: clone size, packfile bloat, hosting limits, and slow CI. Use Git for packaging text, not for upstream release archives.

### Git LFS

Git LFS is the natural "sources in Git, but not really" option:

- the repository contains small pointer files with an OID and size;
- the actual blob lives in an LFS server;
- clones are small until `git lfs pull`;
- the OID is content-addressed.

It is much more standard than a distro lookaside, and it fits the existing Git review workflow. The drawbacks are operational: an LFS server is still a separate service, bandwidth can be expensive, and GitHub/GitLab LFS quotas are real. It is a good **vendoring mode**, not the only model.

### git-annex

git-annex is the power-user version of the same idea:

- files are symlinks/pointers to content-addressed keys;
- content lives in "special remotes" (S3, rsync, web, directory, etc.);
- `git annex get` fetches what you need;
- integrity is checked against the key.

It is flexible and decentralized, but the UX is unfamiliar to most packagers and the tooling is niche. Good as an optional backend, not the default.

### OCI artifact registry

An OCI registry is a content-addressed, authenticated, replicated blob store with a standard API. ORAS lets you store arbitrary artifacts (not just container images) with a digest. A source tarball can be an OCI artifact, and a native SRPM can be stored as an OCI artifact.

This is the strongest **lookaside replacement**:

- `oras push` / `oras pull` or an OCI client;
- digest is the identity;
- registries already exist in most organizations;
- signing/attestation (cosign, Sigstore) is mature;
- mirrors and replication are standard registry features.

Costs: registry auth and retention policies, garbage collection can delete old artifacts, and a registry is still a service. But it is a *standard* service, not a bespoke lookaside.

### Generic CAS

Nix's store and Bazel's remote cache are the reference models:

- the source is a fixed-output derivation: the expected hash is declared before fetching;
- the content is stored by hash;
- a binary cache can serve it;
- a build only needs the hash, not the URL.

This is conceptually the cleanest. The downside is that a general CAS protocol/service is another piece of infrastructure. `rpmx` can implement a simple local CAS and optionally speak to a remote CAS; it does not need to invent a new distributed store.

### Upstream URL + hash + mirrors

This is the simplest and most common model outside RPM:

- the recipe declares the upstream URL and hash;
- the lock records the hash and optionally a set of mirrors;
- the fetcher tries the locators and verifies the digest;
- a local cache keeps what it fetched.

Nix's `fetchurl` is a fixed-output derivation; Cargo, Go, npm and PyPI all fetch by URL plus a checksum. The weakness is URL rot, but mirrors, OCI, LFS or SRPMs solve that without changing the identity.

This should be the **default** for `rpmx`. It requires no new service and makes the source identity explicit.

### SRPM

The SRPM is the mandatory, self-contained source artifact. It carries the sources, the recipe, the lock, the patches and the build manifest; native SRPMs contain no spec. A native `rpmx` SRPM is defined in [`07-srpm-first.md`](07-srpm-first.md).

It solves:

- offline builds;
- archival reproducibility;
- downstream rebuild without a lookaside;
- source review of the exact bytes.

The cost is duplication: the same tarball exists in the upstream, the local cache and the SRPM. Content-addressed storage can deduplicate at rest. SRPMs are produced for every build because they are the canonical build input, not only for releases.

### Debian pool

Debian's pool is not a per-package lookaside. Source packages live in the archive and are mirrored; the `.dsc` references files with hashes. It is centralized, but it uses standard archive mirroring and is self-describing.

The lesson is not "copy the pool"; it is "a source archive can be a normal mirrored artifact store". Fedora's lookaside is a special case because dist-git wanted to stay small.

### IPFS and language proxies

IPFS is content-addressed and decentralized, but availability depends on pinning and the enterprise story is weak. Language proxies (Go module proxy, Cargo registry, npm, PyPI) are excellent inside their ecosystems but are not a general source store.

They are good **additional locators**, not the foundation.

## 5. Recommended model for `rpmx`

### 5.1 A source is a digest plus locators

```yaml
sources:
  - name: curl
    digest: "sha512:8a8b..."
    size: 1234567
    signature:
      type: openpgp
      url: https://curl.se/download/curl-8.11.1.tar.xz.asc
      fingerprint: "27EDEAF22F3ABCE1"
    locators:
      - kind: https
        url: https://curl.se/download/curl-8.11.1.tar.xz
      - kind: https
        url: https://mirror.example.org/curl/curl-8.11.1.tar.xz
      - kind: oci
        ref: ghcr.io/example/sources/curl:8.11.1
      - kind: file
        path: vendor/curl-8.11.1.tar.xz
```

- `digest` is the identity. It is required.
- `size` is optional but recommended; it lets the fetcher reject absurd responses early.
- `signature` is optional; a profile can require it.
- `locators` is an ordered list. The fetcher tries them until one matches the digest.
- `kind` is extensible: `https`, `file`, `git`, `oci`, `cas`, `lfs`, `lookaside` (for migration).
- A locator is untrusted; the digest is the trust anchor.

A git source is pinned by a full commit and packed into a deterministic
archive before it is stored in the CAS:

```yaml
sources:
  - name: mytool
    kind: git
    digest: "sha512:..."
    locators:
      - kind: git
        url: https://github.com/example/mytool.git
        commit: 0123456789abcdef0123456789abcdef01234567
```

The archive is built with sorted entries, the commit timestamp as the entry
mtime, and a `mytool/` prefix. The timestamp is what lets an offline rebuild
derive `source-date-epoch` from the cached archive.
On the first fetch a placeholder digest fails with the generated digest in the
error, so the recipe can be pinned without a separate tool.

The lock records the resolved digest, size, signature result and the locator that succeeded:

```json
{
  "name": "curl",
  "digest": "sha512:8a8b...",
  "size": 1234567,
  "signature": {
    "type": "openpgp",
    "fingerprint": "27EDEAF22F3ABCE1",
    "verified": true
  },
  "locator": {
    "kind": "https",
    "url": "https://curl.se/download/curl-8.11.1.tar.xz"
  }
}
```

The locator in the lock is for audit, not for rebuild. A rebuild may use any locator that yields the same digest.

### 5.2 Local content-addressed cache

```
~/.cache/rpmx/cas/sha256/8a/8b... 
```

- `rpmx fetch --locked` populates it.
- `rpmx build --offline` uses it and fails with a clear digest if missing.
- `rpmx cache verify` re-hashes entries.
- `rpmx cache gc --keep-locked` removes entries not referenced by the current lock/repository.
- Hardlink/reflink into the build source directory; never mutate the cache.

The cache is an optimization, not the source of truth. It can be deleted and rebuilt from locators or an SRPM.

### 5.3 SRPM assembly

```
rpmx srpm --output curl-8.11.1-1.src.rpm
rpmx srpm --oci ghcr.io/example/curl-src:8.11.1
```

The SRPM contains the sources, recipe, lock, patches and build manifest; no spec. It is deterministic and signable. A build can use it as the only input:

```
rpmx build --from-srpm curl-8.11.1-1.src.rpm
rpmx build --from-srpm ghcr.io/example/curl-src:8.11.1
```

This is the replacement for "download from the lookaside". It is portable, archival and offline-friendly. It can be stored in an OCI registry, a release asset, or a plain HTTP object.

### 5.4 Vendoring modes

A distro or project can choose a policy:

| Mode | What it does | Good for |
|---|---|---|
| `none` | Locators + local cache; no vendoring | Normal packages, upstream sources available. |
| `git-lfs` | Stores sources as LFS objects and pointers in the package repo | Self-contained dist-git with moderate sizes. |
| `oci` | Pushes sources as OCI artifacts; lock records digest/ref | Organizations with a registry and supply-chain tooling. |
| `srpm` | Stores/publishes native SRPMs | Archival, downstream rebuilds, air-gapped builds. |
| `lookaside` | Reads an existing Fedora-style lookaside | Migration only. |

`rpmx vendor --mode ...` performs the upload/creation and updates the lock.

### 5.5 Fetch policy

- **Default:** fetch from locators, verify digest and optional signature, store in CAS.
- **`--offline`:** never network; use CAS or an SRPM; fail with the missing digest.
- **`--locked`:** refuse if the recipe or lock would change; fetch only what the lock names.
- **`--from-srpm`:** use the SRPM as the only input; useful for air-gapped builds.
- **`--prefer` / `--no-network`:** explicit policy for CI.

No build script gets network access. Fetching is a separate, auditable phase.

### 5.6 Go modules without a hand-made tarball

Go has a built-in source distribution protocol that fits the locator model: the **module proxy**.

A recipe can name a module directly:

```yaml
sources:
  - name: mytool
    kind: go-module
    module: example.com/mytool
    version: v1.2.3
    sum: "h1:..."
```

`rpmx` resolves this to a proxy URL such as:

```
https://proxy.golang.org/example.com/mytool/@v/v1.2.3.zip
```

and verifies the `h1:` checksum from the `go.sum`/module sum database. The packager does not create or upload a tarball.

For dependencies, the fetch stage reads `go.mod` and `go.sum`, fetches every required module through the `go-module` locator's proxy, and verifies it against the sum database:

```yaml
sources:
  - name: hello
    kind: go-module
    locators:
      - kind: go-module
        proxy: https://proxy.golang.org
        sumdb: sum.golang.org
```

The resolved module cache feeds source assembly; the assembly steps never need
the proxy URL. The fetch stage runs roughly:

```
go mod download
go mod verify
go mod vendor
```

The resulting `vendor/` tree is part of the source snapshot that goes into the SRPM. The build phase uses `-mod=vendor` with no network. `vendor/modules.txt` and `go.sum` remain in the snapshot, so the module graph and hashes are auditable.

**This is not "no source in the SRPM".** It is "no human-authored tarball". The SRPM still needs the bytes to rebuild offline. The choice is how those bytes are represented:

| Mode | SRPM contains | Build uses | Trade-off |
|---|---|---|---|
| `vendor` (default) | `vendor/` tree inside the source snapshot | `-mod=vendor` | Idiomatic, easy to audit; larger uncompressed source. |
| `module-cache` | Go module cache (`cache/download/...`) | `GOPROXY=file://...` | Preserves module zips and sums; more complex. |
| `proxy` | Only `go.mod`/`go.sum` | Network at build time | **Not allowed** under the SRPM-first contract. |

Private modules work through `GOPROXY`/`GOPRIVATE`/`GONOSUMDB`; credentials belong in the fetch phase, never in the SRPM. `go.sum` is still the integrity check, and the lock records the module path, version and sum.

Limitations worth planning for:

- `replace` directives to local paths need the local directories included in the snapshot.
- `go.work` multi-module workspaces need an explicit source-assembly policy; `go work vendor` is the current mechanism.
- module zips exclude nested modules and VCS metadata by design; that is usually what you want for a build.
- generated code (`go generate`) may need tools declared as build dependencies.
- vendored license files should be audited for compliance; `go-licenses` or an SBOM tool can enumerate them.

The important property is that the proxy is just another locator. You can use the public proxy, a private Athens/JFrog/GitLab proxy, a Git source, or a local vendor directory. The SRPM still contains the resolved, verifiable source snapshot.

### 5.7 MVP: proxy-first, lookaside as one backend

If the goal is to ship something soon, the minimum viable architecture is:

```
rpmx fetch  --->  acquisition proxy  --->  upstream / lookaside / mirror
                       |
                  content cache
                       |
                  rpmx local CAS
                       |
                  SRPM assembly
                       |
                  offline build
```

The proxy is the **only** network egress. The build sandbox still has no network. The fetch phase may talk to the proxy; the build phase never does.

The proxy interface is content-addressed and small. The MVP is plain HTTPS with no custom service protocol:

```
GET  /v1/blobs/sha512/<128-hex>        # required
HEAD /v1/blobs/sha512/<128-hex>        # required
PUT  /v1/blobs/sha512/<128-hex>        # optional, authenticated upload
```

Contract:

| Item | Rule |
|---|---|
| Identity | The path digest is the source `digest`. SHA-512 only. A proxy may cache by any key, but the request path is the trusted digest. |
| Success | `200` with the exact bytes; `Content-Length` must match the blob and `Content-Type: application/octet-stream`. |
| Absent | `404` with a JSON body `{"code":"NOT_FOUND","digest":"sha512:..."}`. |
| Digest mismatch | The client rejects the response and reports `RPMX-FETCH-004`; it never retries the same locator blindly. |
| Range | `Range: bytes=...` returns `206` or `200`; the client verifies the assembled digest, not the part. |
| Redirects | Allowed, but the client must not forward credentials across origins and must re-verify the digest. |
| Auth | Bearer token or mTLS at the transport; credentials come from the fetch environment, never the recipe or SRPM. |
| Limits | Bounded response size, connect timeout, read timeout and total deadline. A proxy cannot cause unbounded memory or disk use. |
| Errors | JSON `{"code","message","digest"?}` with a documented code set; no stack traces or internal paths. |
| Offline | The client works from the local CAS with no proxy. `--offline` makes a proxy request an error. |

A lookaside adapter is then just a backend that resolves a filename/hash pair to bytes:

```
lookaside: name=curl-8.11.1.tar.xz, hash=SHA512:...
        -> GET /v1/blobs/sha512/<128-hex>
```

The proxy may fetch from upstream on a cache miss, or from the existing lookaside, or from an OCI registry, or from a mirror. `rpmx` does not need to know which. It sends the digest it expects and verifies the bytes it receives.

This gives a useful MVP without painting us into a corner:

- **Now:** one proxy, one content-addressed endpoint, a lookaside adapter, and a local CAS.
- **Later:** add `oci`, `go-module`, `lfs`, `cas` and `git` adapters behind the same endpoint.
- **Always:** the recipe/lock identify the content; the proxy only transports it.

Rules for the MVP:

1. **Fetch before build, not during build.** "Build-time pull" means the fetch phase of the build orchestrator, not network access inside `%build`.
2. **The proxy is untrusted.** Verify the digest and optional signature after every fetch.
3. **The lock is trusted independently.** Do not let the proxy supply the expected hash.
4. **The SRPM contains the bytes.** Once assembled, the proxy is not needed for rebuilds.
5. **No credentials in the recipe.** Proxy auth belongs to the fetch environment.
6. **The proxy interface is stable and small.** A content-addressed GET is easier to replace than a custom lookaside API.
7. **Cache misses are explicit.** If the digest is unavailable, fail with the digest and the locators tried; do not silently substitute a different file.

This is essentially "lookaside, but with a standard content-addressed interface and a replaceable backend". It is a good compromise between shipping now and not baking the legacy cache into the model.

### 5.8 Digest policy: SHA-512 by default, algorithm-agile

The canonical digest is **SHA-512**, written with an explicit algorithm prefix:

```yaml
sources:
  - name: curl
    digest: "sha512:8a8b..."
```

Rules:

- **SHA-512 is the default and the minimum** for source identity, lock entries and the SRPM manifest.
- **SHA-256 is allowed only as a transport digest** where a protocol requires it: OCI registry digests, Go module `h1:` sums, Git object IDs. It is recorded separately, not used as the sole trust anchor.
- **MD5 and SHA-1 are rejected.** There is no compatibility mode that accepts them for source identity.
- **Algorithm-prefixed values are mandatory.** A bare 64- or 128-character hex string is a lint error. `sha512sum` is a file format, not a digest type.
- **Multiple digests are allowed during migration.** A source may carry `digest: "sha512:..."` plus `transport_digests: { sha256: "..." }`; all declared digests that are present must match.
- **The strongest allowed digest is authoritative.** A weaker transport digest cannot satisfy a policy that requires SHA-512.

What is hashed:

| Input | Digest target |
|---|---|
| Archive/file | Exact bytes as fetched (compressed archive). |
| Git source | A deterministic archive of the checkout, excluding `.git`; commit recorded separately. |
| Go module | SHA-512 of the module zip bytes, plus the Go `h1:` sum as transport metadata. |
| OCI artifact | SHA-512 of the downloaded layer/artifact; registry SHA-256 digest recorded separately. |
| SRPM | SHA-512 of the signed SRPM for distribution; SHA-512 of the unsigned payload for reproducibility. |
| File manifest | SHA-512 of each packaged file; RPM's internal file digests remain an RPM concern. |

Why SHA-512 rather than something "faster":

- It is FIPS-approved and widely available.
- It has a large security margin (256-bit collision resistance).
- It is already used by RPM's SHA-512 payload digest and by many `sources` files.
- Hashing sources is not the build bottleneck; compatibility and assurance matter more.

**SHA-256** is still a perfectly good hash and is faster with SHA-NI on many CPUs. It remains the transport digest for OCI and Go because those ecosystems standardized on it. We do not pretend a transport digest is the policy digest.

**BLAKE3** is a reasonable optional *accelerator* for local integrity checks, but it is not FIPS-approved and is not a good universal trust anchor for a distro. Do not make it the default. If it is supported later, it gets its own `blake3:` prefix and an explicit profile enablement; it never silently replaces SHA-512.

**SHA-3-512** is a defensible future alternative if a profile requires it, but the ecosystem support is weaker. The algorithm-prefixed format makes that a policy change, not a schema change.

### 5.9 OCI registry as an optional lookaside backend

An OCI registry is a good fit for a **centralized source store**:

- content-addressed by digest;
- standard auth and scoped repositories;
- replication and mirroring;
- signatures and attestations (cosign/notation, OCI referrers);
- works with existing artifact managers (Harbor, Artifactory, Nexus, GHCR, Quay, ECR).

It should be a backend behind the acquisition interface, not a required part of the schema.

#### Suggested layout

```
registry.example.org/rpmx/sources/curl@sha256:...     # source artifact
registry.example.org/rpmx/srpms/curl@sha256:...       # native SRPM artifact
```

A source artifact can be an ORAS/OCI artifact whose layer is the source file:

```
artifactType: application/vnd.rpmx.source.v1
layers:
  - mediaType: application/x-xz
    digest: sha256:...
    size: ...
annotations:
  org.rpmx.source.name: curl-8.11.1.tar.xz
  org.rpmx.source.sha512: ...
```

An SRPM artifact can be a single blob with a vendor media type, or a manifest whose layer is the `.src.rpm`.

#### Rules

1. **Digest, not tag, is authoritative.** Tags such as `curl-8.11.1` are discovery aids. The lock records the manifest digest.
2. **The registry digest is a transport digest.** Most registries use SHA-256. `rpmx` computes and records the policy SHA-512 over the bytes.
3. **The registry is untrusted.** Verify SHA-512, signatures and the SRPM manifest; a registry compromise must not be enough to change the build.
4. **Pull-only credentials for builds.** Push is restricted to the publishing pipeline; no registry credentials in the build sandbox or SRPM.
5. **Immutability and retention.** Disable tag mutation, protect manifests from GC, and replicate. A registry that can delete source blobs is a cache, not an archive.
6. **The SRPM remains the durable source.** If the registry is lost or reorganized, rebuilds still work from SRPMs.
7. **Test artifact support.** Some registries reject unknown media types or lack the OCI 1.1 referrers API. Use ORAS-compatible media types and a compatibility test in CI.
8. **No credentials in locators.** Use registry auth from the fetch environment; locators contain only the fully qualified repository and digest.

#### What OCI solves

- A standard, content-addressed lookaside replacement.
- Centralized caching and replication.
- Signatures and SBOM/attestation storage via referrers.
- Familiar operations for organizations already running registries.

#### What OCI does not solve

- Registry GC and retention are still operational risks.
- Large single layers are not file-level deduplicated; a general CAS is better for chunk-level dedupe.
- Artifact support varies by registry.
- Tags are mutable unless policy forbids it.
- A registry is still a service; the SRPM is still the offline fallback.

**Recommendation:** support OCI as an optional `kind: oci` locator/backend. If a distro wants one centralized lookaside replacement, this is the best standard choice. Keep HTTPS + local CAS + SRPM as the default path so OCI is not mandatory.

### 5.10 Source assembly is a pipeline stage

Source generation belongs in the pipeline, but not inside `%build`. It is a distinct **source assembly** stage between fetch and SRPM assembly:

```
resolve recipe + lock
        |
        v
fetch sources (proxy, CAS)
        |
        v
source assembly
  - unpack/verify
  - git -> deterministic archive
  - go mod vendor / cargo vendor
  - deterministic code generation
  - license collection
  - deterministic source snapshot
        |
        v
SRPM assembly
        |
        v
offline build from SRPM
```

Commands mirror the stages:

```
rpmx fetch      # populate CAS
rpmx source     # produce the exact source snapshot
rpmx srpm       # package that snapshot into an SRPM
rpmx build      # source -> SRPM -> binaries
rpmx rebuild    # SRPM -> binaries
```

What belongs in source assembly:

- converting a Git checkout into a deterministic archive;
- fetching and vendoring Go modules (`go mod vendor`);
- vendoring Cargo/npm/etc. dependencies;
- deterministic source transformations that are part of the snapshot;
- code generation when the output is deterministic and belongs in the source artifact;
- collecting license files and source metadata.

What belongs in the build:

- compilation, linking and packaging;
- target-specific code generation that depends on the target toolchain;
- tests;
- anything that needs the target sysroot.

Generated code is the hard case:

| Generation | Where | Why |
|---|---|---|
| `go mod vendor` | Source assembly | The vendored tree is part of the source snapshot. |
| `make dist` | Source assembly | It produces the source artifact; tools pinned. |
| Protobuf/generated parsers | Either | Source if deterministic and target-independent; build if target-specific. |
| `go generate` | Build by default | It can execute arbitrary code and may need target tools; sandbox it. |
| `npm ci` lifecycle scripts | Source assembly or build, sandboxed | They execute package code; no network, no secrets. |

The rule: **if the generated output is part of what downstream needs to rebuild, put it in the source snapshot and record the generator/tool versions. If it is target-specific, generate it in the build with pinned tools and no network.**

Rules for source assembly:

1. **No network after fetch.** All generator dependencies are fetched first.
2. **Separate sandbox from the build.** Source assembly can run generator code; the build gets only the resulting snapshot.
3. **Pin generator tools** in the lock; record versions and commands in the manifest.
4. **Deterministic output.** Sorted file lists, normalized mtimes, no host paths, `SOURCE_DATE_EPOCH`, stable archive order.
5. **Content-address the result.** The source snapshot digest goes into the lock and SRPM manifest.
6. **Review generated diffs.** A generator change is a source change; do not silently regenerate.
7. **No secrets.** Source assembly has no credentials or signing keys.
8. **Cache by inputs.** Upstream digests + generator versions + recipe/lock -> source snapshot digest.

If a generator cannot be made deterministic or must run on the target, do it in the build phase — but the SRPM must still contain all inputs and pinned tools, and the build must be offline. Record that choice in the manifest.

### 5.11 Bumping Go modules for CVE remediation

A dependency bump is a **source change**, so it belongs in source assembly, before the SRPM — not in `go/build` and not in `%build`.

Melange has a good model here: its `go/bump` step takes `deps: module@version` and runs before `go/build`. `rpmx` should have the same idea, but earlier in the pipeline:

```yaml
source-assembly:
  pipeline:
    - uses: go/bump
      with:
        deps:
          - module: golang.org/x/net
            version: v0.33.0
            reason: CVE-2024-45337
            advisory: https://pkg.go.dev/vuln/GO-2024-3333
          - module: github.com/sirupsen/logrus
            version: v1.9.3
            reason: CVE-2023-45142
    - uses: go/vendor
```

The `go/bump` step renders `deps.yaml` and runs omnibump in the
source-assembly sandbox:

```
omnibump --deps deps.yaml --tidy
go mod verify
go mod vendor
```

Then it records:

- `go.mod`/`go.sum` before and after;
- the direct and indirect module versions;
- the selected versions from `go list -m all`;
- the advisory IDs and reason;
- the new source snapshot digest;
- the new `vendor/modules.txt` digest.

The SRPM contains the bumped snapshot. The build stays offline and uses `-mod=vendor`.

Rules:

1. **Bump before SRPM.** A bump in the build would make the SRPM incomplete and the build nondeterministic.
2. **Fetch new module versions before source assembly.** The fetch phase must populate the module cache/proxy for the bumped versions; source assembly then runs with `GOPROXY=off` or a local file proxy.
3. **Pin the Go toolchain.** Use `GOTOOLCHAIN=local` and record the Go version; do not let a build download a different toolchain.
4. **Review the diff.** `go get` can upgrade transitive dependencies through MVS. Show the full module graph diff, not just the requested module.
5. **Run tests and vulnerability scanning.** `go test ./...` plus `govulncheck ./...` (with a prepared vulnerability DB) should confirm the advisory is resolved.
6. **Record the advisory.** The manifest/SBOM should link the CVE/GHSA/OSV ID and the fixed version.
7. **Bump the package release.** A dependency-only change still produces a new binary, so increment `release` (or let `release.auto` resolve it) and generate a changelog entry.
8. **Handle no-fix cases explicitly.** If no upstream release fixes the issue, use a reviewed `replace` to a fork or a pseudo-version; record why. Do not silently drop the dependency.
9. **Workspaces.** For `go.work`, use `go work vendor` and record the workspace modules.

This is the same affordance as melange’s `go/bump`, but placed where the SRPM-first model requires it: the bump is part of the source artifact, not hidden inside the build.

#### 5.11.1 Use omnibump as the bump engine

Chainguard’s [`omnibump`](https://github.com/chainguard-dev/omnibump) is a better fit than reimplementing per-ecosystem bump logic. It is a declarative dependency-update tool covering Go, Rust, Java (Maven/Gradle), JavaScript (npm/pnpm/yarn/bun), Python and Ruby.

`rpmx` should not depend on omnibump as part of its data model. It should be an implementation detail of the `go/bump` built-in step:

1. `go/bump` reads `with.deps` and renders an omnibump `deps.yaml`.
2. `rpmx plan` shows the rendered file and the argv; `rpmx source --check` can run `--dry-run`.
3. `rpmx source` applies with `omnibump --deps deps.yaml --tidy` in the source-assembly sandbox.
4. A following `go/vendor` step runs `go mod vendor`.
5. The manifest records the before/after diff, the resolved omnibump digest and the advisory IDs.

The recipe names `go/bump`; it does not name omnibump, its version or an image.
The step library declares that `go/bump` needs `go` and `omnibump`, and the
builder image or active profile supplies them. The lock records the resolved
tool digest. `source-assembly.tools` remains as an advanced override only for a
tool that is not in the builder image.

The built-in step looks like:

```yaml
source-assembly:
  pipeline:
    - uses: go/bump
      with:
        deps:
          - module: golang.org/x/net
            version: v0.33.0
            reason: CVE-2024-45337
            advisory: https://pkg.go.dev/vuln/GO-2024-3333
```

Rules for using omnibump:

- **Exact versions only.** `@latest` is a lock-update operation, not a source-assembly operation. Resolve it in `rpmx lock`, record the exact version, then bump to that.
- **Use `--dry-run` first.** The plan should show the manifest diff and the transitive co-update set before anything changes.
- **Use its co-update detection.** Go modules often require co-updating transitive dependencies; omnibump surfaces the required set and refuses an impossible bump.
- **Use its `+incompatible` handling.** Do not hand-edit Go versions to add suffixes.
- **The lock records omnibump.** It resolves the version and digest from the builder image; the recipe does not name them.
- **No network.** The fetch phase must pre-populate the module/package cache for the target versions; source assembly runs offline.
- **Run tests and vulnerability checks after the bump.** `go test`, `cargo test`, `govulncheck`, `osv-scanner`, etc., depending on the ecosystem.
- **Treat the bump as a source change.** Review the manifest/lock diff, bump the package release, record the advisory and produce a new SRPM.

This gives the project a Go bump mechanism without baking Go-specific update logic into the core. Other ecosystems get their own built-in steps (`cargo/bump`, `npm/bump`, ...) when they are needed.

## 6. Trust and provenance

- **Digest**: mandatory. It is the build's identity for the source.
- **Signature**: optional; verifies publisher identity. OpenPGP, minisign or Sigstore can be supported.
- **Lock**: records the digest, size and signature result. `rpmx lock --update` shows a diff; a digest change is a review event, not a silent update.
- **Mirrors**: untrusted; a mirror that serves the right digest is fine. TLS is for privacy, not integrity.
- **SRPM**: signed with the distro key; includes a provenance attestation listing the recipe/lock/source digests.
- **SBOM**: includes source digests, licenses and upstream URLs.

## 7. Migration from lookaside

`rpmx import-spec` should:

1. Read the spec's `SourceN` URLs and the `sources` file.
2. Match each `sources` hash to a source filename.
3. Create a `sources:` entry with `kind: https` (upstream URL) and `kind: lookaside` (existing cache URL) as a fallback.
4. Verify that the bytes from the lookaside match the recorded hash.
5. Optionally upload the SRPM to an OCI registry or archive it in the repository.

There is no spec export. Native SRPMs are rebuilt with `rpmx`; migration stops at the recipe.

New packages should not depend on the lookaside. If a distro wants to keep it as one locator, that is fine; it is no longer the model.

## 8. Open questions

1. **Should `digest` be the top-level identity, or should `name` + digest be?** Names are useful for humans and extraction; the digest is the identity.
2. **Do we need `size`?** It helps reject bad responses early, but it is not a security boundary.
3. **Which upstream signature formats should `rpmx` verify?** `rpmx` does not sign. It verifies OpenPGP and minisign on inputs, with Sigstore as a plugin; output signing is external.
4. **Should SRPMs be stored as plain files or OCI artifacts?** Both; plain files for repositories, OCI for distribution/signing.
5. **How aggressive should `cache gc` be?** It must never remove objects referenced by the current lock or an SRPM.
6. **Do we need a remote CAS protocol?** Start with HTTP/OCI; add a CAS protocol only if a real mirror needs it.
7. **How do we handle sources that are generated, not fetched?** E.g., vendored dependency tarballs produced during the build. They are outputs, not inputs; they need a separate `generated-sources` phase or a lock entry with a post-build digest.
8. **What about sources that cannot be redistributed?** The SRPM policy must support a restricted/private SRPM, with an explicit legal flag, while still satisfying the rebuild requirement where legally possible.

## 9. Summary

The lookaside cache is a hash manifest plus a private blob store. Keep the hash manifest; replace the private store with:

- **digest + locators** in the recipe;
- **local CAS** for caching;
- **standard transports** (HTTPS, OCI, LFS, file);
- **SRPMs** for archival and offline rebuilds;
- **no mandatory distro-specific service**.

That gives RPM-style reproducibility without making every rebuild depend on one cache.
