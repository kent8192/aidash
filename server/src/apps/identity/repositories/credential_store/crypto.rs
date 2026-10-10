//! Domain-separated AEAD for Key Material and the persistent key check.
use crate::{Error, Result};
use aes_gcm::{
	Aes256Gcm, KeyInit, Nonce,
	aead::{Aead, AeadCore, OsRng, Payload},
};
use hkdf::Hkdf;
use secrecy::{ExposeSecret, SecretString};
use sha2::Sha256;
use zeroize::Zeroizing;

pub const ALGORITHM: &str = "aes-256-gcm";
const CHECK: &[u8] = b"aidash-provider-credential-store/key-check/v1";
pub struct Key {
	pub id: String,
	encryption: Zeroizing<[u8; 32]>,
}
impl Key {
	pub fn parse(value: &SecretString) -> Result<Self> {
		let value = value.expose_secret().trim();
		if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
			return Err(Error::Invalid(
				"Provider Credential Store Master Key must be 64 hex characters".into(),
			));
		}
		let mut master = Zeroizing::new([0u8; 32]);
		for (i, byte) in master.iter_mut().enumerate() {
			*byte = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).expect("validated hex");
		}
		let hkdf = Hkdf::<Sha256>::new(
			Some(b"aidash-provider-credential-store/v1"),
			master.as_ref(),
		);
		let mut encryption = Zeroizing::new([0u8; 32]);
		hkdf.expand(b"encryption/aes-256-gcm", encryption.as_mut())
			.expect("fixed HKDF output");
		let mut id = Zeroizing::new([0u8; 16]);
		hkdf.expand(b"key-identifier", id.as_mut())
			.expect("fixed HKDF output");
		Ok(Self {
			id: id.iter().map(|b| format!("{b:02x}")).collect(),
			encryption,
		})
	}
	pub fn seal(&self, plaintext: &[u8], aad: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
		let cipher = Aes256Gcm::new_from_slice(self.encryption.as_ref()).expect("256-bit key");
		let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
		let ciphertext = cipher
			.encrypt(
				&nonce,
				Payload {
					msg: plaintext,
					aad,
				},
			)
			.map_err(|_| Error::Invalid("Provider Credential Store encryption failed".into()))?;
		Ok((nonce.to_vec(), ciphertext))
	}
	pub fn open(&self, nonce: &[u8], ciphertext: &[u8], aad: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
		if nonce.len() != 12 {
			return Err(Error::Invalid("invalid AEAD nonce length".into()));
		}
		let cipher = Aes256Gcm::new_from_slice(self.encryption.as_ref()).expect("256-bit key");
		cipher
			.decrypt(
				Nonce::from_slice(nonce),
				Payload {
					msg: ciphertext,
					aad,
				},
			)
			.map(Zeroizing::new)
			.map_err(|_| Error::Invalid("AEAD authentication failed".into()))
	}
	pub fn check(&self) -> Result<(Vec<u8>, Vec<u8>)> {
		self.seal(CHECK, &check_aad(&self.id))
	}
	pub fn verify(&self, nonce: &[u8], ciphertext: &[u8]) -> Result<()> {
		if self
			.open(nonce, ciphertext, &check_aad(&self.id))?
			.as_slice()
			!= CHECK
		{
			return Err(Error::Invalid(
				"Provider Credential Store key check failed".into(),
			));
		}
		Ok(())
	}
}
fn encode(label: &[u8], fields: &[&[u8]]) -> Vec<u8> {
	let mut result = label.to_vec();
	for field in fields {
		result.extend_from_slice(&(field.len() as u64).to_be_bytes());
		result.extend_from_slice(field);
	}
	result
}
pub fn version_aad(tenant: &str, resource: &str, version: i64) -> Vec<u8> {
	encode(
		b"aidash-provider-credential-store/v1",
		&[
			tenant.as_bytes(),
			resource.as_bytes(),
			&version.to_be_bytes(),
		],
	)
}
fn check_aad(id: &str) -> Vec<u8> {
	encode(
		b"aidash-provider-credential-store/key-check/v1",
		&[id.as_bytes()],
	)
}
