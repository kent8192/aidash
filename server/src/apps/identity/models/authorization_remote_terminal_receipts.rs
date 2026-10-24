//! Minimal receiver evidence for an exact original terminal Cancel retry.
//! No admission/Run payload or authority survives through this record.
use aidash_domain::federation::execution::admission::Activation;
use reinhardt::{db::orm::Json, model};
use serde::{Deserialize, Serialize};
#[model(
	app_label = "identity",
	table_name = "authorization_remote_terminal_receipts"
)]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationRemoteTerminalReceipt {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub node: String,
	#[field(field_type = "text")]
	pub source_node: String,
	#[field]
	pub grant_id: uuid::Uuid,
	#[field]
	pub activation: Json<Activation>,
}
