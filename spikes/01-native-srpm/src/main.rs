use rpm::{BuildConfig, FileOptions, PackageBuilder};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::env::args().nth(1).unwrap_or_else(|| "demo.src.rpm".into());

    let mut builder = PackageBuilder::new(
        "rpmx-demo",
        "1.0.0",
        "MIT",
        "src",
        "A minimal native rpmx source package",
    );
    builder
        .release("1")
        .using_config(BuildConfig::v4().source_date(1_700_000_000))
        .with_file_contents(
            "schema: 1\npackage:\n  name: rpmx-demo\n  version: \"1.0.0\"\n",
            FileOptions::new("./package.yaml").mode(rpm::FileMode::regular(0o644)),
        )?
        .with_file_contents(
            "{\"schema\": 1}\n",
            FileOptions::new("./package.lock.json").mode(rpm::FileMode::regular(0o644)),
        )?
        .with_file_contents(
            "{\"schema\": 1, \"note\": \"manifest\"}\n",
            FileOptions::new("./RPMX-MANIFEST.json").mode(rpm::FileMode::regular(0o644)),
        )?;

    let pkg = builder.build()?;
    pkg.write_file(&out)?;
    println!("wrote {out}");
    Ok(())
}
