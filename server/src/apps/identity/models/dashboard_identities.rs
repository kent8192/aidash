//! Persistent dashboard_identities records.

use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(
	app_label = "identity",
	table_name = "dashboard_identities",
	constraints = [unique(fields = ["issuer", "gcip_tenant", "subject"], name = "dashboard_identity_key")]
)]
#[derive(Serialize, Deserialize)]
pub struct DashboardIdentity {
	#[field(primary_key = true)]
	pub id: uuid::Uuid,
	#[field(field_type = "text")]
	pub issuer: String,
	#[field(field_type = "text")]
	pub subject: String,
	#[field(field_type = "text", default = "")]
	pub gcip_tenant: String,
	#[field(null = true)]
	pub valid_since: Option<DateTime<Utc>>,
	#[field(field_type = "text", null = true)]
	pub verified_email: Option<String>,
	#[field(field_type = "text", null = true)]
	pub display_name: Option<String>,
	#[field(null = true)]
	pub last_valid_at: Option<DateTime<Utc>>,
	#[field(null = true)]
	pub disabled_at: Option<DateTime<Utc>>,
	/// The latest sign-in that offered Display Attributes, bounding their retention.
	#[field(null = true)]
	pub display_observed_at: Option<DateTime<Utc>>,
	/// Display Erasure is permanent: later sign-ins no longer record attributes.
	#[field(null = true)]
	pub display_erased_at: Option<DateTime<Utc>>,
	#[field(field_type = "text", null = true)]
	pub display_erased_by: Option<String>,
}
