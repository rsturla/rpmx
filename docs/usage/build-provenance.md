# Build-input provenance

Normal build requirements select packages. rpmx records the resolved build and
sysroot RPMs automatically. Consumer recipes need no second toolchain list.

A provider can add portable capabilities with the existing dependency syntax:

```yaml
outputs:
  - name: example-compiler
    dependencies:
      provides:
        - rpmx(toolchain:example:1) = ${package.version}
        - rpmx(component:example-stdlib:1) = ${package.version}
```

Names identify a stable capability and optional compatibility stream. They do
not have to match the provider's RPM name. Values use RPM version ordering.
Use a separate capability generation when a repair has no upstream version
change, or set a provider EVR floor in the distribution's repair policy.

Each production binary RPM contains
`/usr/share/rpmx/buildinfo/<output>.json`. The receipt records the source RPM,
output architecture, exact input names, EVRs, root roles, checksums, source
RPM identities and provider capabilities. It stays with the signed payload
and published RPM. It does not depend on retention of CI artifacts.

Run `rpmx provenance FILE.rpm` to read it as JSON. Legacy or stage-only RPMs
return `null`; malformed receipts fail. Root reuse without cached input RPMs
retains locked identities but sets `roots_complete` to false. Repair checks
must treat missing or incomplete coverage conservatively. Corrupt cached RPMs
fail the build. Keep the input CAS when exporting roots for audited builds.

For each advertised `rpmx(...)` capability, a consumer gets a potential-influence
provide such as `buildinput(toolchain:example:1) = 1.2.3`. This is not proof that
the compiler ran or that a component is bundled. Exact input revisions remain
in the receipt, including revisions that only change a backported fix.

## Component coverage

A trusted shared build step can refine potential influence for one output and
one exact provider. The build can read `${provenance.dir}/inputs.json`. Write
one JSON report per output/provider under `${provenance.dir}/reports/`, or use
`provenance/record` with `file` and a unique `name` to copy a generated report.

```json
{
  "schema": 1,
  "output": "example-app",
  "root": "build",
  "checksum": "sha512:<exact-provider-RPM-checksum>",
  "complete": true,
  "used": ["rpmx(component:example-core:1)"]
}
```

`complete: true` asserts coverage of **all** `rpmx(component:*)` capabilities
advertised by that provider for that output. Only then can absent components,
such as crypto, be excluded. A partial report cannot remove any component.
Toolchain capabilities cannot be removed by component coverage. Unknown
outputs, unknown providers, unadvertised components and duplicate reports
fail the build. Reports are assertions by trusted recipe/build-step code.
rpmx validates their scope; it cannot prove the external analysis is correct.

The current built-in steps do not infer fine-grained component use. A shared
step can implement a compiler-specific analysis and emit this general format.
A negative grep result alone is not complete coverage. The provider must first
advertise the component identities that the analysis reports.

Source-carried reports are cleared before production steps. Inputs are
captured on the host, so a build cannot rewrite the authoritative inventory.
Reports are captured before build tests. Test mutations cannot change the
packaged receipt. The SRPM contains no resolved native build receipt, so the
same shared SRPM remains usable on every target architecture.

Coverage concerns the locked binary build roots. It does not attest source
assembly tools, execution of individual commands, or downstream runtime
vulnerability status. Stage-only packaging makes no build-input claim.
