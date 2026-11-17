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
use pgp::types::{KeyDetails, PlainSecretParams, SecretParams};
use rpm::signature::pgp::Signer;
use rpm::signature::Signing;
use rpm::Timestamp;
use rpmx_core::error::codes;
use rpmx_core::Error;
use rsa::traits::PublicKeyParts;
use zeroize::Zeroizing;

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
        .build()
        .map_err(|error| {
            Error::new(
                codes::SIGN_003,
                format!("cannot build key parameters: {error}"),
            )
        })?;

    let mut secret = params.generate(rand::thread_rng()).map_err(|error| {
        Error::new(
            codes::SIGN_003,
            format!("cannot generate signing key: {error}"),
        )
    })?;
    secret.primary_key = canonical_rsa_primary(&secret.primary_key)?;
    if let Some(passphrase) = options.passphrase {
        secret
            .primary_key
            .set_password(rand::thread_rng(), &passphrase.into())
            .map_err(|error| {
                Error::new(
                    codes::SIGN_003,
                    format!("cannot protect signing key: {error}"),
                )
            })?;
    }
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

/// RFC 9580 section 5.5.5.1 requires p < q. rPGP preserves the RSA generator's
/// prime order, which can make GnuPG's CRT signer produce invalid signatures.
/// Rebuild only the private representation; the public packet and self-signature stay intact.
fn canonical_rsa_primary(key: &pgp::packet::SecretKey) -> Result<pgp::packet::SecretKey, Error> {
    let SecretParams::Plain(PlainSecretParams::RSA(secret)) = key.secret_params() else {
        return Err(Error::new(
            codes::SIGN_003,
            "generated key is not an unencrypted RSA primary",
        ));
    };
    let material = Zeroizing::new(secret.to_bytes());
    let mut primes = vec![
        rsa::BigUint::from_bytes_be(&material.1),
        rsa::BigUint::from_bytes_be(&material.2),
    ];
    primes.sort();
    let public: pgp::types::RsaPublicParams = secret.into();
    let normalized = rsa::RsaPrivateKey::from_components(
        public.key.n().clone(),
        public.key.e().clone(),
        rsa::BigUint::from_bytes_be(&material.0),
        primes,
    )
    .map_err(|error| {
        Error::new(
            codes::SIGN_003,
            format!("cannot order RSA secret primes: {error}"),
        )
    })?;
    // rPGP computes OpenPGP u = p^-1 mod q from this ordered private key.
    pgp::packet::SecretKey::new(
        key.public_key().clone(),
        SecretParams::Plain(PlainSecretParams::RSA(normalized.into())),
    )
    .map_err(|error| {
        Error::new(
            codes::SIGN_003,
            format!("cannot encode RSA secret key: {error}"),
        )
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

/// Choose the signature creation time. Signed builds must be reproducible, so
/// an explicit timestamp or a non-zero source date epoch is required.
pub fn signature_timestamp(
    explicit: Option<u32>,
    source_date_epoch: u32,
    signing: Option<&SigningKey>,
) -> Result<Option<u32>, Error> {
    let Some(key) = signing else {
        return Ok(None);
    };
    let created_at = key.created_at();
    let timestamp = match explicit {
        Some(seconds) => seconds,
        None if source_date_epoch == 0 => {
            return Err(Error::new(
                codes::SIGN_004,
                "a signed build needs a deterministic timestamp; \
                 set package.source-date-epoch, pass --source-date-epoch, or pass --signing-timestamp",
            ));
        }
        // A key cannot vouch for a time before it existed. Clamp to its
        // creation time so the result stays deterministic and verifiable.
        None => source_date_epoch.max(created_at),
    };
    if timestamp < created_at {
        return Err(Error::new(
            codes::SIGN_004,
            format!(
                "signature timestamp {timestamp} is before signing key {} was created at {created_at}",
                key.fingerprint()
            ),
        ));
    }
    Ok(Some(timestamp))
}

/// Load the optional signing key selected on the command line.
pub fn load_signing_key(
    path: Option<&Path>,
    passphrase_env: Option<&str>,
) -> Result<Option<SigningKey>, Error> {
    let Some(path) = path else {
        return Ok(None);
    };
    let passphrase = signing_passphrase(passphrase_env)?;
    Ok(Some(load_key(path, passphrase.as_deref())?))
}

/// Resolve the passphrase for a signing key from an environment variable.
/// The passphrase is never accepted as a command-line argument.
pub fn signing_passphrase(variable: Option<&str>) -> Result<Option<String>, Error> {
    let Some(variable) = variable else {
        return Ok(None);
    };
    std::env::var(variable).map(Some).map_err(|_| {
        Error::new(
            codes::SIGN_005,
            format!("signing passphrase environment variable {variable} is not set"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_canonical_rsa(key: &pgp::packet::SecretKey) {
        let SecretParams::Plain(PlainSecretParams::RSA(secret)) = key.secret_params() else {
            panic!("expected unencrypted RSA material");
        };
        let (_, p, q, u) = secret.to_bytes();
        let p = rsa::BigUint::from_bytes_be(&p);
        let q = rsa::BigUint::from_bytes_be(&q);
        let u = rsa::BigUint::from_bytes_be(&u);
        let public: pgp::types::RsaPublicParams = secret.into();
        assert!(p < q, "OpenPGP requires p < q");
        assert_eq!(&p * &q, *public.key.n());
        assert_eq!((p * u) % q, rsa::BigUint::from(1u8));
    }

    #[test]
    fn reversed_rsa_primes_are_ordered_without_changing_the_certificate() {
        use rand::SeedableRng;
        let params = SecretKeyParamsBuilder::default()
            .key_type(KeyType::Rsa(2048))
            .can_certify(true)
            .can_sign(true)
            .primary_user_id("deterministic test <test@example.invalid>".into())
            .build()
            .expect("parameters");
        let mut key = params
            .generate(rand::rngs::StdRng::seed_from_u64(42))
            .expect("key");
        let SecretParams::Plain(PlainSecretParams::RSA(secret)) = key.primary_key.secret_params()
        else {
            panic!("RSA key");
        };
        let (d, p, q, _) = secret.to_bytes();
        let mut primes = vec![
            rsa::BigUint::from_bytes_be(&p),
            rsa::BigUint::from_bytes_be(&q),
        ];
        primes.sort_by(|a, b| b.cmp(a));
        assert!(
            primes[0] > primes[1],
            "force the formerly broken prime order"
        );
        let public: pgp::types::RsaPublicParams = secret.into();
        let reversed = rsa::RsaPrivateKey::from_components(
            public.key.n().clone(),
            public.key.e().clone(),
            rsa::BigUint::from_bytes_be(&d),
            primes,
        )
        .expect("valid reversed-prime RSA");
        key.primary_key = pgp::packet::SecretKey::new(
            key.primary_key.public_key().clone(),
            SecretParams::Plain(PlainSecretParams::RSA(reversed.into())),
        )
        .expect("reversed packet");
        let fingerprint = key.fingerprint();
        let public_before = SignedPublicKey::from(key.clone());
        key.primary_key = canonical_rsa_primary(&key.primary_key).expect("canonical RSA");
        assert_canonical_rsa(&key.primary_key);
        assert_eq!(key.fingerprint(), fingerprint);
        assert_eq!(SignedPublicKey::from(key.clone()), public_before);
        key.verify_bindings()
            .expect("original self-signature remains valid");
        let normalized = key.primary_key.clone();
        key.primary_key = canonical_rsa_primary(&key.primary_key).expect("idempotent");
        assert_eq!(key.primary_key, normalized);
    }

    #[test]
    fn generated_keys_are_canonical_before_optional_passphrase_protection() {
        for passphrase in [None, Some("test-only passphrase")] {
            let directory = tempfile::tempdir().expect("tempdir");
            let path = directory.path().join("test.asc");
            let key = generate_key(KeygenOptions {
                user_id: "rpmx test <rpmx@example.invalid>",
                bits: 2048,
                passphrase,
            })
            .expect("generate");
            let (mut parsed, _) = SignedSecretKey::from_string(&key.secret_armored).expect("parse");
            assert_eq!(
                matches!(
                    parsed.primary_key.secret_params(),
                    SecretParams::Encrypted(_)
                ),
                passphrase.is_some()
            );
            if let Some(passphrase) = passphrase {
                assert!(parsed
                    .primary_key
                    .remove_password(&"wrong password".into())
                    .is_err());
                parsed
                    .primary_key
                    .remove_password(&passphrase.into())
                    .expect("decrypt");
            }
            assert_canonical_rsa(&parsed.primary_key);
            parsed.verify_bindings().expect("self-signature");
            std::fs::write(&path, &key.secret_armored).expect("write key");
            let loaded = load_key(&path, passphrase).expect("load key");
            assert_eq!(loaded.fingerprint(), key.fingerprint);
            loaded
                .sign_detached(b"canonical RSA signing control", None)
                .expect("sign");
            assert!(key.public_armored.contains("BEGIN PGP PUBLIC KEY BLOCK"));
        }
    }
}
