//! A Tenant's Nodes (#241). `node_names` is the Deployment-wide claim of a
//! name, kept after purge so a name is never reused; `nodes` is the
//! Tenant-owned record with the label and the default flag.
use chrono::{DateTime, Utc};
use reinhardt::model;
use serde::{Deserialize, Serialize};

#[model(app_label = "identity", table_name = "node_names")]
#[derive(Serialize, Deserialize)]
pub struct NodeNameRoot {
	#[field(primary_key = true, field_type = "text")]
	pub name: String,
	#[field(field_type = "text")]
	pub tenant: String,
	#[field]
	pub created_at: DateTime<Utc>,
}

#[model(app_label = "identity", table_name = "nodes")]
#[derive(Serialize, Deserialize)]
pub struct Node {
	#[field(primary_key = true, field_type = "text")]
	pub tenant: String,
	#[field(primary_key = true, field_type = "text")]
	pub name: String,
	#[field(field_type = "text")]
	pub label: String,
	#[field]
	pub is_default: bool,
	#[field]
	pub created_at: DateTime<Utc>,
	/// `identity` or `deployment_administrator`, as `tenants.created_by_kind`.
	#[field(field_type = "text")]
	pub created_by_kind: String,
	#[field(null = true)]
	pub created_by_identity: Option<uuid::Uuid>,
}

impl Node {
	pub fn into_domain(self) -> crate::Result<aidash_domain::nodes::Node> {
		Ok(aidash_domain::nodes::Node {
			tenant: aidash_domain::tenancy::TenantName::new(self.tenant)?,
			name: aidash_domain::nodes::NodeName::parse(&self.name)?,
			label: self.label,
			default: self.is_default,
			created_at: self.created_at,
		})
	}
}
