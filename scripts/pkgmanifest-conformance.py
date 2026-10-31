#!/usr/bin/env python3
"""Parse an rpmx-exported manifest with upstream libpkgmanifest.

This is the conformance oracle for `rpmx root export`: it proves that the
document a native Rust serializer produces is a document the reference
implementation accepts and can serialise again.
"""

import sys

import libpkgmanifest.manifest as manifest_lib

path = sys.argv[1]
archs = sys.argv[2:] or ["x86_64", "noarch"]

parsed = manifest_lib.Parser().parse(path)
assert parsed.document == "rpm-package-manifest", parsed.document
assert (parsed.version.major, parsed.version.minor, parsed.version.patch) == (0, 2, 3), (
    parsed.version.major,
    parsed.version.minor,
    parsed.version.patch,
)

# `packages[arch]` includes noarch packages for that basearch, so deduplicate
# across the requested architectures instead of summing the groups.
seen = {}
for arch in archs:
    for package in parsed.packages[arch]:
        key = (
            package.nevra.name,
            package.nevra.epoch,
            package.nevra.version,
            package.nevra.release,
            package.nevra.arch,
        )
        seen[key] = package

for package in seen.values():
    assert package.repo_id, "package has no repository id"
    assert package.nevra.name, "package has no name"
    assert package.checksum.digest, "package has no checksum digest"

total = len(seen)
assert total > 0, "manifest contains no packages"
manifest_lib.Serializer().serialize(parsed, path + ".reserialized")
print(f"conformance ok: {total} packages parsed and re-serialised")
