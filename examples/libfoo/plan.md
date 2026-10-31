# Worked plan: libfoo

Illustrative, not executed.

## 1. Source resolution

```
source libfoo:
  kind         git
  url          https://github.com/example/libfoo.git
  commit       0123456789abcdef0123456789abcdef01234567
  destination  libfoo
```

The commit is the source identity. `rpmx` checks it out and records a
deterministic archive digest in the lock.

## 2. Source assembly

```
checkout libfoo at 0123456789abcdef...
run: python3 tools/gen-parser.py
run: python3 tools/check-generated.py
collect: generated parser tables, LICENSE
snapshot digest: sha512:...
sandbox: source-assembly, no network, no credentials
```

The generator is pinned in the lock (`python3`, Ubuntu package version or
container digest). The generated files are part of the source snapshot; they are
not regenerated during the build.

If `check-generated.py` detects a diff after regeneration, source assembly
fails. This catches a generator that is nondeterministic or a generated file
that was accidentally committed stale.

## 3. SRPM contents

```
SOURCES/libfoo-2.3.4.tar.zst       # source snapshot, including generated tables
SOURCES/rpmx/package.yaml
SOURCES/rpmx/package.lock.json
SOURCES/rpmx/RPMX-MANIFEST.json
SOURCES/rpmx/patches/0001-....patch
```

`RPMX-MANIFEST.json` records:

```
source assembly:
  snapshot digest
  python3 version
  tools/gen-parser.py command
  tools/check-generated.py command
patch 0001: digest, strip level
```

## 4. Build

```
meson setup build --prefix=/usr -Dtests=true
meson compile -C build
meson test -C build --print-errorlogs
meson install -C build --destdir ${stage}
```

No network and no generator. The build consumes the source snapshot from the
SRPM.

## 5. Outputs

- `libfoo`: `/usr/lib/libfoo.so.2*`
- `libfoo-devel`: headers, `libfoo.so`, `foo.pc`

The exact manifest is computed after `install`; overlapping claims fail.

## 6. What this demonstrates

- Generated source belongs in source assembly, not `%build`.
- The SRPM contains the generated snapshot, so `rpmx rebuild` is offline.
- The generator and its output are auditable and content-addressed.
- A nondeterministic generator fails before an SRPM is produced.
- A patch remains unapplied in the SRPM and is applied during the build.
