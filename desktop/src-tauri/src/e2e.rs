//! Compiled only for explicitly requested debug acceptance builds.
//! Real HTTP, loopback callbacks and OS credentials are used. Only the system
//! browser launcher is replaced by a separate deterministic broker client.
use std::{io, path::PathBuf};

pub fn profiles_path() -> io::Result<PathBuf> {
	let path = PathBuf::from(std::env::var_os("AIDASH_E2E_STATE").ok_or_else(|| {
		io::Error::other("AIDASH_E2E_STATE must name an isolated test directory")
	})?);
	if !path.is_absolute() {
		return Err(io::Error::other("AIDASH_E2E_STATE must be absolute"));
	}
	Ok(path.join("connections.json"))
}

pub fn prepare() -> io::Result<bool> {
	let path = profiles_path()?;
	if std::env::var_os("AIDASH_E2E_CLEANUP").is_some() {
		let profiles = crate::profiles::Profiles::load(path).map_err(io::Error::other)?;
		for profile in profiles.settings.profiles {
			tauri::async_runtime::block_on(crate::credentials::delete(profile))
				.map_err(io::Error::other)?;
		}
		return Ok(true);
	}
	std::fs::create_dir_all(path.parent().ok_or_else(|| io::Error::other("test path"))?)?;
	// The runner asserts that persistence was tested across distinct native PIDs.
	std::fs::write(path.with_file_name("pid"), std::process::id().to_string())?;
	Ok(false)
}

pub fn open_browser(url: &reqwest::Url) -> Result<tokio::process::Child, String> {
	if url.scheme() != "http" || url.host_str() != Some("127.0.0.1") {
		return Err("The test browser accepts only loopback broker fixtures".into());
	}
	let node = std::env::var_os("AIDASH_E2E_NODE").ok_or("Missing test Node binary")?;
	let script = std::env::var_os("AIDASH_E2E_BROWSER").ok_or("Missing test browser script")?;
	tokio::process::Command::new(node)
		.arg(script)
		.arg(url.as_str())
		.kill_on_drop(true)
		.spawn()
		.map_err(|_| "Cannot start the external broker fixture client".into())
}
