// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

//! Typed Pages wire data; OAuth credentials stay native.
use serde::{Deserialize, Serialize};
#[derive(Clone, Serialize, Deserialize)]
pub struct Plugin {
	pub id: String,
	pub publisher_name: String,
	pub slug: String,
	pub name: String,
	pub description: String,
	pub license: String,
	pub origin: String,
	pub price_cents: u64,
	pub currency: String,
	pub declared_permissions: Vec<String>,
	pub latest_version: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PluginVersion {
	pub version: String,
	pub digest: String,
	pub size_bytes: u64,
	pub recalled: bool,
	pub recall_reason: Option<String>,
	pub origin: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
	pub authenticated: bool,
	pub csrf_token: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Publisher {
	pub id: String,
	pub name: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct LibraryEntry {
	pub plugin_id: String,
	pub slug: String,
	pub name: String,
	pub versions: Vec<PluginVersion>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Catalog {
	#[serde(default)]
	pub plugins: Vec<Plugin>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Detail {
	pub plugin: Plugin,
	pub versions: Vec<PluginVersion>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Library {
	pub entries: Vec<LibraryEntry>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Publishers {
	pub publishers: Vec<Publisher>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CreatedPublisher {
	pub publisher: Publisher,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CreatedPlugin {
	pub plugin: Plugin,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PublishedVersion {
	pub version: PluginVersion,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginDraft {
	pub publisher_id: String,
	pub slug: String,
	pub name: String,
	pub description: String,
	pub price_cents: u64,
	pub currency: String,
	pub declared_permissions: Vec<String>,
	pub license: String,
	pub origin: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Publication {
	pub version: String,
	pub files: std::collections::BTreeMap<String, String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Acquisition {
	pub acquired: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Connection {
	pub id: String,
	pub deployment_id: String,
	pub tenant_id: Option<String>,
	pub revoked: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Connections {
	pub connections: Vec<Connection>,
}
