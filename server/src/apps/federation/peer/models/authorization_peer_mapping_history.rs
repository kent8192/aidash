//! Persistent authorization_peer_mapping_history records.

use crate::Result;
use chrono::{DateTime, Utc};
use reinhardt::db::orm::{Model, OrmExecutor};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(
	app_label = "federation",
	table_name = "authorization_peer_mapping_history"
)]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationPeerMappingHistory {
	#[field(primary_key = true)]
	pub sequence: i64,
	#[field(field_type = "text")]
	pub source_node: String,
	#[field(field_type = "text")]
	pub source_tenant: String,
	#[field(field_type = "text")]
	pub source_subject: String,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub credential_id: uuid::Uuid,
	#[field]
	pub enabled: bool,
	#[field]
	pub revision: i64,
	#[field(field_type = "text")]
	pub actor: String,
	#[field]
	pub updated_at: DateTime<Utc>,
}

impl AuthorizationPeerMappingHistory {
	pub(crate) async fn page<E: OrmExecutor>(
		db: &mut E,
		tenant: &str,
		after: i64,
		limit: usize,
	) -> Result<Vec<Self>> {
		Ok(Self::objects()
			.filter(Self::field_tenant().eq(tenant))
			.filter(Self::field_sequence().gt(after))
			.order_by(&["sequence"])
			.limit(limit)
			.all_with_db(db)
			.await?)
	}
}

impl AuthorizationPeerMappingHistory {
	pub fn tenant_record_id(&self) -> String {
		self.tenant.clone()
	}
}
