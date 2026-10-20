//! A person's own Approval Mode and Approval Rules in one Tenant (#411, ADR
//! 0076). Only the owner writes them; Tenant Administrators with
//! `approval.read` read them.
use chrono::{DateTime, Utc};
use reinhardt::db::orm::Json;
use reinhardt::model;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// One subject's default Approval Mode. A subject without a row is in Manual.
#[model(app_label = "identity", table_name = "identity_approval_preferences")]
#[derive(Serialize, Deserialize)]
pub struct ApprovalPreference {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	#[field(primary_key = true, field_type = "text")]
	pub subject: String,
	/// An `ApprovalMode` in snake case.
	#[field(field_type = "text")]
	pub default_mode: String,
	/// How long the subject's Runs wait for an answer to a model's question
	/// (#413); nothing reads it yet.
	#[field(null = true)]
	pub answer_deadline_seconds: Option<i64>,
	/// When the subject accepted the risk of Bypass. Kept once recorded, so
	/// choosing another mode and returning to Bypass asks nothing again.
	#[field(null = true)]
	pub bypass_accepted_at: Option<DateTime<Utc>>,
	#[field]
	pub revision: i64,
	#[field]
	pub updated_at: DateTime<Utc>,
}

/// One allow, ask or deny decision of a subject on a kind and a target.
#[model(app_label = "identity", table_name = "identity_approval_rules")]
#[derive(Serialize, Deserialize)]
pub struct ApprovalRuleRecord {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	#[field(primary_key = true)]
	pub id: Uuid,
	#[field(field_type = "text")]
	pub subject: String,
	/// An `ApprovalKind` in snake case.
	#[field(field_type = "text")]
	pub kind: String,
	/// A `RuleTarget`: the string `"every"` or `{"exact": "<target>"}`.
	#[field]
	pub target: Json<Value>,
	/// A `RuleEffect` in snake case.
	#[field(field_type = "text")]
	pub effect: String,
	#[field]
	pub created_at: DateTime<Utc>,
}

/// A subject's Approval Mode for one Thread, which wins over their default.
/// A subject without a row for the Thread uses their default.
#[model(app_label = "identity", table_name = "identity_thread_approval_modes")]
#[derive(Serialize, Deserialize)]
pub struct ThreadApprovalMode {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	#[field(primary_key = true)]
	pub thread_id: Uuid,
	#[field(primary_key = true, field_type = "text")]
	pub subject: String,
	/// An `ApprovalMode` in snake case.
	#[field(field_type = "text")]
	pub mode: String,
	#[field]
	pub revision: i64,
	#[field]
	pub updated_at: DateTime<Utc>,
}

/// The approval Decider a Tenant chose for the Auto Approval Mode (ADR 0077). A
/// Tenant without a row, or with no Decider in it, uses the built-in one.
#[model(
	app_label = "identity",
	table_name = "identity_tenant_approval_settings"
)]
#[derive(Serialize, Deserialize)]
pub struct TenantApprovalSettings {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	/// A `decider` of hook `approval` in the Tenant Registry; `None` with its
	/// version for the built-in.
	#[field(null = true, field_type = "text")]
	pub decider_id: Option<String>,
	#[field(null = true, field_type = "text")]
	pub decider_version: Option<String>,
	#[field]
	pub revision: i64,
	#[field]
	pub updated_at: DateTime<Utc>,
}
