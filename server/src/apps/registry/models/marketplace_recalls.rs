//! Deployment-wide retained Recall ledger, independent of any live Connection.
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};
#[model(app_label = "registry", table_name = "marketplace_plugin_recalls")]
#[derive(Serialize, Deserialize)]
pub struct MarketplacePluginRecall {
	#[field(primary_key = true)]
	pub plugin_id: uuid::Uuid,
	#[field(primary_key = true, field_type = "text")]
	pub version: String,
	#[field(field_type = "text")]
	pub digest: String,
	#[field(field_type = "text")]
	pub reason: String,
	pub recalled_at: DateTime<Utc>,
}
