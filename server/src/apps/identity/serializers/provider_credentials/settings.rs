//! Non-secret managed Store/broker settings layered above operator TOML defaults.
use reinhardt::conf::settings::sources::{ConfigSource, ScopedSource, SourceError};
use std::path::PathBuf;

pub(crate) struct ManagedSource(PathBuf);
impl ManagedSource {
	pub(crate) fn new(path: impl Into<PathBuf>) -> Self {
		Self(path.into())
	}
}
impl ConfigSource for ManagedSource {
	fn load(&self) -> Result<indexmap::IndexMap<String, serde_json::Value>, SourceError> {
		let value: serde_json::Value = serde_json::from_slice(&std::fs::read(&self.0)?)?;
		let object = value
			.as_object()
			.filter(|object| object.len() == 1 && object.contains_key("provider_credentials"))
			.ok_or_else(|| {
				SourceError::InvalidSource(
					"managed settings may contain only provider_credentials".into(),
				)
			})?;
		Ok(object
			.iter()
			.map(|(key, value)| (key.clone(), value.clone()))
			.collect())
	}
	fn load_scoped(&self) -> Result<ScopedSource, SourceError> {
		Ok(ScopedSource {
			values: self.load()?,
			interpolation_file: None,
		})
	}
	fn priority(&self) -> u8 {
		55
	}
	fn description(&self) -> String {
		"Managed Provider Credential settings".into()
	}
}
