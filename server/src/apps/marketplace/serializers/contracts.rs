//! Immutable distribution and installation contracts.
use crate::registry::EntityRef;
use reinhardt::Validate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize, JsonSchema, Validate)]
#[schemars(rename = "MarketplacePublish")]
#[serde(deny_unknown_fields)]
pub struct Publish {
	pub source: EntityRef,
	pub package_id: String,
	pub author: String,
	#[serde(default)]
	pub permissions: Vec<String>,
	#[serde(default)]
	pub dependencies: Vec<EntityRef>,
	pub idempotency_key: Uuid,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema, Validate)]
#[schemars(rename = "MarketplaceAudienceInput")]
#[serde(deny_unknown_fields)]
pub struct AudienceInput {
	pub expected_revision: i64,
	pub tenants: BTreeSet<String>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema, Validate)]
#[schemars(rename = "MarketplaceInstall")]
#[serde(deny_unknown_fields)]
pub struct Install {
	pub digest: String,
	#[serde(default = "crate::domain::empty_object")]
	pub config: Value,
	#[serde(default)]
	pub bindings: Vec<DependencyBinding>,
	pub idempotency_key: Uuid,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema, Validate)]
#[schemars(rename = "MarketplaceConfigure")]
#[serde(deny_unknown_fields)]
pub struct Configure {
	pub expected_revision: i64,
	pub config: Value,
	#[serde(default)]
	pub bindings: Vec<DependencyBinding>,
	pub idempotency_key: Uuid,
}

#[derive(Deserialize, JsonSchema, Validate)]
#[schemars(rename = "MarketplaceCompatibilityInput")]
#[serde(deny_unknown_fields)]
pub struct CompatibilityInput {
	pub enabled: bool,
	pub expected_revision: i64,
	pub compatible_instances_confirmed: bool,
}

#[derive(Clone, Deserialize, Serialize, JsonSchema, Validate)]
#[schemars(rename = "MarketplaceActivate")]
#[serde(deny_unknown_fields)]
pub struct Activate {
	pub tenant: String,
	pub revision: i64,
	pub expected_activation_revision: i64,
	pub expected_catalog_revision: i64,
	pub enabled: bool,
}

#[derive(Clone, Deserialize, Serialize, JsonSchema, Validate)]
#[schemars(rename = "MarketplaceAdopt")]
#[serde(deny_unknown_fields)]
pub struct Adopt {
	pub tenant: String,
	pub source: EntityRef,
	pub idempotency_key: Uuid,
}

pub use aidash_domain::registry::Projection;

pub use aidash_domain::marketplace::{
	Audience, Compatibility, DependencyBinding, Detail, Installation, InstallationRevision,
	PublicationPreview, Summary,
};
