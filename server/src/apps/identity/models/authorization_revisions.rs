//! Persistent authorization_revisions records.

use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[model(app_label = "identity", table_name = "authorization_revisions")]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationRevision {
	#[field(field_type = "text", primary_key = true)]
	pub tenant: String,
	#[field(primary_key = true)]
	pub revision: i64,
	#[field]
	pub document: Json<Value>,
	#[field(field_type = "text")]
	pub actor: String,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}

use crate::Result;
use reinhardt::db::orm::{Model, OrmExecutor};
use serde_json::json;

impl AuthorizationRevision {
	pub(crate) async fn page<E: OrmExecutor>(
		db: &mut E,
		tenant: &str,
		after: i64,
		limit: i64,
	) -> Result<Vec<Value>> {
		Ok(Self::objects()
			.filter(Self::field_tenant().eq(tenant))
			.filter(Self::field_revision().gt(after))
			.order_by(&["revision"])
			.limit(limit.clamp(1, 200) as usize)
			.all_with_db(db)
			.await?
			.into_iter()
			.map(|row| {
				json!({
					"tenant": row.tenant, "revision": row.revision, "document": row.document.0,
					"actor": row.actor, "created_at": row.created_at,
				})
			})
			.collect())
	}
}

impl AuthorizationRevision {
	pub fn tenant_record_id(&self) -> String {
		self.tenant.clone()
	}
}
