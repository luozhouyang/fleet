//! Ed25519 machine identities and signature verification.

use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{AuthError, Result};

/// Current on-disk key file format version.
const KEY_FILE_VERSION: u32 = 1;
/// Algorithm name stored in the key file.
const KEY_FILE_ALGORITHM: &str = "ed25519";
/// Length of an Ed25519 private key seed in bytes.
const PRIVATE_KEY_LEN: usize = 32;
/// Length of an Ed25519 public key in bytes.
const PUBLIC_KEY_LEN: usize = 32;
/// Length of an Ed25519 signature in bytes.
const SIGNATURE_LEN: usize = 64;

/// Ed25519 key pair identifying a machine.
///
/// The private key never leaves the machine. Persist it with [`Self::save`],
/// which creates parent directories and restricts the file to mode `0600` on
/// Unix. The debug implementation intentionally prints only public material.
#[derive(Clone)]
pub struct MachineIdentity {
    signing_key: SigningKey,
}

impl fmt::Debug for MachineIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MachineIdentity")
            .field("public_key", &self.public_key_base64())
            .field("fingerprint", &self.fingerprint())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct KeyFile {
    version: u32,
    algorithm: String,
    public_key: String,
    private_key: String,
}

impl MachineIdentity {
    /// Generate a fresh identity using the operating system CSPRNG.
    pub fn generate() -> Self {
        Self {
            signing_key: SigningKey::generate(&mut OsRng),
        }
    }

    /// Load an identity, generating and saving a new one if `path` does not exist.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if path.exists() {
            Self::load(path)
        } else {
            let identity = Self::generate();
            identity.save(path)?;
            Ok(identity)
        }
    }

    /// Load an identity from a JSON key file.
    ///
    /// On Unix the file must not be accessible by group or other users; a mode
    /// with any of `0o077` set is rejected with
    /// [`AuthError::InsecurePermissions`].
    pub fn load(path: &Path) -> Result<Self> {
        #[cfg(unix)]
        check_permissions(path)?;

        let bytes = fs::read(path).map_err(|err| AuthError::io(path, err))?;
        let file: KeyFile = serde_json::from_slice(&bytes).map_err(|err| {
            AuthError::InvalidKeyFile(format!("{} is not a valid key file: {err}", path.display()))
        })?;
        Self::from_key_file(&file, path)
    }

    /// Save the identity as a JSON key file.
    ///
    /// Parent directories are created if needed. On Unix the file is created
    /// with mode `0600` and an existing file's mode is tightened to `0600`.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|err| AuthError::io(parent, err))?;
            }
        }

        let file = KeyFile {
            version: KEY_FILE_VERSION,
            algorithm: KEY_FILE_ALGORITHM.to_string(),
            public_key: self.public_key_base64(),
            private_key: STANDARD.encode(self.signing_key.to_bytes()),
        };
        let json = serde_json::to_vec_pretty(&file)?;

        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }

        let mut handle = options.open(path).map_err(|err| AuthError::io(path, err))?;
        handle
            .write_all(&json)
            .map_err(|err| AuthError::io(path, err))?;
        handle.flush().map_err(|err| AuthError::io(path, err))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = fs::Permissions::from_mode(0o600);
            fs::set_permissions(path, permissions).map_err(|err| AuthError::io(path, err))?;
        }

        Ok(())
    }

    /// Rotate the key file: keep the old identity as `<path>.previous`, save a
    /// freshly generated identity to `path`, and return the previous identity.
    ///
    /// The previous key is needed once to prove ownership during the next
    /// connection handshake. Call [`Self::clear_previous`] after the control
    /// plane accepts the rotation.
    pub fn rotate_file(path: &Path) -> Result<Self> {
        let previous = Self::load(path)?;
        let next = Self::generate();
        previous.save(&previous_path(path))?;
        next.save(path)?;
        Ok(previous)
    }

    /// Load the previous identity left by [`Self::rotate_file`], if present.
    pub fn load_previous(path: &Path) -> Result<Option<Self>> {
        let previous = previous_path(path);
        if previous.exists() {
            Self::load(&previous).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Remove the previous key file after a successful rotation handshake.
    pub fn clear_previous(path: &Path) -> Result<()> {
        let previous = previous_path(path);
        match fs::remove_file(&previous) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(AuthError::io(&previous, err)),
        }
    }

    /// Base64 (standard alphabet) encoding of the Ed25519 public key bytes.
    pub fn public_key_base64(&self) -> String {
        STANDARD.encode(self.signing_key.verifying_key().to_bytes())
    }

    /// Sign `data`, returning the base64 encoding of the 64-byte signature.
    pub fn sign(&self, data: &[u8]) -> String {
        STANDARD.encode(self.signing_key.sign(data).to_bytes())
    }

    /// Stable machine fingerprint: `m_` followed by the lowercase hex SHA-256
    /// of the public key bytes.
    pub fn fingerprint(&self) -> String {
        fingerprint_for(&self.public_key_base64())
    }

    fn from_key_file(file: &KeyFile, path: &Path) -> Result<Self> {
        if file.version != KEY_FILE_VERSION {
            return Err(AuthError::InvalidKeyFile(format!(
                "{} uses unsupported key file version {}; expected {}",
                path.display(),
                file.version,
                KEY_FILE_VERSION
            )));
        }
        if !file.algorithm.eq_ignore_ascii_case(KEY_FILE_ALGORITHM) {
            return Err(AuthError::InvalidKeyFile(format!(
                "{} uses unsupported algorithm {:?}; expected {:?}",
                path.display(),
                file.algorithm,
                KEY_FILE_ALGORITHM
            )));
        }

        let private_key: [u8; PRIVATE_KEY_LEN] = decode_fixed(&file.private_key, "private key")?;
        let public_key: [u8; PUBLIC_KEY_LEN] = decode_fixed(&file.public_key, "public key")?;

        let signing_key = SigningKey::from_bytes(&private_key);
        if signing_key.verifying_key().to_bytes() != public_key {
            return Err(AuthError::InvalidKeyFile(format!(
                "{} public key does not match its private key",
                path.display()
            )));
        }

        Ok(Self { signing_key })
    }
}

/// Path of the previous key file used during rotation.
fn previous_path(path: &Path) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(".previous");
    PathBuf::from(value)
}

/// Verify an Ed25519 signature.
///
/// `public_key_b64` and `signature_b64` must be standard base64; decoded key and
/// signature lengths must be exactly 32 and 64 bytes. Verification uses
/// [`VerifyingKey::verify_strict`], which rejects malleable signatures and
/// small-order public keys.
pub fn verify_signature(public_key_b64: &str, data: &[u8], signature_b64: &str) -> Result<()> {
    let public_key: [u8; PUBLIC_KEY_LEN] = decode_fixed(public_key_b64, "public key")?;
    let signature_bytes: [u8; SIGNATURE_LEN] = decode_fixed(signature_b64, "signature")?;

    let verifying_key = VerifyingKey::from_bytes(&public_key)
        .map_err(|err| AuthError::InvalidKey(err.to_string()))?;
    let signature = Signature::from_bytes(&signature_bytes);

    verifying_key
        .verify_strict(data, &signature)
        .map_err(|_| AuthError::InvalidSignature)
}

fn fingerprint_for(public_key_b64: &str) -> String {
    // The key file can only contain a valid key, so the fallback is defensive.
    match STANDARD.decode(public_key_b64) {
        Ok(bytes) => {
            let digest = Sha256::digest(&bytes);
            format!("m_{}", hex::encode(digest))
        }
        Err(_) => {
            let digest = Sha256::digest(public_key_b64.as_bytes());
            format!("m_{}", hex::encode(digest))
        }
    }
}

fn decode_fixed<const N: usize>(value: &str, what: &str) -> Result<[u8; N]> {
    let bytes = STANDARD
        .decode(value)
        .map_err(|err| AuthError::InvalidBase64(format!("{what}: {err}")))?;
    if bytes.len() != N {
        return Err(AuthError::InvalidKey(format!(
            "{what} must be {N} bytes, got {}",
            bytes.len()
        )));
    }
    let mut out = [0u8; N];
    out.copy_from_slice(&bytes);
    Ok(out)
}

#[cfg(unix)]
pub(crate) fn check_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = fs::metadata(path).map_err(|err| AuthError::io(path, err))?;
    let mode = metadata.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(AuthError::InsecurePermissions {
            path: path.to_path_buf(),
            mode,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify_roundtrip() {
        let identity = MachineIdentity::generate();
        let data = b"challenge nonce bytes";
        let signature = identity.sign(data);

        verify_signature(&identity.public_key_base64(), data, &signature)
            .expect("fresh signature must verify");
    }

    #[test]
    fn verify_rejects_tampered_data() {
        let identity = MachineIdentity::generate();
        let signature = identity.sign(b"original");

        let err = verify_signature(&identity.public_key_base64(), b"tampered", &signature)
            .expect_err("tampered data must not verify");
        assert!(matches!(err, AuthError::InvalidSignature));
    }

    #[test]
    fn verify_rejects_tampered_signature() {
        let identity = MachineIdentity::generate();
        let signature = identity.sign(b"original");
        let mut bytes = STANDARD.decode(&signature).expect("valid signature");
        bytes[0] ^= 0x01;
        let tampered = STANDARD.encode(&bytes);

        let err = verify_signature(&identity.public_key_base64(), b"original", &tampered)
            .expect_err("tampered signature must not verify");
        assert!(matches!(err, AuthError::InvalidSignature));
    }

    #[test]
    fn verify_rejects_bad_base64() {
        let identity = MachineIdentity::generate();
        let signature = identity.sign(b"original");

        let err = verify_signature("not base64 !!!", b"original", &signature)
            .expect_err("bad public key encoding must fail");
        assert!(matches!(err, AuthError::InvalidBase64(_)));

        let err = verify_signature(&identity.public_key_base64(), b"original", "%%%")
            .expect_err("bad signature encoding must fail");
        assert!(matches!(err, AuthError::InvalidBase64(_)));
    }

    #[test]
    fn verify_rejects_wrong_lengths() {
        let identity = MachineIdentity::generate();
        let signature = identity.sign(b"original");

        let short_key = STANDARD.encode([0u8; PUBLIC_KEY_LEN - 1]);
        let err = verify_signature(&short_key, b"original", &signature)
            .expect_err("short public key must fail");
        assert!(matches!(err, AuthError::InvalidKey(_)));

        let short_signature = STANDARD.encode([0u8; SIGNATURE_LEN - 1]);
        let err = verify_signature(&identity.public_key_base64(), b"original", &short_signature)
            .expect_err("short signature must fail");
        assert!(matches!(err, AuthError::InvalidKey(_)));
    }

    #[test]
    fn save_and_load_roundtrip_in_nested_directory() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("nested").join("machine.key");
        let identity = MachineIdentity::generate();

        identity.save(&path).expect("save identity");
        assert!(path.exists());

        let loaded = MachineIdentity::load(&path).expect("load identity");
        assert_eq!(loaded.public_key_base64(), identity.public_key_base64());
        assert_eq!(loaded.fingerprint(), identity.fingerprint());

        let data = b"payload";
        verify_signature(&loaded.public_key_base64(), data, &loaded.sign(data))
            .expect("loaded identity signs correctly");
    }

    #[test]
    fn fingerprint_is_stable_and_prefixed() {
        let identity = MachineIdentity::generate();
        let first = identity.fingerprint();
        let second = identity.fingerprint();

        assert_eq!(first, second);
        assert!(first.starts_with("m_"));
        assert_eq!(first.len(), 2 + 64);
        assert!(first[2..].chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(first[2..], first[2..].to_lowercase());

        let loaded = MachineIdentity::generate();
        assert_ne!(first, loaded.fingerprint());
    }

    #[test]
    fn load_or_create_persists_identity() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("machine.key");

        let created = MachineIdentity::load_or_create(&path).expect("create identity");
        assert!(path.exists());

        let reloaded = MachineIdentity::load_or_create(&path).expect("load existing identity");
        assert_eq!(created.public_key_base64(), reloaded.public_key_base64());
    }

    #[test]
    fn load_rejects_mismatched_public_key() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("machine.key");
        let identity = MachineIdentity::generate();
        identity.save(&path).expect("save identity");

        let other = MachineIdentity::generate();
        let mut file: KeyFile =
            serde_json::from_slice(&fs::read(&path).expect("read")).expect("parse");
        file.public_key = other.public_key_base64();
        fs::write(&path, serde_json::to_vec(&file).expect("encode")).expect("write");

        let err = MachineIdentity::load(&path).expect_err("mismatched key pair must fail");
        assert!(matches!(err, AuthError::InvalidKeyFile(_)));
    }

    #[cfg(unix)]
    #[test]
    fn saved_key_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("machine.key");
        MachineIdentity::generate().save(&path).expect("save");

        let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn load_rejects_group_or_world_readable_key_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("machine.key");
        MachineIdentity::generate().save(&path).expect("save");

        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("chmod");
        let err = MachineIdentity::load(&path).expect_err("insecure mode must be rejected");
        assert!(matches!(
            err,
            AuthError::InsecurePermissions { mode: 0o644, .. }
        ));

        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("chmod");
        MachineIdentity::load(&path).expect("owner-only key file loads");
    }

    #[test]
    fn debug_does_not_expose_private_key() {
        let identity = MachineIdentity::generate();
        let rendered = format!("{identity:?}");
        assert!(rendered.contains(&identity.fingerprint()));
        assert!(!rendered.contains(&STANDARD.encode(identity.signing_key.to_bytes())));
    }
}

#[cfg(test)]
mod rotation_tests {
    use super::*;

    #[test]
    fn rotate_file_keeps_a_previous_key_and_swaps_the_current_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("machine.key");
        let original = MachineIdentity::generate();
        original.save(&path).unwrap();
        let original_public = original.public_key_base64();

        let previous = MachineIdentity::rotate_file(&path).unwrap();
        assert_eq!(previous.public_key_base64(), original_public);
        let current = MachineIdentity::load(&path).unwrap();
        assert_ne!(current.public_key_base64(), original_public);

        let stored_previous = MachineIdentity::load_previous(&path).unwrap().unwrap();
        assert_eq!(stored_previous.public_key_base64(), original_public);

        MachineIdentity::clear_previous(&path).unwrap();
        assert!(MachineIdentity::load_previous(&path).unwrap().is_none());
        // Clearing twice is fine.
        MachineIdentity::clear_previous(&path).unwrap();
    }
}
