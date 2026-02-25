use std::path::Path;
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand::rngs::OsRng;
use tracing::info;

use crate::errors::AppError;

/// Device cryptographic identity using Ed25519.
/// The keypair is generated once and persisted to disk.
/// This gives each edge device a unique, stable identity.
pub struct DeviceIdentity {
    signing_key: SigningKey,
}

impl DeviceIdentity {
    /// Load existing keypair or generate a new one.
    pub fn load_or_generate(key_path: &str) -> Result<Self, AppError> {
        if Path::new(key_path).exists() {
            Self::load(key_path)
        } else {
            Self::generate(key_path)
        }
    }

    fn generate(key_path: &str) -> Result<Self, AppError> {
        let signing_key = SigningKey::generate(&mut OsRng);
        let bytes = signing_key.to_bytes();

        // Ensure parent directory exists
        if let Some(parent) = Path::new(key_path).parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(key_path, &bytes)?;

        // Set restrictive permissions (owner read-only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(key_path, std::fs::Permissions::from_mode(0o600))?;
        }

        info!(key_path = %key_path, "Generated new device keypair");
        Ok(Self { signing_key })
    }

    fn load(key_path: &str) -> Result<Self, AppError> {
        let bytes = std::fs::read(key_path)?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| AppError::SecurityError("Invalid key file length".into()))?;
        let signing_key = SigningKey::from_bytes(&arr);
        info!(key_path = %key_path, "Loaded device keypair");
        Ok(Self { signing_key })
    }

    /// Return the public key as a hex string (safe to expose in API responses)
    pub fn public_key_hex(&self) -> String {
        let vk: VerifyingKey = (&self.signing_key).into();
        hex::encode(vk.as_bytes())
    }

    /// Sign arbitrary bytes (e.g., for plugin signature verification)
    #[allow(dead_code)]
    pub fn sign(&self, data: &[u8]) -> Vec<u8> {
        use ed25519_dalek::Signer;
        self.signing_key.sign(data).to_bytes().to_vec()
    }

    /// Verify a plugin signature against this device's public key
    #[allow(dead_code)]
    pub fn verify_plugin_signature(&self, binary: &[u8], signature: &[u8]) -> Result<(), AppError> {
        use ed25519_dalek::Verifier;
        let vk: VerifyingKey = (&self.signing_key).into();
        let sig_bytes: [u8; 64] = signature
            .try_into()
            .map_err(|_| AppError::SecurityError("Invalid signature length".into()))?;
        let sig = ed25519_dalek::Signature::from_bytes(&sig_bytes);
        vk.verify(binary, &sig)
            .map_err(|e| AppError::SecurityError(format!("Plugin signature invalid: {}", e)))
    }
}
