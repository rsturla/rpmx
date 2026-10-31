use std::path::Path;

use rpmx_core::error::codes;
use rpmx_core::Error;
use rpmx_rpm::sign::{generate_key, load_key, sign_package, KeygenOptions};

use crate::commands::signing_passphrase;

pub struct KeygenArgs<'a> {
    pub output: &'a Path,
    pub user_id: &'a str,
    pub bits: u32,
    pub passphrase_env: Option<&'a str>,
    pub json: bool,
}

pub struct SignArgs<'a> {
    pub signing_key: &'a Path,
    pub passphrase_env: Option<&'a str>,
    pub timestamp: Option<u32>,
    pub rpms: &'a [std::path::PathBuf],
    pub json: bool,
}

pub fn keygen(args: KeygenArgs<'_>) -> Result<(), Error> {
    let passphrase = signing_passphrase(args.passphrase_env)?;
    let key = generate_key(KeygenOptions {
        user_id: args.user_id,
        bits: args.bits,
        passphrase: passphrase.as_deref(),
    })?;

    let public = public_path(args.output);
    write_secret(args.output, &key.secret_armored)?;
    std::fs::write(&public, &key.public_armored).map_err(|error| {
        Error::new(
            codes::SIGN_006,
            format!("cannot write {}: {error}", public.display()),
        )
    })?;

    if args.json {
        let value = serde_json::json!({
            "ok": true,
            "fingerprint": key.fingerprint,
            "secret_key": args.output.display().to_string(),
            "public_key": public.display().to_string(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&value)
                .map_err(|error| Error::new(codes::CLI_003, error.to_string()))?
        );
    } else {
        println!("{}", key.fingerprint);
        println!("{}", args.output.display());
        println!("{}", public.display());
    }
    Ok(())
}

pub fn run(args: SignArgs<'_>) -> Result<(), Error> {
    let passphrase = signing_passphrase(args.passphrase_env)?;
    let key = load_key(args.signing_key, passphrase.as_deref())?;

    let mut signed = Vec::new();
    for path in args.rpms {
        sign_package(path, &key, args.timestamp)?;
        signed.push(path.display().to_string());
    }

    if args.json {
        let value = serde_json::json!({
            "ok": true,
            "fingerprint": key.fingerprint(),
            "signed": signed,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&value)
                .map_err(|error| Error::new(codes::CLI_003, error.to_string()))?
        );
    } else {
        for path in &signed {
            println!("{path}");
        }
    }
    Ok(())
}

fn public_path(secret: &Path) -> std::path::PathBuf {
    let mut name = secret.as_os_str().to_owned();
    name.push(".pub");
    std::path::PathBuf::from(name)
}

fn write_secret(path: &Path, text: &str) -> Result<(), Error> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| {
            Error::new(
                codes::SIGN_006,
                format!("cannot write {}: {error}", path.display()),
            )
        })?;
    file.write_all(text.as_bytes()).map_err(|error| {
        Error::new(
            codes::SIGN_006,
            format!("cannot write {}: {error}", path.display()),
        )
    })?;
    // `.mode()` only applies when the file is created; tighten an existing file too.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|error| {
        Error::new(
            codes::SIGN_006,
            format!("cannot set permissions on {}: {error}", path.display()),
        )
    })
}
