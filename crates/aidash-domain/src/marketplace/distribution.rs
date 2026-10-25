//! Product-independent Agent Plugin distribution contracts and validation.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use std::collections::BTreeMap;
use uuid::Uuid;

pub const MAX_ARTIFACT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_ARTIFACT_FILES: usize = 512;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plugin {
	pub id: Uuid,
	pub publisher_id: Uuid,
	pub publisher_name: String,
	pub slug: String,
	pub name: String,
	pub description: String,
	pub price_cents: i64,
	pub currency: String,
	pub declared_permissions: Vec<String>,
	pub license: String,
	pub origin: String,
	pub latest_version: Option<String>,
}
impl Plugin {
	pub fn validate(&self) -> Result<(), &'static str> {
		if self.slug.is_empty()
			|| self.slug.len() > 120
			|| !self
				.slug
				.bytes()
				.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
		{
			return Err("slug must contain lowercase ASCII letters, digits or hyphens");
		}
		if self.name.is_empty()
			|| self.name.len() > 200
			|| self.description.len() > 16_384
			|| self.price_cents < 0
			|| self.currency != "USD"
			|| self.license.is_empty()
			|| self.license.len() > 512
			|| self.origin.len() > 2048
			|| self.declared_permissions.len() > 100
			|| self
				.declared_permissions
				.iter()
				.any(|p| p.is_empty() || p.len() > 200)
		{
			return Err("invalid plugin metadata");
		}
		let origin = url::Url::parse(&self.origin).map_err(|_| "invalid artifact origin")?;
		if origin.scheme() != "https"
			|| origin.host_str().is_none()
			|| !origin.username().is_empty()
			|| origin.password().is_some()
		{
			return Err("artifact origin must be an HTTPS URL without credentials");
		}
		Ok(())
	}
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
	pub plugin_id: Uuid,
	pub version: String,
	pub digest: String,
	/// Authoritative acquisition receipt, excluded from the immutable tree digest.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub entitlement_expires_at: Option<DateTime<Utc>>,
	/// Digest uses the existing Agent Plugin canonical tree digest.
	/// Values are canonical RFC 4648 standard base64; paths are relative POSIX paths.
	pub files: BTreeMap<String, String>,
}
impl Artifact {
	pub fn validate(&self) -> Result<usize, &'static str> {
		semver::Version::parse(&self.version).map_err(|_| "invalid semantic version")?;
		if self.version.len() > 128 {
			return Err("invalid semantic version");
		}

		if self.files.is_empty() || self.files.len() > MAX_ARTIFACT_FILES {
			return Err("invalid artifact file count");
		}
		let mut size = 0usize;
		let mut files = Vec::new();
		for (path, content) in &self.files {
			if !path.is_ascii()
				|| path.len() > 512
				|| path.starts_with('/')
				|| path.contains('\\')
				|| path.contains('\0')
				|| path
					.split('/')
					.any(|p| p.is_empty() || p == "." || p == "..")
			{
				return Err("invalid artifact path");
			}
			if content.len() > MAX_ARTIFACT_BYTES * 4 / 3 + 4 {
				return Err("artifact too large");
			}
			let bytes = STANDARD
				.decode(content)
				.map_err(|_| "invalid artifact base64")?;
			if STANDARD.encode(&bytes) != *content {
				return Err("noncanonical artifact base64");
			}
			size = size.checked_add(bytes.len()).ok_or("artifact too large")?;
			if size > MAX_ARTIFACT_BYTES {
				return Err("artifact too large");
			}
			files.push(crate::agent_plugin::PluginFile {
				path: path.clone(),
				bytes,
			});
		}
		let loaded =
			crate::agent_plugin::load(&files).map_err(|_| "invalid Agent Plugin artifact")?;
		if loaded
			.manifest
			.version
			.as_deref()
			.is_some_and(|version| version != self.version)
		{
			return Err("artifact manifest version differs from immutable envelope");
		}
		let digest = loaded.digest;
		if self.digest != digest {
			return Err("artifact digest mismatch");
		}
		Ok(size)
	}
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PublishedVersion {
	pub version: String,
	pub digest: String,
	pub size_bytes: usize,
	pub recalled: bool,
	pub recall_reason: Option<String>,
	pub origin: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Publisher {
	pub id: Uuid,
	pub name: String,
	pub account_id: Uuid,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entitlement {
	pub plugin_id: Uuid,
	pub owner: LibraryOwner,
	pub expires_at: Option<DateTime<Utc>>,
	pub billing_event_id: Option<String>,
}
impl Entitlement {
	pub fn active_at(&self, now: DateTime<Utc>) -> bool {
		self.expires_at.is_none_or(|expires| expires > now)
	}
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum LibraryOwner {
	Account(Uuid),
	Tenant {
		deployment_id: String,
		tenant_id: String,
	},
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Connection {
	pub id: Uuid,
	pub account_id: Uuid,
	pub deployment_id: String,
	/// Only a pre-provisioned administrator enrollment can bind a Tenant.
	pub tenant_id: Option<String>,
	pub credential_digest: String,
	pub revoked: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Recall {
	pub plugin_id: Uuid,
	pub version: String,
	pub digest: String,
	pub reason: String,
	pub recalled_at: DateTime<Utc>,
}
