//! Persistent authorization_peer_graph_grants records.
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "authorization_peer_graph_grants")]
#[derive(Serialize, Deserialize)]
pub struct AuthorizationPeerGraphGrants {
	#[field(field_type = "text", primary_key = true)]
	pub tenant: String,
	/// The viewer's Deployment id.
	#[field(primary_key = true, field_type = "text")]
	pub source_deployment: String,
	#[field(primary_key = true, field_type = "uuid")]
	pub source_identity: uuid::Uuid,
	/// The viewer Deployment's key, pinned like a Node Registration pins one.
	#[field(field_type = "text")]
	pub source_key_id: String,
	#[field(field_type = "text")]
	pub source_public_key_pem: String,
	#[field]
	pub enabled: bool,
	#[field]
	pub revision: i64,
	#[field(auto_now_add = true)]
	pub updated_at: DateTime<Utc>,
}

impl AuthorizationPeerGraphGrants {
	pub fn bundle_id(&self) -> String {
		self.tenant.clone()
	}
}
