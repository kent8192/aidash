//! Persistent authorization_run_registry_reads records.

use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(
	app_label = "identity",
	table_name = "authorization_run_registry_reads"
)]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationRunRegistryRead {
	#[field(primary_key = true, field_type = "uuid")]
	pub run_id: uuid::Uuid,
	#[field(primary_key = true, field_type = "text")]
	pub entry_id: String,
	#[field(primary_key = true, field_type = "text")]
	pub entry_version: String,
}

impl AuthorizationRunRegistryRead {}
