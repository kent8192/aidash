use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::{io::Write, path::PathBuf};

#[derive(Clone, Serialize, Deserialize)]
pub struct Profile {
	pub id: String,
	pub name: String,
	pub origin: String,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Settings {
	pub profiles: Vec<Profile>,
	pub selected: Option<String>,
}
pub struct Profiles {
	pub settings: Settings,
	path: PathBuf,
}

pub fn origin(value: &str) -> Result<String, String> {
	let url = Url::parse(value).map_err(|_| "Enter a valid Aidash origin URL")?;
	let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
	if !(url.scheme() == "https" || url.scheme() == "http" && loopback)
		|| !url.username().is_empty()
		|| url.password().is_some()
		|| url.path() != "/"
		|| url.query().is_some()
		|| url.fragment().is_some()
		|| url.host_str().is_none()
	{
		return Err("Use an HTTPS origin (HTTP is allowed only for localhost, 127.0.0.1 or [::1]), without a path or credentials".into());
	}
	Ok(url.origin().ascii_serialization())
}
impl Profiles {
	pub fn load(path: PathBuf) -> Result<Self, String> {
		let settings: Settings = match std::fs::read(&path) {
			Ok(bytes) => serde_json::from_slice(&bytes)
				.map_err(|_| "Cannot read saved connections; the file has been preserved")?,
			Err(error) if error.kind() == std::io::ErrorKind::NotFound => Settings::default(),
			Err(_) => return Err("Cannot read saved connections".into()),
		};
		for profile in &settings.profiles {
			if origin(&profile.origin)? != profile.origin
				|| uuid::Uuid::parse_str(&profile.id).is_err()
			{
				return Err("Invalid saved connection; the file has been preserved".into());
			}
		}
		Ok(Self { settings, path })
	}
	pub fn persist(&mut self, settings: Settings) -> Result<(), String> {
		let parent = self
			.path
			.parent()
			.ok_or("Invalid configuration directory")?;
		std::fs::create_dir_all(parent).map_err(|_| "Cannot create the connections directory")?;
		let mut file =
			tempfile::NamedTempFile::new_in(parent).map_err(|_| "Cannot save connections")?;
		serde_json::to_writer(&mut file, &settings).map_err(|_| "Cannot encode connections")?;
		file.flush()
			.and_then(|_| file.as_file().sync_all())
			.map_err(|_| "Cannot save connections")?;
		file.persist(&self.path)
			.map_err(|_| "Cannot replace the connections file")?;
		self.settings = settings;
		Ok(())
	}
	pub fn selected(&self) -> Result<Profile, String> {
		self.settings
			.profiles
			.iter()
			.find(|p| Some(&p.id) == self.settings.selected.as_ref())
			.cloned()
			.ok_or("Choose an Aidash connection".into())
	}
}
/// Only bundled pages may inhabit a native-capable window.
pub fn local_navigation(url: &Url) -> bool {
	let allowed = matches!(
		url.origin().ascii_serialization().as_str(),
		"tauri://localhost" | "http://tauri.localhost"
	);
	// Custom protocols have an opaque origin in url::Url.
	let custom = url.scheme() == "tauri" && url.host_str() == Some("localhost");
	allowed
		|| custom
		|| cfg!(debug_assertions) && url.origin().ascii_serialization() == "http://127.0.0.1:1420"
}
#[cfg(test)]
mod tests {
	use super::*;
	#[rstest::rstest]
	#[case("http://example.com")]
	#[case("https://user:password@example.com")]
	#[case("https://example.com/path")]
	#[case("https://example.com/?secret=x")]
	#[case("file:///tmp/index.html")]
	#[case("http://localhost.evil.test")]
	fn refuses_unsafe_destinations(#[case] value: &str) {
		assert!(origin(value).is_err());
	}
	#[test]
	fn persists_metadata_without_changing_destination() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("connections.json");
		let mut profiles = Profiles::load(path.clone()).unwrap();
		let id = uuid::Uuid::new_v4().to_string();
		profiles
			.persist(Settings {
				profiles: vec![Profile {
					id: id.clone(),
					name: "Local".into(),
					origin: origin("http://127.0.0.1:8080/").unwrap(),
				}],
				selected: Some(id.clone()),
			})
			.unwrap();
		assert_eq!(Profiles::load(path).unwrap().selected().unwrap().id, id);
	}
	#[test]
	fn remote_pages_have_no_native_window() {
		assert!(!local_navigation(
			&Url::parse("https://aidash.example/").unwrap()
		));
		assert!(!local_navigation(
			&Url::parse("https://tauri.localhost/").unwrap()
		));
		assert!(local_navigation(
			&Url::parse("tauri://localhost/index.html").unwrap()
		));
	}
}
