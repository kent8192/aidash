//! Persistent authorization_run_reads records.

use crate::apps::identity::services::states::AuthorizationRunReadResourceKind;
use reinhardt::db::associations::ForeignKeyField;
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "authorization_run_reads")]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationRunRead {
	#[field(primary_key = true, field_type = "uuid")]
	pub run_id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[rel(
		foreign_key,
		shared_fields = ["tenant"],
		db_constraint = "authorization_run_reads_workspace_id_fkey",
		db_index = false
	)]
	pub workspace: ForeignKeyField<crate::apps::identity::models::AuthorizationWorkspace>,
	#[field(primary_key = true, field_type = "text", max_length = 64)]
	pub resource_kind: AuthorizationRunReadResourceKind,
	#[field(primary_key = true, field_type = "uuid")]
	pub resource_id: uuid::Uuid,
}

impl AuthorizationRunRead {}
