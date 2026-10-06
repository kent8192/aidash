//! Persistent authorization_peer_mappings records.

use crate::Result;
use chrono::{DateTime, Utc};
use reinhardt::db::orm::{Model, OrmExecutor};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "federation", table_name = "authorization_peer_mappings")]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationPeerMapping {
	#[field(primary_key = true, field_type = "text")]
	pub source_node: String,
	#[field(primary_key = true, field_type = "text")]
	pub source_tenant: String,
	#[field(primary_key = true, field_type = "text")]
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

impl AuthorizationPeerMapping {
	pub(crate) async fn page<E: OrmExecutor>(
		db: &mut E,
		tenant: &str,
		offset: usize,
		limit: usize,
	) -> Result<Vec<Self>> {
		Ok(Self::objects()
			.filter(Self::field_tenant().eq(tenant))
			.order_by(&["source_node", "source_tenant", "source_subject"])
			.offset(offset)
			.limit(limit)
			.all_with_db(db)
			.await?)
	}
}

impl AuthorizationPeerMapping {
	pub fn tenant_record_id(&self) -> String {
		self.tenant.clone()
	}
}
