//! Persistent authorization_catalog_history records.

use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "authorization_catalog_history")]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationCatalogHistory {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	/// Denormalized business reference; the typed relation is authoritative.
	#[field(primary_key = true, field_type = "text", db_column = "entry_id")]
	pub entry_key: String,
	#[field(primary_key = true, field_type = "text")]
	pub entry_version: String,
	#[field(primary_key = true)]
	pub revision: i64,
	#[field]
	pub enabled: bool,
	#[field(field_type = "text")]
	pub actor: String,
	#[field(auto_now_add = true)]
	pub created_at: DateTime<Utc>,
}

use crate::Result;
use reinhardt::core::exception::Error as FrameworkError;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::db::orm::Model;

impl AuthorizationCatalogHistory {
	pub(crate) async fn for_entry(
		tx: &mut dyn TransactionExecutor,
		tenant: &str,
		id: &str,
		version: &str,
	) -> Result<Vec<Self>> {
		Ok(Self::objects()
			.filter(Self::field_tenant().eq(tenant))
			.filter(Self::field_entry_key().eq(id))
			.filter(Self::field_entry_version().eq(version))
			.order_by(&["-created_at", "-revision", "entry_id", "entry_version"])
			.limit(100)
			.all_with_executor(tx)
			.await
			.map_err(FrameworkError::from)?)
	}
}
