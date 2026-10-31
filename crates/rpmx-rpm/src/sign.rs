//! Optional OpenPGP signing for RPM packages.
//!
//! rpmx never generates or stores signing keys as part of a build. This module
//! exists so a caller can supply a key explicitly, either with `rpmx sign` or
//! by passing `--signing-key` to a build command. When no key is supplied the
//! package is written unsigned and this module is not used.

use std::path::Path;

use pgp::composed::{
    Deserializable, DetachedSignature, KeyType, SecretKeyParamsBuilder, SignedPublicKey,
    SignedSecretKey,
};
use pgp::types::KeyDetails;
use rpm::signature::pgp::Signer;
use rpm::signature::Signing;
use rpm::Timestamp;
use rpmx_core::error::codes;
use rpmx_core::Error;

/// A secret key loaded for signing, with the identity reported to callers.
#[derive(Clone)]
pub struct SigningKey {
    signer: Signer,
    fingerprint: String,
    created_at: u32,
}

impl SigningKey {
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Unix seconds when the signing key was created. A signature older than
    /// its key is rejected by rpm, so callers must not go below this bound.
    pub fn created_at(&self) -> u32 {
        self.created_at
    }

    /// Produce an ASCII-armored detached OpenPGP signature over arbitrary bytes,
    /// as used by `repomd.xml.asc`.
    pub fn sign_detached(&self, data: &[u8], timestamp: Option<u32>) -> Result<String, Error> {
        let seconds = timestamp.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_secs() as u32)
                .unwrap_or(self.created_at)
        });
        if seconds < self.created_at {
            return Err(Error::new(
                codes::SIGN_004,
                format!(
                    "signature timestamp {seconds} is before signing key {} was created at {}",
                    self.fingerprint, self.created_at
                ),
            ));
        }
        let raw = self
            .signer
            .sign(data, Timestamp(seconds))
            .map_err(|error| Error::new(codes::SIGN_002, format!("cannot sign data: {error}")))?;
        let signature = DetachedSignature::from_bytes(std::io::Cursor::new(raw.as_slice()))
            .map_err(|error| {
                Error::new(
                    codes::SIGN_002,
                    format!("cannot parse generated signature: {error}"),
                )
            })?;
        signature.to_armored_string(None.into()).map_err(|error| {
            Error::new(
                codes::SIGN_002,
                format!("cannot armor generated signature: {error}"),
            )
        })
    }
}

pub struct KeygenOptions<'a> {
    pub user_id: &'a str,
    pub bits: u32,
    pub passphrase: Option<&'a str>,
}

pub struct GeneratedKey {
    pub secret_armored: String,
    pub public_armored: String,
    pub fingerprint: String,
}

/// Generate an RSA OpenPGP key in the same shape `gpg --gen-key` produces:
/// a signing-capable primary key with one user ID.
pub fn generate_key(options: KeygenOptions<'_>) -> Result<GeneratedKey, Error> {
    if options.bits < 2048 {
        return Err(Error::new(
            codes::SIGN_004,
            format!(
                "RSA key size {} is too small; use at least 2048 bits",
                options.bits
            ),
        ));
    }
    if options.user_id.trim().is_empty() {
        return Err(Error::new(
            codes::SIGN_004,
            "the signing key needs a non-empty user ID",
        ));
    }

    let params = SecretKeyParamsBuilder::default()
        .key_type(KeyType::Rsa(options.bits))
        .can_certify(true)
        .can_sign(true)
        .primary_user_id(options.user_id.to_string())
        .passphrase(options.passphrase.map(str::to_string))
        .build()
        .map_err(|error| {
            Error::new(
                codes::SIGN_003,
                format!("cannot build key parameters: {error}"),
            )
        })?;

    let secret = params.generate(rand::thread_rng()).map_err(|error| {
        Error::new(
            codes::SIGN_003,
            format!("cannot generate signing key: {error}"),
        )
    })?;
    let public = SignedPublicKey::from(secret.clone());
    let secret_armored = secret.to_armored_string(None.into()).map_err(|error| {
        Error::new(
            codes::SIGN_003,
            format!("cannot serialize the secret key: {error}"),
        )
    })?;
    let public_armored = public.to_armored_string(None.into()).map_err(|error| {
        Error::new(
            codes::SIGN_003,
            format!("cannot serialize the public key: {error}"),
        )
    })?;

    Ok(GeneratedKey {
        secret_armored,
        public_armored,
        fingerprint: format!("{:X}", secret.fingerprint()),
    })
}

/// Load an ASCII-armored OpenPGP secret key. The passphrase decrypts the key
/// material; an empty passphrase means the key is unprotected.
pub fn load_key(path: &Path, passphrase: Option<&str>) -> Result<SigningKey, Error> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        Error::new(
            codes::SIGN_001,
            format!("cannot read signing key {}: {error}", path.display()),
        )
    })?;
    let secret = load_single_certificate(path, &text)?;
    let (fingerprint, created_at) = signing_identity(&secret);

    let mut signer = Signer::from_asc(&text).map_err(|error| {
        Error::new(
            codes::SIGN_001,
            format!("cannot load signing key {}: {error}", path.display()),
        )
    })?;
    if let Some(passphrase) = passphrase {
        signer = signer.with_key_passphrase(passphrase);
    }

    Ok(SigningKey {
        signer,
        fingerprint,
        created_at,
    })
}

/// Sign an RPM on disk in place. `timestamp` is the signature creation time;
/// `None` uses the current time.
pub fn sign_package(path: &Path, key: &SigningKey, timestamp: Option<u32>) -> Result<(), Error> {
    let metadata = rpm::PackageMetadata::open(path).map_err(|error| {
        Error::new(
            codes::SIGN_002,
            format!("cannot read {} for signing: {error}", path.display()),
        )
    })?;
    let header = metadata.header_bytes().map_err(|error| {
        Error::new(
            codes::SIGN_002,
            format!("cannot serialize the header of {}: {error}", path.display()),
        )
    })?;
    if let Some(seconds) = timestamp {
        if seconds < key.created_at {
            // rpm rejects a signature that predates its key, so refuse to make
            // one instead of writing a package that cannot be verified.
            return Err(Error::new(
                codes::SIGN_004,
                format!(
                    "signature timestamp {seconds} is before signing key {} was created at {}",
                    key.fingerprint, key.created_at
                ),
            ));
        }
    }
    let timestamp = timestamp.map_or_else(Timestamp::now, Timestamp);
    let signature = key
        .signer
        .sign(header.as_slice(), timestamp)
        .map_err(|error| {
            Error::new(
                codes::SIGN_002,
                format!("cannot sign {}: {error}", path.display()),
            )
        })?;
    rpm::Package::apply_signature_in_place(path, signature).map_err(|error| {
        Error::new(
            codes::SIGN_002,
            format!("cannot write the signature to {}: {error}", path.display()),
        )
    })
}

/// Parse exactly one OpenPGP certificate. Accepting a keyring would make the
/// reported fingerprint ambiguous, because the signer chooses the certificate.
fn load_single_certificate(path: &Path, text: &str) -> Result<SignedSecretKey, Error> {
    let (mut certificates, _headers) =
        SignedSecretKey::from_string_many(text).map_err(|error| {
            Error::new(
                codes::SIGN_001,
                format!(
                    "cannot parse signing key {}; expected an ASCII-armored OpenPGP secret key: {error}",
                    path.display()
                ),
            )
        })?;

    let first = certificates.next().ok_or_else(|| {
        Error::new(
            codes::SIGN_001,
            format!(
                "signing key {} contains no OpenPGP certificate",
                path.display()
            ),
        )
    })?;
    if certificates.next().is_some() {
        return Err(Error::new(
            codes::SIGN_001,
            format!(
                "signing key {} contains more than one OpenPGP certificate; use a file with exactly one",
                path.display()
            ),
        ));
    }
    first.map_err(|error| {
        Error::new(
            codes::SIGN_001,
            format!("cannot parse signing key {}: {error}", path.display()),
        )
    })
}

/// Report the identity and creation time of the key the signer will actually
/// use: the first signing-capable subkey, or the primary key when there is none.
fn signing_identity(key: &SignedSecretKey) -> (String, u32) {
    let signing_key = key
        .secret_subkeys
        .iter()
        .find(|subkey| subkey.signatures.iter().any(|sig| sig.key_flags().sign()))
        .map(|subkey| &subkey.key);
    match signing_key {
        Some(subkey) => (
            format!("{:X}", subkey.fingerprint()),
            subkey.created_at().as_secs(),
        ),
        None => (
            format!("{:X}", key.primary_key.fingerprint()),
            key.primary_key.created_at().as_secs(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_key_roundtrips_through_the_loader() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("test.asc");
        let key = generate_key(KeygenOptions {
            user_id: "rpmx test <rpmx@example.invalid>",
            bits: 2048,
            passphrase: None,
        })
        .expect("generate");
        std::fs::write(&path, &key.secret_armored).expect("write key");

        let loaded = load_key(&path, None).expect("load key");
        assert_eq!(loaded.fingerprint(), key.fingerprint);
        assert!(key.public_armored.contains("BEGIN PGP PUBLIC KEY BLOCK"));
    }
}
