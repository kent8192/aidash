//! Immutable distribution identities, revisions, and dependency contracts.
use crate::registry::{EntityRef, Entry, Localized, Package};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "MarketplaceSummary")]
pub struct Summary {
	pub key: String,
	pub repository: String,
	pub owner_tenant: String,
	pub package_id: String,
	pub version: String,
	pub kind: String,
	pub name: Localized,
	pub description: Localized,
	pub author: String,
	pub capabilities: Vec<String>,
	pub permissions: Vec<String>,
	pub languages: Vec<String>,
	pub digest: String,
	pub actions: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "MarketplaceDetail")]
pub struct Detail {
	pub summary: Summary,
	pub manifest: Package,
	pub audience: Audience,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "MarketplaceAudience")]
pub struct Audience {
	pub revision: i64,
	pub tenants: BTreeSet<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "MarketplaceDependencyBinding")]
#[serde(deny_unknown_fields)]
pub struct DependencyBinding {
	pub source: EntityRef,
	pub target: EntityRef,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "MarketplaceInstallation")]
pub struct Installation {
	pub id: String,
	pub tenant: String,
	pub package_key: String,
	pub latest_revision: i64,
	pub active_revision: Option<i64>,
	pub activation_revision: i64,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "MarketplaceInstallationRevision")]
pub struct InstallationRevision {
	pub installation: Installation,
	pub revision: i64,
	pub entry: Entry,
	pub digest: String,
	pub config: Value,
	pub dependencies: Vec<EntityRef>,
	pub bindings: Vec<DependencyBinding>,
	pub approved: bool,
	pub actions: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, JsonSchema)]
#[schemars(rename = "MarketplaceCompatibility")]
pub struct Compatibility {
	pub enabled: bool,
	pub revision: i64,
	pub contract: u8,
}

#[derive(Serialize, schemars::JsonSchema)]
#[schemars(rename = "MarketplacePublicationPreview")]
pub struct PublicationPreview {
	pub allowed: bool,
	pub version: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct ConsentEdge {
	pub source: String,
	pub redistributor: String,
	pub grant: SourceGrant,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SourceGrant {
	Consent,
	Audience,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Version {
	pub key: String,
	pub repository: String,
	pub owner_tenant: String,
	pub package_id: String,
	pub version: String,
	pub kind: String,
	pub publisher: String,
	pub source: EntityRef,
	pub manifest_source: String,
	pub digest: String,
	pub dependencies: Vec<Dependency>,
	pub lineage: BTreeSet<ConsentEdge>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Dependency {
	pub reference: EntityRef,
	pub kind: String,
	pub digest: String,
	pub package: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Revision {
	pub installation: String,
	pub tenant: String,
	pub revision: i64,
	pub entry: Entry,
	pub digest: String,
	pub config: Value,
	pub dependencies: Vec<EntityRef>,
	pub bindings: Vec<DependencyBinding>,
	// Retain the verified original source independently of mutable distribution.
	pub source: Version,
}

pub mod definitions;

pub mod publication;

#[derive(Clone, Serialize, Deserialize)]
pub struct Replay {
	pub fingerprint: String,
	pub result: Value,
}

pub mod installations;

pub mod events;
