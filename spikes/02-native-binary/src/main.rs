use rpm::{BuildConfig, FileMode, FileOptions, PackageBuilder};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "rpmx-demo-bin-1.0.0-1.x86_64.rpm".into());

    let mut builder = PackageBuilder::new(
        "rpmx-demo-bin",
        "1.0.0",
        "MIT",
        "x86_64",
        "A minimal native rpmx binary package",
    );

    builder
        .release("1")
        .using_config(BuildConfig::v4().source_date(1_700_000_000))
        .with_file_contents(
            "#!/bin/sh\necho rpmx-demo-bin 1.0.0\n",
            FileOptions::new("/usr/bin/rpmx-demo-bin").mode(FileMode::regular(0o755)),
        )?
        .with_file_contents(
            "rpmx-demo-bin\n",
            FileOptions::new("/usr/share/doc/rpmx-demo-bin/README").mode(FileMode::regular(0o644)),
        )?;

    let pkg = builder.build()?;
    pkg.write_file(&out)?;
    println!("wrote {out}");
    Ok(())
}
