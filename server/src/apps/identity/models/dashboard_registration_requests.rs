//! Persistent dashboard_registration_requests records.

use chrono::{DateTime, Utc};
use reinhardt::db::associations::ForeignKeyField;
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "dashboard_registration_requests")]
#[derive(Serialize, Deserialize)]
pub struct DashboardRegistrationRequest {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[rel(
		foreign_key,
		db_constraint = "dashboard_registration_requests_identity_id_fkey",
		db_index = false
	)]
	pub identity: ForeignKeyField<crate::apps::identity::models::DashboardIdentity>,
	#[field(field_type = "text")]
	pub status: String,
	#[field]
	pub created_at: DateTime<Utc>,
	#[field]
	pub expires_at: DateTime<Utc>,
	#[field(null = true)]
	pub decided_at: Option<DateTime<Utc>>,
	#[field(null = true)]
	pub decided_by: Option<uuid::Uuid>,
	#[field(field_type = "text", null = true)]
	pub decision_actor: Option<String>,
	/// The Tenant this request is addressed to, through a Tenant Binding or an
	/// Invitation. Only Deployment Administrators decide unaddressed requests.
	#[field(field_type = "text", null = true)]
	pub tenant: Option<String>,
	/// The Invitation this request was made with.
	#[rel(
		foreign_key,
		db_constraint = "dashboard_registration_requests_invitation_id_fkey",
		db_index = false,
		null = true
	)]
	pub invitation: ForeignKeyField<crate::apps::identity::models::TenantInvitation>,
}

impl DashboardRegistrationRequest {}
