# Worked plan: hello-go

Illustrative, not executed.

## 1. Source resolution

```
source hello:
  kind       go-module
  module     github.com/example/hello
  version    v1.4.0
  digest     sha512:<placeholder>
  sum        h1:<placeholder>
  locators   proxy.golang.org
             registry.example.org/rpmx/sources/hello-go@sha256:...
```

The fetch phase asks the proxy for the module zip and verifies:

- the `go.sum` / sumdb `h1:` value;
- the SHA-512 digest recorded in the lock;
- the module path and version.

No hand-made tarball is involved. The module proxy is a locator, not a build-time dependency.

## 2. Source assembly

```
extract module zip
run: go mod download
run: go mod verify
run: go mod vendor
collect: go.mod, go.sum, vendor/, LICENSE
snapshot digest: sha512:...
sandbox: source-assembly, no network, no credentials
```

The vendored tree and module metadata become part of the source snapshot. The build later uses `-mod=vendor`.

## 3. SRPM contents

```
SOURCES/hello-go-1.4.0.tar.zst       # deterministic snapshot with vendor/
SOURCES/rpmx/package.yaml
SOURCES/rpmx/package.lock.json
SOURCES/rpmx/RPMX-MANIFEST.json
```

`RPMX-MANIFEST.json` records:

```
source assembly:
  snapshot digest
  go version
  go mod vendor command
module identity:
  github.com/example/hello v1.4.0
  h1:...
  sha512:...
```

## 4. Build

```
go build -mod=vendor -trimpath -buildvcs=false -o /usr/bin/hello .
```

No network. The Go toolchain comes from the build root; modules come from `vendor/`.

## 5. Tests

```
installed: hello
```

## 6. What this demonstrates

- A Go package can be acquired from a module proxy without a packager-created tarball.
- The SRPM still contains the exact source snapshot required for offline rebuilds.
- The module proxy is a locator, not the source of truth.
- SHA-512 is the policy digest; `h1:` is Go’s transport/module sum.
- The source-assembly sandbox is separate from the build sandbox.
