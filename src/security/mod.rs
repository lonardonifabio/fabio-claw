use std::path::Path;
use ed25519_dalek::{SigningKey, VerifyingKey, Signature, Signer, Verifier};
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

        if let Some(parent) = Path::new(key_path).parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(key_path, &bytes)?;

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

    /// Sign arbitrary bytes — used when registering a plugin's binary hash
    pub fn sign(&self, data: &[u8]) -> Vec<u8> {
        self.signing_key.sign(data).to_bytes().to_vec()
    }

    /// Verify a plugin binary against a stored Ed25519 signature (base64-encoded).
    ///
    /// Workflow:
    ///   1. On first install: `fabio-claw-sign <binary>` produces a `.sig` file containing
    ///      the device-signed SHA-256 hash of the binary.
    ///   2. At runtime, this method reads the `.sig` file, re-hashes the binary,
    ///      and verifies the signature matches the device's public key.
    ///
    /// This ensures no plugin binary can be swapped after signing without detection.
    pub fn verify_plugin_signature(
        &self,
        binary_path: &Path,
        sig_path: &Path,
    ) -> Result<(), AppError> {
        // Read the binary and hash it
        let binary_bytes = std::fs::read(binary_path)
            .map_err(|e| AppError::SecurityError(format!("Cannot read plugin binary: {}", e)))?;

        let binary_hash = sha256(&binary_bytes);

        // Read the base64-encoded signature file
        let sig_b64 = std::fs::read_to_string(sig_path)
            .map_err(|e| AppError::SecurityError(format!("Cannot read .sig file '{}': {}", sig_path.display(), e)))?;

        let sig_bytes = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            sig_b64.trim(),
        ).map_err(|e| AppError::SecurityError(format!("Invalid base64 in .sig file: {}", e)))?;

        let sig_arr: [u8; 64] = sig_bytes
            .try_into()
            .map_err(|_| AppError::SecurityError("Signature must be 64 bytes".into()))?;

        let signature = Signature::from_bytes(&sig_arr);
        let verifying_key: VerifyingKey = (&self.signing_key).into();

        verifying_key
            .verify(&binary_hash, &signature)
            .map_err(|e| AppError::SecurityError(format!(
                "Plugin signature invalid for '{}': {}",
                binary_path.display(), e
            )))?;

        Ok(())
    }

    /// Generate a `.sig` file for a plugin binary (call once after building).
    /// The resulting file should be placed alongside the binary in the plugin dir.
    pub fn sign_plugin(&self, binary_path: &Path, sig_path: &Path) -> Result<(), AppError> {
        let binary_bytes = std::fs::read(binary_path)
            .map_err(|e| AppError::SecurityError(format!("Cannot read binary: {}", e)))?;

        let hash = sha256(&binary_bytes);
        let sig = self.signing_key.sign(&hash);
        let sig_b64 = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            sig.to_bytes(),
        );

        std::fs::write(sig_path, sig_b64)
            .map_err(|e| AppError::SecurityError(format!("Cannot write .sig file: {}", e)))?;

        info!(binary = %binary_path.display(), sig = %sig_path.display(), "Plugin signed");
        Ok(())
    }
}

/// SHA-256 hash using only std — no external crypto crate needed.
fn sha256(data: &[u8]) -> Vec<u8> {
    // We use a simple iterative SHA-256 implementation to avoid adding the sha2 crate.
    // For a production system, add sha2 = "0.10" to dependencies and use that instead.
    use std::num::Wrapping;

    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1,
        0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
        0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
        0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147,
        0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
        0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
        0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];

    let mut h: [Wrapping<u32>; 8] = [
        Wrapping(0x6a09e667), Wrapping(0xbb67ae85), Wrapping(0x3c6ef372), Wrapping(0xa54ff53a),
        Wrapping(0x510e527f), Wrapping(0x9b05688c), Wrapping(0x1f83d9ab), Wrapping(0x5be0cd19),
    ];

    let len = data.len();
    let bit_len = (len as u64) * 8;

    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 { msg.push(0); }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [Wrapping(0u32); 64];
        for i in 0..16 {
            w[i] = Wrapping(u32::from_be_bytes([chunk[i*4], chunk[i*4+1], chunk[i*4+2], chunk[i*4+3]]));
        }
        for i in 16..64 {
            let s0 = w[i-15].0.rotate_right(7) ^ w[i-15].0.rotate_right(18) ^ (w[i-15].0 >> 3);
            let s1 = w[i-2].0.rotate_right(17) ^ w[i-2].0.rotate_right(19) ^ (w[i-2].0 >> 10);
            w[i] = w[i-16] + Wrapping(s0) + w[i-7] + Wrapping(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.0.rotate_right(6) ^ e.0.rotate_right(11) ^ e.0.rotate_right(25);
            let ch = (e.0 & f.0) ^ (!e.0 & g.0);
            let temp1 = hh + Wrapping(s1) + Wrapping(ch) + Wrapping(K[i]) + w[i];
            let s0 = a.0.rotate_right(2) ^ a.0.rotate_right(13) ^ a.0.rotate_right(22);
            let maj = (a.0 & b.0) ^ (a.0 & c.0) ^ (b.0 & c.0);
            let temp2 = Wrapping(s0) + Wrapping(maj);
            hh = g; g = f; f = e; e = d + temp1; d = c; c = b; b = a; a = temp1 + temp2;
        }

        h[0] += a; h[1] += b; h[2] += c; h[3] += d;
        h[4] += e; h[5] += f; h[6] += g; h[7] += hh;
    }

    let mut result = Vec::with_capacity(32);
    for word in &h { result.extend_from_slice(&word.0.to_be_bytes()); }
    result
}
