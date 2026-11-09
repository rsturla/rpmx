# The surface syntax: TOML is probably the wrong choice

## The problem

The `curl` example exposed it. This is the TOML for one output:

```toml
[[output]]
name = "libcurl"
summary = "Library for transferring data with URLs"
description = "libcurl is a client-side URL transfer library."
arch = "any"
files = { include = ["/usr/lib/libcurl.so.4*"] }

[output.dependencies]
run = ["ca-certificates"]
provides = ["libcurl.so.4()(64bit)"]

[[output.action]]
type = "ldconfig"
```

It is not *wrong*. It is just visually noisy:

- `[[output]]` is a speed bump every time a new package output starts.
- `[output.dependencies]` silently attaches to the previous `[[output]]`; the reader has to know TOML's array-of-tables scoping rules.
- `files = { include = [...] }` is an inline table, which is compact but visually different from every other nested section.
- `[features.ssl.dependencies]` and `[features.ssl.pipeline]` encode hierarchy in header text rather than indentation.
- Arrays of strings with commas and brackets are harder to scan than a block list.
- There is no visual cue for where one output ends and the next begins beyond the next `[[...]]`.

TOML is excellent for flat or shallow configuration (Cargo, `pyproject.toml`). A package definition is a tree with repeated objects: sources, patches, features, outputs, file attributes, actions, tests. That is the shape TOML handles least gracefully.

The irony is that the design's whole thesis is "make the structure visible and checkable". The surface syntax should do the same.

---

## What the surface syntax has to do

1. **Show hierarchy at a glance.** A reader should see that `files`, `dependencies`, `actions` belong to one output without counting brackets.
2. **Make repeated objects obvious.** Sources, patches, outputs, actions and tests are lists of similar things; each item should have a clear start and end.
3. **Stay typed and schema-validatable.** The format must reject unknown fields, wrong types and invalid values before the build.
4. **Support comments and multiline text.** Packaging needs explanations and descriptions.
5. **Be familiar to packagers.** A new format already has a learning cost; the syntax should not add a second one.
6. **Have good editor/tooling support.** Syntax highlighting, schema completion, formatters, linters.
7. **Avoid implicit magic.** No anchors, no merge keys, no type coercion surprises.
8. **Be canonicalizable.** `rpmx fmt` must produce one obvious representation.

---

## Candidates

### TOML

**Pros:** explicit types, no significant whitespace, comments, mature parsers, good for flat data.

**Cons:** array-of-tables syntax is awkward; nested tables are header-driven; inline tables encourage dense one-liners; hierarchy is not visual. The `curl` example is the evidence.

**Verdict:** good for the lockfile and tool configuration; poor for the recipe.

### YAML 1.2 (strict profile)

```yaml
outputs:
  - name: libcurl
    summary: Library for transferring data with URLs
    files:
      include:
        - /usr/lib/libcurl.so.4*
    dependencies:
      run:
        - ca-certificates
      provides:
        - libcurl.so.4()(64bit)
    actions:
      - type: ldconfig
```

**Pros:** visual hierarchy; block lists are easy to scan; multiline strings (`|`) are natural; comments; ubiquitous editor support; JSON Schema integration; used by melange, Flatpak, Snap, Solus and Kubernetes, so packagers have seen it.

**Cons:** YAML has a reputation for implicit typing (`yes`, `on`, dates, floats), anchors/aliases, and indentation mistakes. These are solvable with a strict profile:

- YAML 1.2 core schema only; `true`/`false` only.
- Reject duplicate keys, anchors, aliases and merge keys.
- Reject custom tags.
- Require quoted strings for versions, hashes and modes.
- Validate with a JSON Schema before doing anything else.
- `rpmx fmt` canonicalizes indentation and key order.
- `rpmx lint` rejects unquoted version-like scalars, tabs, and multiline plain scalars.

**Verdict:** the best balance of readability, familiarity and tooling. This is the recommendation.

### KDL

```kdl
output "libcurl" {
    summary "Library for transferring data with URLs"
    files {
        include "/usr/lib/libcurl.so.4*"
    }
    dependencies {
        run "ca-certificates"
        provides "libcurl.so.4()(64bit)"
    }
    action "ldconfig"
}
```

**Pros:** very readable; node/property model maps well to package objects; no implicit typing; comments; compact.

**Cons:** much smaller ecosystem; less editor/schema tooling; packagers have not seen it; the available parser/formatter ecosystem is less mature; the "node with children and properties" model can be ambiguous for arrays of objects.

**Verdict:** the most elegant option, and a reasonable runner-up if we want a distinctive format. Too risky for v1.

### A custom line-oriented DSL

```text
output libcurl {
  summary "Library for transferring data with URLs"
  files /usr/lib/libcurl.so.4*
  requires ca-certificates
  provides libcurl.so.4()(64bit)
  action ldconfig
}
```

**Pros:** can be optimized exactly for packaging; very readable; small grammar; excellent errors if designed well.

**Cons:** we would design, document, parse, format, lint and maintain a new language; editor support is a project of its own; every edge case (quoting, escapes, multiline strings, versions, operators) becomes our problem. This is how MacPorts ended up with Tcl and Gentoo with Bash: a custom language is easy to start and expensive to own.

**Verdict:** reject for v1. Revisit only if YAML proves too limiting.

### CUE

**Pros:** schema and data in one language; constraints; strong validation.

**Cons:** niche; unfamiliar syntax; steep learning curve; overkill for packagers; tooling less common.

**Verdict:** interesting for schema authors, not for packagers.

### Starlark / Python / Nix / Scheme

**Pros:** expressive; reusable functions; can generate repetitive structures.

**Cons:** Turing-complete (or close); requires execution to understand; raises the barrier; repeats the macro problem in a different language.

**Verdict:** reject as the primary format; allow inline pipeline steps as the controlled extension point.

---

## Recommendation: YAML 1.2, strict profile, JSON Schema, canonical formatter

The recipe becomes `package.yaml`. The lock becomes `package.lock.json` (machine-generated, unambiguous, JSON Schema-validated). The CLI emits JSON; the schema is generated from the implementation's types.

### The same recipe, readable

See [`examples/curl/package.yaml`](../../examples/curl/package.yaml) for the full version. A representative slice:

```yaml
package:
  name: curl
  version: "8.11.1"
  release: 1
  summary: A command line tool for transferring data with URLs
  license: curl

build:
  pipeline:
    - uses: cmake
      with:
        args:
          - -DCMAKE_BUILD_TYPE=Release
          - -DCURL_USE_OPENSSL=ON
  packages:
    - cmake >= 3.20
    - ninja
  sysroot:
    - openssl-devel >= 3.0
    - zlib-devel

features:
  default: [ssl, http2]
  ssl:
    requires: [tls]
    conflicts: [gnutls]
    dependencies:
      host:
        - openssl-devel >= 3.0

outputs:
  - name: libcurl
    summary: Library for transferring data with URLs
    files:
      include:
        - /usr/lib/libcurl.so.4*
    dependencies:
      run:
        - ca-certificates
      provides:
        - libcurl.so.4()(64bit)
    actions:
      - type: ldconfig
```

The hierarchy is now visual. `outputs:` is a list; each output starts with `- name:`; `files`, `dependencies` and `actions` are visibly children of that output. There is no `[[...]]`, no "last array element" scoping, and no inline table.

### Strict YAML rules

These are non-negotiable if YAML is the format:

1. **YAML 1.2 core schema only.** Booleans are exactly `true` and `false`. No `yes`, `no`, `on`, `off`.
2. **Duplicate keys are errors.** The parser must reject them; most YAML libraries do not by default.
3. **No anchors, aliases or merge keys.** They make a document non-local and defeat schema validation.
4. **No custom tags.** Only the standard scalar/sequence/mapping tags.
5. **Quote versions, hashes and modes.** `version: "8.11.1"`, `digest: "sha512:..."`, `mode: "0755"`. Lint rejects unquoted values that look like versions or octal modes.
6. **No tabs for indentation.** Two spaces, enforced by the formatter.
7. **Canonical key order.** `rpmx fmt` orders keys by schema, not alphabetically, so the document reads in build order.
8. **One document per file.** Reject trailing documents.
9. **JSON Schema validation before feature resolution.** Unknown fields, wrong types and missing required fields are errors with a line, column and field path.
10. **Schema completion in editors.** Ship a `yaml-language-server` schema reference at the top of every example:
    ```yaml
    # yaml-language-server: $schema=https://example.org/rpmx/package.schema.json
    ```

### Why not keep TOML for the recipe and YAML only for examples?

That would give two authoring formats and two sets of docs. It doubles the surface area and the migration work. Pick one. YAML is the better recipe surface; JSON is the better lock surface.

### What about the lockfile?

Use JSON:

- it is machine-generated, so YAML's ergonomics do not matter;
- no implicit typing or duplicate-key ambiguity;
- easy to hash and diff;
- JSON Schema works;
- the CLI already emits JSON.

If human review of the lock is important, `rpmx lock --diff` can render a readable diff.

---

## Could we support YAML, JSON, TOML and KDL at once?

Yes. In fact, that is the cleanest architecture if we separate three things:

1. **The data model** — the canonical tree the planner understands.
2. **The codecs** — parsers/emitters for YAML, JSON, TOML, KDL.
3. **The authoring surface** — the format we document, format, lint and optimize for humans.

```
package.yaml / package.json / package.toml / package.kdl
                         |
                  format frontend
             (parser + source spans)
                         |
              canonical document
        (JSON-compatible: maps, lists,
         strings, integers, booleans)
                         |
             schema + policy + planner
                         |
               package.lock.json
```

That architecture is worth having regardless of which formats ship. It forces the schema to be explicit and stops syntax from leaking into the semantics.

### The catch: the formats do not all have the same data model

| Format | Fit to a JSON-compatible tree | Main issue |
|---|---|---|
| **JSON** | Exact | No comments; unpleasant for humans. |
| **YAML** | Exact after parsing | Implicit typing, anchors, aliases, duplicate keys. |
| **TOML** | Good, but not exact | No `null`; arrays of tables are awkward; dates/floats exist but we would not use them. |
| **KDL** | Not exact | Nodes/properties/children, not maps/arrays. A normative mapping is required. |

This is why "support all four" is not just four parser libraries. It means defining a **format-neutral model**, choosing parsers that preserve enough source information, and deciding how each syntax maps onto the model.

### We would not write the parsers

Every candidate has mature libraries:

| Format | Typical libraries | What we still have to do |
|---|---|---|
| JSON | stdlib (`encoding/json`, `serde_json`, `json`) | Schema validation; stable diagnostics. |
| YAML | `ruamel.yaml`, `js-yaml`, `go-yaml`, maintained Rust YAML crates | Reject anchors/aliases/merge keys/duplicates; enforce YAML 1.2 core scalar rules; preserve spans. |
| TOML | `tomllib`, `toml`, `BurntSushi/toml`, `@iarna/toml` | Reject `null`/date/float surprises; normalize to the canonical model; handle arrays of tables. |
| KDL | `kdl` Rust crate, `kdljs`, other KDL libraries | Define the node/property-to-model mapping; accept a smaller ecosystem. |

The work is in the **adapter**, not the grammar:

- choose a parser that exposes byte/line/column spans;
- configure it to be strict (duplicate keys, aliases, tags);
- map its generic value into the typed canonical model;
- normalize scalars (`"1"` vs `1`, booleans, versions);
- produce our own schema diagnostics with the parser's spans;
- fuzz the integration and enforce size/depth limits.

That is a few hundred lines per format, not a parser project. The real argument for fewer first-class formats is docs, formatters, editor schemas and user confusion, not parser implementation.

### Constraints that make multi-format work

- **The canonical model is JSON-compatible**: maps with string keys, ordered lists, strings, integers, booleans. No `null`; use absence. No dates; use strings. No floats unless a field genuinely needs them.
- **Ordered lists are semantic** for sources, patches, outputs, actions and tests. Maps are unordered.
- **Comments are not part of the model.** Converting formats loses comments. `rpmx convert` must say so.
- **Duplicate keys are errors in every format.**
- **Numbers and booleans are normalized at parse time**, not left to the format's defaults.
- **KDL needs a defined mapping**, e.g. a node name is a map key, repeated nodes are list items, node arguments/properties are fields. That mapping is a spec of its own, not an implementation detail.
- **TOML needs a no-`null` model** and a convention for arrays of tables. It can represent the model, but the result is the least readable for this tree.

### What I would actually ship

**First-class authoring formats: YAML and JSON.**

- YAML is the human-facing default: hierarchy, comments, multiline text.
- JSON is the machine-facing equivalent: exact, unambiguous, easy to generate and diff.
- Both map 1:1 to the canonical model.
- `rpmx fmt` formats YAML and JSON.
- `rpmx lint` validates both against the same schema.
- Docs use YAML; JSON is documented as the generated/machine form.

**Import/export codecs: TOML and KDL.**

- `rpmx convert --to toml` and `--to kdl` work.
- `rpmx build package.toml` and `rpmx build package.kdl` work by detecting the format.
- They are not the documented authoring surface, and `rpmx fmt` may not preserve their comments/formatting.
- This gives the interoperability benefit without maintaining four sets of docs, four error-span implementations and four formatter behaviors.

**If you want all four to be first-class**, it is still possible, but the cost is explicit:

- four codec integrations (using existing parser libraries) and four emitters;
- four source-span/error implementations;
- four formatter behaviors;
- four documentation examples for every feature;
- four security/fuzzing surfaces;
- a normative KDL mapping;
- a policy for what "round-trip" means (semantic equality, not byte equality).

That is a product decision, not a technical blocker. My recommendation is **one primary authoring format, two first-class codecs (YAML/JSON), and the other two as converters**. It keeps the "it is all the same data" property without pretending the syntaxes are equally good at expressing it.

### Round-trip guarantee

The guarantee should be:

> `parse(emit(model, format), format) == model`

for every supported format, excluding comments and formatting. That is a property test, not a promise that a TOML file and a YAML file look alike. If a format cannot represent a value (e.g., TOML `null`), the model must not contain that value.

### What this means for the lock

The lock stays **JSON**, regardless of the recipe format. It is machine-generated, has no comments, and is consumed by tools. Letting it be YAML/TOML/KDL adds no value.

---

## Decision

**Switch the primary authoring format from TOML to strict YAML 1.2.** Keep JSON for the lock and machine interfaces. Keep the typed model, schema, planner and semantics unchanged; only the surface changes.

This also aligns with the modern tools that influenced the design: melange, Flatpak, Snap, Solus and apko all use YAML for their human-facing definitions. The design should learn from that, not fight it.

## What changes in the dossier

- `package.toml` becomes `package.yaml` everywhere.
- `package.lock.toml` becomes `package.lock.json`.
- The schema is JSON Schema, generated from the implementation types.
- `rpmx fmt` canonicalizes YAML.
- `rpmx lint` enforces the strict YAML profile.
- The worked example and plan are updated.
- TOML remains an accepted *output* format for machine consumption if needed, not a primary authoring format.

## Open question

The only real alternative worth considering is **KDL**: it is more elegant and avoids YAML's typing baggage, at the cost of ecosystem and familiarity. If readability is the top priority and we are willing to own a smaller tooling ecosystem, KDL is the runner-up. YAML is the safer bet for packagers and CI.
