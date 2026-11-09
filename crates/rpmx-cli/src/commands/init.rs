//! Create a package that builds on the first try.

use std::path::Path;

use rpmx_core::error::codes;
use rpmx_core::Error;

use crate::commands::InitArgs;

const PROFILE: &str = r#"document: rpmx-profile
version: 1
name: fedora-44
releasever: "44"
dist-tag: .fc44
archs: [x86_64]
builder:
  image: registry.fedoraproject.org/fedora:44
repositories:
  - id: fedora
    metalink: https://mirrors.fedoraproject.org/metalink?repo=fedora-$releasever&arch=$basearch
  - id: updates
    metalink: https://mirrors.fedoraproject.org/metalink?repo=updates-released-f$releasever&arch=$basearch
packages:
  install: [bash, coreutils, findutils, gcc, make]
sbom: true
options:
  weak_deps: false
  docs: false
"#;

const README: &str = r#"# {name}

Build it:

```console
rpmx lint
rpmx build
```

The artifacts land in `packages/<arch>/`. Tests run with `rpmx test`.

`package.yaml` is the recipe. `profiles/` holds the build root. Both are
reviewed like code: `rpmx lock` records the resolved sources, steps and root.
"#;

const GITIGNORE: &str = "packages/\n.rpmx/\n";

const C_RECIPE: &str = r#"schema: 1

package:
  name: {name}
  version: "0.1.0"
  release: 1
  summary: {name}
  license: MIT
  source-date-epoch: 1704067200
  maintainers:
    - name: Your Name
      email: you@example.org

build:
  root:
    profile: fedora-44
  packages: [gcc, make, bash]
  pipeline:
    - uses: make
    - runs: |
        install -D -m 0644 LICENSE "$STAGE/usr/share/licenses/{name}/LICENSE"
    - uses: strip

source-assembly:
  pipeline:
    - runs: |
        cat > hello.c <<'SOURCE'
        #include <stdio.h>

        int main(void) {
            puts("hello from {name}");
            return 0;
        }
        SOURCE
        cat > Makefile <<'SOURCE'
        all: hello
        hello: hello.c
        	$(CC) $(CFLAGS) -o $@ hello.c
        check: hello
        	./hello
        install: hello
        	install -D -m 0755 hello $(DESTDIR)/usr/bin/hello
        SOURCE
        cat > LICENSE <<'SOURCE'
        MIT License

        Copyright (c) {name} contributors

        Permission is hereby granted, free of charge, to any person obtaining a copy
        of this software and associated documentation files (the "Software"), to deal
        in the Software without restriction.
        SOURCE

outputs:
  - name: {name}
    summary: {name}
    files:
      include:
        - /usr/bin/hello
        - /usr/share/licenses/{name}/LICENSE
      license:
        - /usr/share/licenses/{name}/LICENSE

tests:
  - name: smoke
    stage: build
    argv: ["./hello"]
  - name: installed
    stage: installed
    argv: ["hello"]
"#;

const METAPACKAGE_RECIPE: &str = r#"schema: 1

package:
  name: {name}
  version: "0.1.0"
  release: 1
  summary: {name} metapackage
  license: MIT
  source-date-epoch: 1704067200
  maintainers:
    - name: Your Name
      email: you@example.org

build:
  root:
    profile: fedora-44
  packages: [bash]
  pipeline:
    - runs: "true"

outputs:
  - name: {name}
    metapackage: true
"#;

const PYTHON_RECIPE: &str = r#"schema: 1

package:
  name: {name}
  version: "0.1.0"
  release: 1
  summary: {name}
  license: MIT
  source-date-epoch: 1704067200
  maintainers:
    - name: Your Name
      email: you@example.org

build:
  root:
    profile: fedora-44
  packages: [python3, python3-pip, python3-setuptools, bash]
  pipeline:
    - runs: |
        python3 -m pip install . --root "$STAGE" --prefix /usr --no-deps --no-build-isolation
        install -D -m 0644 LICENSE "$STAGE/usr/share/licenses/{name}/LICENSE"
    - uses: strip

source-assembly:
  pipeline:
    - runs: |
        cat > pyproject.toml <<'SOURCE'
        [build-system]
        requires = ["setuptools>=61"]
        build-backend = "setuptools.build_meta"

        [project]
        name = "{name}"
        version = "0.1.0"
        description = "{name}"
        SOURCE
        mkdir -p {module}
        cat > {module}/__init__.py <<'SOURCE'
        def main():
            return 0
        SOURCE
        cat > LICENSE <<'SOURCE'
        MIT License

        Copyright (c) {name} contributors

        Permission is hereby granted, free of charge, to any person obtaining a copy
        of this software and associated documentation files (the "Software"), to deal
        in the Software without restriction.
        SOURCE

outputs:
  - name: {name}
    arch: noarch
    summary: {name}
    files:
      include:
        - /usr/lib/python3*/site-packages/{module}/**
        - /usr/lib/python3*/site-packages/{module}-*.dist-info/**
        - /usr/share/licenses/{name}/LICENSE
      license:
        - /usr/share/licenses/{name}/LICENSE

tests:
  - name: import
    stage: installed
    argv: ["python3", "-c", "import {module}"]
"#;

pub fn run(args: InitArgs) -> Result<(), Error> {
    let directory = args
        .directory
        .clone()
        .unwrap_or_else(|| Path::new(".").to_path_buf());
    std::fs::create_dir_all(&directory).map_err(|error| {
        Error::new(
            codes::IO_001,
            format!("cannot create {}: {error}", directory.display()),
        )
    })?;
    let name = directory
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty() && *name != ".")
        .unwrap_or("example")
        .to_string();
    let module = name.replace('-', "_");
    let (recipe, needs_profile) = match args.template.as_str() {
        "c" => (C_RECIPE, true),
        "metapackage" => (METAPACKAGE_RECIPE, true),
        "python" => (PYTHON_RECIPE, true),
        other => {
            return Err(Error::new(
                codes::IO_001,
                format!("unknown template '{other}'; expected c, python or metapackage"),
            ))
        }
    };
    let recipe = recipe
        .replace("fedora-44", &args.profile)
        .replace("{name}", &name)
        .replace("{module}", &module);
    if args.split {
        write_split(&directory, &recipe, args.force)?;
    } else {
        write(&directory.join("package.yaml"), &recipe, args.force)?;
    }
    if needs_profile {
        let profile = directory
            .join("profiles")
            .join(format!("{}.yaml", args.profile));
        write(
            &profile,
            &PROFILE.replace("fedora-44", &args.profile),
            args.force,
        )?;
    }
    write(
        &directory.join("README.md"),
        &README.replace("{name}", &name),
        args.force,
    )?;
    write(&directory.join(".gitignore"), GITIGNORE, args.force)?;
    std::fs::create_dir_all(directory.join("pipelines"))
        .map_err(|error| Error::new(codes::IO_001, format!("cannot create pipelines: {error}")))?;
    println!("{}", directory.join("package.yaml").display());
    Ok(())
}

/// Split a generated single-file recipe into one file per top-level section.
fn write_split(directory: &Path, recipe: &str, force: bool) -> Result<(), Error> {
    let parsed = rpmx_core::parse_recipe(recipe)?;
    let mut include = Vec::new();
    let package = strip_nulls(serde_json::to_value(&parsed.package).map_err(json)?);
    if !parsed.vars.is_empty() {
        write_section(directory, "vars", &parsed.vars, &mut include, force)?;
    }
    if !parsed.build.pipeline.is_empty()
        || !parsed.build.packages.is_empty()
        || parsed.build.root.is_some()
    {
        write_section(directory, "build", &parsed.build, &mut include, force)?;
    }
    if !parsed.sources.is_empty() {
        write_section(directory, "sources", &parsed.sources, &mut include, force)?;
    }
    if !parsed.patches.is_empty() {
        write_section(directory, "patches", &parsed.patches, &mut include, force)?;
    }
    if !parsed.source_assembly.pipeline.is_empty() || !parsed.source_assembly.tools.is_empty() {
        write_section(
            directory,
            "source-assembly",
            &parsed.source_assembly,
            &mut include,
            force,
        )?;
    }
    if let Some(matrix) = &parsed.matrix {
        write_section(directory, "matrix", matrix, &mut include, force)?;
    }
    if !parsed.outputs.is_empty() {
        write_section(directory, "outputs", &parsed.outputs, &mut include, force)?;
    }
    if !parsed.tests.is_empty() {
        write_section(directory, "tests", &parsed.tests, &mut include, force)?;
    }
    let mut text = format!("schema: {}\ninclude:\n", parsed.schema);
    for name in &include {
        text.push_str(&format!("  - {name}.yaml\n"));
    }
    text.push_str("\npackage:\n");
    let package = serde_yaml_ng::to_string(&package).map_err(yaml)?;
    for line in package.lines() {
        text.push_str("  ");
        text.push_str(line);
        text.push('\n');
    }
    write(&directory.join("package.yaml"), &text, force)
}

fn write_section(
    directory: &Path,
    key: &str,
    value: &impl serde::Serialize,
    include: &mut Vec<String>,
    force: bool,
) -> Result<(), Error> {
    let mut map = serde_json::Map::new();
    map.insert(
        key.to_string(),
        strip_nulls(serde_json::to_value(value).map_err(json)?),
    );
    let text = serde_yaml_ng::to_string(&map).map_err(yaml)?;
    write(&directory.join(format!("{key}.yaml")), &text, force)?;
    include.push(key.to_string());
    Ok(())
}

fn yaml(error: serde_yaml_ng::Error) -> Error {
    Error::new(codes::IO_001, format!("cannot write split recipe: {error}"))
}

fn json(error: serde_json::Error) -> Error {
    Error::new(codes::IO_001, format!("cannot write split recipe: {error}"))
}

/// Generated fragments should not carry `field: null` noise for absent values.
fn strip_nulls(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.into_iter()
                .filter(|(_, value)| !value.is_null())
                .map(|(key, value)| (key, strip_nulls(value)))
                .collect(),
        ),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(strip_nulls).collect())
        }
        other => other,
    }
}

fn write(path: &Path, text: &str, force: bool) -> Result<(), Error> {
    if path.exists() && !force {
        return Err(Error::new(
            codes::INIT_001,
            format!(
                "{} already exists; pass --force to overwrite it",
                path.display()
            ),
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            Error::new(
                codes::IO_001,
                format!("cannot create {}: {error}", parent.display()),
            )
        })?;
    }
    std::fs::write(path, text).map_err(|error| {
        Error::new(
            codes::IO_001,
            format!("cannot write {}: {error}", path.display()),
        )
    })
}
