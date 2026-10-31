# Worked plan: go-bump

Illustrative, not executed. This is a CVE-remediation rebuild: the upstream
version stays `1.4.0`, the package release moves from `1` to `2`.

## 1. Source resolution

```
source hello-api:
  kind       go-module
  module     github.com/example/hello-api
  version    v1.4.0
  digest     sha512:<placeholder>
  sum        h1:<placeholder>
  locators   proxy.golang.org
             registry.example.org/rpmx/sources/hello-api@sha256:...
```

## 2. Bump plan

The recipe asks for two Go module bumps:

```
golang.org/x/net          -> v0.33.0   CVE-2024-45337  GO-2024-3333
github.com/sirupsen/logrus -> v1.9.3    CVE-2023-45142  GO-2023-2135
```

`rpmx` translates them to an omnibump `deps.yaml`:

```yaml
packages:
  - name: golang.org/x/net
    version: v0.33.0
  - name: github.com/sirupsen/logrus
    version: v1.9.3
```

and runs:

```
omnibump --deps deps.yaml --dry-run
```

The dry run reports the direct changes and any required co-updates. If the
selected versions require newer transitive modules, the plan fails until those
are added explicitly.

## 3. Fetch

The fetch phase must pre-populate the module cache for:

- the original module graph;
- the target versions of the bumped modules;
- the co-updated transitive modules;
- the `go` toolchain (or use `GOTOOLCHAIN=local`).

No network is available during source assembly.

## 4. Source assembly

```
omnibump --deps deps.yaml --tidy
go mod verify
go mod vendor
```

Then record:

- `go.mod`/`go.sum` before and after;
- `go list -m all` before and after;
- `vendor/modules.txt`;
- omnibump version and invocation;
- advisory IDs and reason;
- new source snapshot digest.

The SRPM contains the bumped snapshot and a new manifest. It is a new source
artifact, not a mutation of the old SRPM.

## 5. Build and test

```
go test ./...
go build -mod=vendor -trimpath -buildvcs=false -o /usr/bin/hello-api .
govulncheck ./...
```

`govulncheck` uses a prepared vulnerability database; it does not reach the
network from the build sandbox. The build uses `vendor/` and `GOTOOLCHAIN=local`.

## 6. Release

```
release: 1 -> 2
```

The changelog entry names the advisories and the bumped modules. The SRPM and
binary RPMs are signed; the SBOM records the new module versions and PURLs.

## 7. What this demonstrates

- Bumps happen in source assembly, before the SRPM.
- `omnibump` is the universal bump engine, not part of the build script.
- Exact versions are used; `@latest` is resolved during lock update, not during
  source assembly.
- Go-specific vendoring still runs after the bump.
- The new SRPM is the durable artifact for the CVE remediation.
