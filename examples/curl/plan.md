# Worked elaboration: `curl`

This is the plan that `rpmx plan --profile fedora-44` would produce for
[`package.yaml`](package.yaml). It is illustrative: no build was executed, and
the file hashes in the recipe are placeholders.

## 1. Inputs

```
recipe:   package.yaml          sha512:...
profile:  fedora-44             sha512:...
features: default + ssl + http2 (from default)
lock:     absent (first planning run)
```

## 2. Feature resolution

Requested:

```
default = ["ssl", "http2"]
```

Constraints:

```
ssl      requires tls
gnutls   requires tls, conflicts ssl
http2    (none)
idn      (none)
```

Result:

```
enabled  = ["ssl", "http2", "tls"]
disabled = ["gnutls", "idn"]
```

Why `tls` was enabled: `ssl.requires = ["tls"]`. `rpmx explain features ssl`
would show this edge.

Invalid alternatives:

```
--features +gnutls   -> error: gnutls conflicts with ssl; disable ssl or drop gnutls
--features -tls,+ssl -> error: ssl requires tls
```

## 3. Sources and patches

```
source curl:
  digest       <placeholder>
  size         1234567
  signature    openpgp, fingerprint 27EDEAF22F3ABCE1
  locators     https://curl.se/download/curl-8.11.1.tar.xz
               https://mirror.example.org/...
  extract      tar.xz, strip-components=1

patch 0001-curl-ca-bundle-path.patch
  strip 1
  reason "Use the distro CA bundle path instead of the upstream default"
  resolves https://github.com/curl/curl/pull/0000
```

The plan fails before fetching if a hash is missing, a signature is required by
policy and absent, or a patch has no `reason`.

## 3a. Source assembly

```
source snapshot: curl-8.11.1.tar.xz, strip-components=1
vendoring:       none (C/CMake project)
generation:      none
license files:   COPYING, COPYING.legacy
snapshot digest: sha512:...
sandbox:         source-assembly, no network, no credentials
```

A Go/Cargo/npm package would vendor/generate here instead. The snapshot digest
goes into the SRPM manifest; the build never runs a generator with network
access.

## 4. Build dependency graph

Scopes:

```
tools: cmake >= 3.20, ninja, pkgconf, gcc, gcc-c++
host:  openssl-devel >= 3.0, zlib-devel, libpsl-devel, libnghttp2-devel
tests: python3
```

Feature provenance:

```
openssl-devel   <- feature ssl
libnghttp2-devel <- feature http2
zlib-devel      <- recipe
libpsl-devel    <- recipe
```

Generated dependencies (after the build, from the actual ELF files):

```
libcurl.so.4()(64bit)       from libcurl
libc.so.6()(64bit)          from curl, libcurl
libssl.so.3()(64bit)        from libcurl (if linked)
libcrypto.so.3()(64bit)     from libcurl
```

`rpmx explain requires libcurl` would show direct, feature, profile and
generated edges separately.

## 5. Step graph

The `cmake` bundle expands to:

```
step 1  cmake/configure
        cwd  /build/curl-8.11.1
        argv cmake -S . -B build \
               -DCMAKE_BUILD_TYPE=Release \
               -DCURL_HIDDEN_SYMBOLS=ON \
               -DCURL_USE_OPENSSL=ON \
               -DCURL_USE_NGHTTP2=ON \
               -DCURL_USE_LIBPSL=ON \
               -DBUILD_TESTING=ON
step 2  cmake/build
        argv cmake --build build --parallel ${jobs}
step 3  cmake/check
        argv ctest --output-on-failure --test-dir build
step 4  cmake/install
        argv cmake --install build --prefix /usr
step 5  strip
        argv strip --strip-unneeded ...
step 6  remove-duplicate-changelog
        argv rm -rf ${stage}/usr/share/doc/curl/CHANGES
```

Source unpacking and patch application happen before step 1, as part of
entering the sandbox from the SRPM payload. Every step is either an `argv`
array or an inline shell fragment; the plan prints the resolved command, and
the step definition is recorded by digest in the lock.

## 6. Output manifest (predicted selectors)

| Output | Selector | Predicted paths |
|---|---|---|
| `curl` | `/usr/bin/curl` | `/usr/bin/curl` |
| `libcurl` | `/usr/lib/libcurl.so.4*` | `/usr/lib/libcurl.so.4`, `/usr/lib/libcurl.so.4.8.0` |
| `libcurl-devel` | `/usr/include/curl/**`, `/usr/lib/libcurl.so`, `/usr/lib/pkgconfig/libcurl.pc` | headers, unversioned symlink, pkg-config file |
| `curl-doc` | `/usr/share/doc/curl/**`, `/usr/share/man/**` | docs, man pages, license |

After the build, the scanner replaces "predicted" with the actual manifest and
checks:

```
[ok] every staged file is claimed exactly once
[ok] no file is claimed by two outputs
[ok] no output is empty
[ok] libcurl-devel does not claim the versioned .so
[ok] curl-doc is noarch and contains no ELF objects
[ok] /usr/share/doc/curl/COPYING is marked as a license
[ok] /usr/bin/curl has mode 0755
```

A duplicate claim would fail like this:

```
RPMX-FILE-007: /usr/lib/libcurl.so.4 is claimed by both 'libcurl' and 'libcurl-devel'
  package.yaml:... output[2].files.include
  suggestion: remove the versioned path from libcurl-devel, or use auto-split
```

## 7. Actions

`libcurl`:

```
type: ldconfig
output: libcurl
phases: post, postun
emitted as: %post -n libcurl -p /sbin/ldconfig
            %postun -n libcurl -p /sbin/ldconfig
```

No shell action is present. If one were added without a `reason`, lint would
fail. If the profile sets `forbid-shell-actions = true`, a shell action would be a
policy error.

## 8. Tests

| Test | Stage | What it checks |
|---|---|---|
| `unit` | build | `ctest` in the build tree |
| `smoke` | installed | `curl --version` in a disposable root with the outputs installed |
| `abi` | installed | exported symbols of `libcurl.so.4` against `abi/libcurl-8.10.json` |
| `reproduce` | package | two builds in different roots produce identical unsigned payloads |

`rpmx test --install` would:

1. Build the RPMs.
2. Create a disposable root from the profile's minimal package set.
3. Install `curl`, `libcurl`, `libcurl-devel`, `curl-doc` and their runtime deps.
4. Run `curl --version`.
5. Run `rpm -V` on each package.
6. Compare the installed file manifest with the plan.
7. Run the ABI check.

## 9. What the lock would record

```
recipe-digest       sha512:...
profile             fedora-44
profile-digest      sha512:...
features            ["ssl", "http2", "tls"]
release             1
source curl         digest, size, signature-verified, locator
source snapshot     sha512 digest, generators/tools
patch 0001          sha512
build toolchain     gcc 14.2.1, cmake 3.31.0, ninja 1.12.1
build root digest   sha512:...
outputs.curl        manifest-digest, file count
outputs.libcurl     manifest-digest, file count
outputs.libcurl-devel manifest-digest, file count
outputs.curl-doc    manifest-digest, file count
source-date-epoch   1735689600
```

`rpmx build --locked` fails if the recipe, profile, source hashes or build-root
digest differ.

## 10. What changed versus the spec

| Concern | Spec | Recipe |
|---|---|---|
| Feature selection | `%bcond` + `%if` | `[features]` with `requires`/`conflicts` |
| Build steps | `%build` shell + macros | `cmake` bundle + inline steps |
| Dependency scope | `BuildRequires` | `tools` / `host` / `tests` |
| File ownership | four `%files` sections with globs | four `[[output]]` manifests, checked for totality |
| Install-time action | `%post -p /sbin/ldconfig` | typed `ldconfig` action |
| Tests | `%check` only | build, installed, ABI and reproduce tests |
| Reproducibility | packager's problem | `SOURCE_DATE_EPOCH`, lock, `--reproduce` |
| Evidence | separate tooling | SBOM + provenance + manifest digest |
| Policy | `%if 0%{?fedora}` | `--profile fedora-44` |
