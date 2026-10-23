//! The Deployment Key of a Self-hosted Deployment: an Ed25519 private key in a
//! PKCS#8 PEM file that only the server's user may read (ADR 0064).
use aidash_application::{Error, Result, federation::key::DeploymentKey};
use aidash_capability::SignError;
use aidash_domain::nodes::DeploymentKeyId;
use async_trait::async_trait;
use ed25519_dalek::{
	Signer, SigningKey,
	pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey},
};
use pkcs8::LineEnding;
use secrecy::{ExposeSecret, SecretString};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

pub struct FileDeploymentKey {
	signing: SigningKey,
	key_id: DeploymentKeyId,
	public_key_pem: String,
}

impl std::fmt::Debug for FileDeploymentKey {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("FileDeploymentKey")
			.field("key_id", &self.key_id)
			.finish_non_exhaustive()
	}
}

impl FileDeploymentKey {
	pub fn new(signing: SigningKey) -> Result<Self> {
		let verifying = signing.verifying_key();
		let public_key_pem = verifying
			.to_public_key_pem(LineEnding::LF)
			.map_err(|_| Error::Invalid("the Deployment Key has no public key PEM".into()))?;
		Ok(Self {
			key_id: DeploymentKeyId::of_public_key(verifying.as_bytes()),
			public_key_pem,
			signing,
		})
	}

	/// Refuses a file that is missing, unreadable, not a regular file, not an
	/// Ed25519 PKCS#8 PEM, or readable by its group or others. The messages name
	/// the path and the reason, never the key.
	pub fn load(path: &Path) -> Result<Self> {
		let refuse = |reason: &str| {
			Error::Invalid(format!("Deployment Key file {}: {reason}", path.display()))
		};
		let mut file = File::open(path).map_err(|error| refuse(&describe(&error)))?;
		let metadata = file.metadata().map_err(|error| refuse(&describe(&error)))?;
		if !metadata.is_file() {
			return Err(refuse("is not a regular file"));
		}
		#[cfg(unix)]
		{
			use std::os::unix::fs::PermissionsExt;
			if metadata.permissions().mode() & 0o077 != 0 {
				return Err(refuse(
					"is readable by its group or others; restrict it to its owner (chmod 600)",
				));
			}
		}
		let mut text = String::new();
		file.read_to_string(&mut text)
			.map_err(|error| refuse(&describe(&error)))?;
		let text = SecretString::from(text);
		let signing = SigningKey::from_pkcs8_pem(text.expose_secret())
			.map_err(|_| refuse("is not an Ed25519 private key in PKCS#8 PEM"))?;
		Self::new(signing)
	}

	/// Writes a new key to `path`, readable by its owner only. An existing file
	/// is kept unless `replace` is set; replacing writes beside it and renames,
	/// so a reader sees the old key or the new one, never a partial file.
	pub fn generate(path: &Path, replace: bool) -> Result<Self> {
		let signing = SigningKey::generate(&mut rand_core::OsRng);
		let pem = signing
			.to_pkcs8_pem(LineEnding::LF)
			.map_err(|_| Error::Invalid("the Deployment Key cannot be encoded".into()))?;
		let write = |error: std::io::Error| {
			Error::Invalid(format!(
				"Deployment Key file {}: {}",
				path.display(),
				describe(&error)
			))
		};
		if let Some(parent) = path
			.parent()
			.filter(|parent| !parent.as_os_str().is_empty())
		{
			std::fs::create_dir_all(parent).map_err(write)?;
		}
		if replace {
			let mut staged = path.as_os_str().to_owned();
			staged.push(".new");
			let staged = std::path::PathBuf::from(staged);
			let _ = std::fs::remove_file(&staged);
			let result = create_private(&staged)
				.and_then(|mut file| {
					file.write_all(pem.as_bytes())
						.and_then(|()| file.sync_all())
				})
				.and_then(|()| std::fs::rename(&staged, path));
			if result.is_err() {
				let _ = std::fs::remove_file(&staged);
			}
			result.map_err(write)?;
		} else {
			let mut file = create_private(path).map_err(|error| {
				if error.kind() == std::io::ErrorKind::AlreadyExists {
					Error::Conflict(format!(
						"Deployment Key file {} already exists; pass --force to replace it",
						path.display()
					))
				} else {
					write(error)
				}
			})?;
			let result = file
				.write_all(pem.as_bytes())
				.and_then(|()| file.sync_all());
			if let Err(error) = result {
				let _ = std::fs::remove_file(path);
				return Err(write(error));
			}
		}
		Self::new(signing)
	}
}

fn create_private(path: &Path) -> std::io::Result<File> {
	let mut options = OpenOptions::new();
	options.write(true).create_new(true);
	#[cfg(unix)]
	{
		use std::os::unix::fs::OpenOptionsExt;
		options.mode(0o600);
	}
	options.open(path)
}

fn describe(error: &std::io::Error) -> String {
	match error.kind() {
		std::io::ErrorKind::NotFound => "does not exist".into(),
		std::io::ErrorKind::PermissionDenied => "cannot be read by the server's user".into(),
		_ => format!("cannot be read ({})", error.kind()),
	}
}

#[async_trait]
impl DeploymentKey for FileDeploymentKey {
	fn key_id(&self) -> &DeploymentKeyId {
		&self.key_id
	}
	fn public_key_pem(&self) -> &str {
		&self.public_key_pem
	}
	async fn sign(&self, input: &[u8]) -> std::result::Result<[u8; 64], SignError> {
		Ok(self.signing.sign(input).to_bytes())
	}
}

#[cfg(test)]
mod tests;
