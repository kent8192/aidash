//! Refresh secrets never cross IPC or fall back to an unencrypted file.
use crate::profiles::Profile;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct Credential {
	pub current: String,
	pub pending: Option<String>,
}
fn entry(profile: &Profile) -> Result<keyring::Entry, String> {
	let build = if cfg!(feature = "e2e") {
		"e2e"
	} else if cfg!(debug_assertions) {
		"development"
	} else {
		"production"
	};
	let service = format!("dev.aidash.desktop.{build}");
	let user = format!("{}:{:x}", profile.id, Sha256::digest(&profile.origin));
	keyring::Entry::new(&service, &user).map_err(store_error)
}
fn store_error(error: keyring::Error) -> String {
	match error {
		keyring::Error::NoStorageAccess(_) => {
			"Unlock your OS credential store and retry. Saved credentials have been preserved."
		}
		_ => {
			"The OS credential store is unavailable. Unlock it and retry; no plaintext fallback is used."
		}
	}
	.into()
}
pub async fn read(profile: Profile) -> Result<Option<Credential>, String> {
	tokio::task::spawn_blocking(move || match entry(&profile)?.get_password() {
		Ok(value) => {
			let value = Zeroizing::new(value);
			serde_json::from_str(&value)
				.map(Some)
				.map_err(|_| "Cannot decode the saved credential; it has been preserved".into())
		}
		Err(keyring::Error::NoEntry) => Ok(None),
		Err(error) => Err(store_error(error)),
	})
	.await
	.map_err(|_| "Credential worker failed")?
}
pub async fn write(profile: Profile, credential: Credential) -> Result<(), String> {
	tokio::task::spawn_blocking(move || {
		let value = Zeroizing::new(
			serde_json::to_string(&credential).map_err(|_| "Cannot encode credential")?,
		);
		entry(&profile)?.set_password(&value).map_err(store_error)
	})
	.await
	.map_err(|_| "Credential worker failed")?
}
pub async fn delete(profile: Profile) -> Result<(), String> {
	tokio::task::spawn_blocking(move || match entry(&profile)?.delete_credential() {
		Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
		Err(error) => Err(store_error(error)),
	})
	.await
	.map_err(|_| "Credential worker failed")?
}
