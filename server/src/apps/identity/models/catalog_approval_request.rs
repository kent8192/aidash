//! Persistent Catalog Approval Requests (ADR 0096). A request records who asked
//! and which closure they saw; it never grants authority.

use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "catalog_approval_requests")]
#[derive(Serialize, Deserialize)]
pub struct CatalogApprovalRequest {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field(field_type = "text")]
	pub node: String,
	#[field(field_type = "text")]
	pub root_id: String,
	#[field(field_type = "text")]
	pub root_version: String,
	#[field(field_type = "text")]
	pub requester: String,
	#[field(field_type = "text")]
	pub reason: String,
	#[field(field_type = "text")]
	pub closure_digest: String,
	/// `open`, `fulfilled`, `declined`, `stale` or `withdrawn`.
	#[field(field_type = "text")]
	pub state: String,
	#[field(field_type = "text", null = true)]
	pub decline_reason: Option<String>,
	#[field(field_type = "text", null = true)]
	pub decided_by: Option<String>,
	#[field]
	pub created_at: DateTime<Utc>,
	#[field]
	pub updated_at: DateTime<Utc>,
	#[field]
	pub revision: i64,
}
