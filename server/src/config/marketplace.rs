//! Optional per-Tenant Marketplace Connections. URLs and Library ownership are
//! Deployment settings, never import request parameters.
use crate::{Error, Result};
use reinhardt::settings;
use serde::{Deserialize, Serialize};
use std::{
	collections::BTreeSet,
	path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TenantConnection {
	pub tenant: String,
	pub connection_id: Uuid,
	pub owner_account_id: Uuid,
	pub owner_tenant_id: Option<String>,
	/// Local retained reference to the personal Library owner's External Identity.
	/// If not supplied/matched, personal imports require an explicit local policy.
	pub personal_identity_id: Option<Uuid>,
	pub secret_file: PathBuf,
}
#[settings(fragment = true, section = "marketplace", validate = false)]
pub struct MarketplaceSettings {
	#[setting(default = "None")]
	pub endpoint: Option<String>,
	#[setting(default = "Vec::new()", leaf)]
	pub connections: Vec<TenantConnection>,
	#[setting(default = "60")]
	pub poll_seconds: u64,
}
impl MarketplaceSettings {
	pub fn configured(&self, base_dir: &Path) -> Result<Self> {
		let mut configured = self.clone();
		if !(10..=3600).contains(&self.poll_seconds) {
			return Err(Error::Invalid(
				"marketplace.poll_seconds must be 10..3600".into(),
			));
		}
		if !self.connections.is_empty() && self.endpoint.is_none() {
			return Err(Error::Invalid(
				"Marketplace Connections require marketplace.endpoint".into(),
			));
		}
		let mut tenants = BTreeSet::new();
		for connection in &mut configured.connections {
			aidash_domain::policy::identifier(&connection.tenant)?;
			if !tenants.insert(connection.tenant.clone())
				|| connection
					.owner_tenant_id
					.as_ref()
					.is_some_and(|owner| owner != &connection.tenant)
			{
				return Err(Error::Invalid(
					"Marketplace Connection must bind exactly its configured Tenant".into(),
				));
			}
			if connection.secret_file.is_relative() {
				connection.secret_file = base_dir.join(&connection.secret_file);
			}
			let bytes = std::fs::read(&connection.secret_file).map_err(|_| {
				Error::Invalid("Marketplace Connection secret file is unavailable".into())
			})?;
			if bytes.is_empty() || bytes.len() > 8192 {
				return Err(Error::Invalid(
					"invalid Marketplace Connection secret file".into(),
				));
			}
			self.client_for(connection)?;
		}
		Ok(configured)
	}
	pub fn connection(&self, tenant: &str) -> Option<&TenantConnection> {
		self.connections
			.iter()
			.find(|connection| connection.tenant == tenant)
	}
	pub fn client_for(
		&self,
		connection: &TenantConnection,
	) -> Result<aidash_integrations::marketplace::Client> {
		aidash_integrations::marketplace::Client::new(
			self.endpoint
				.as_deref()
				.ok_or_else(|| Error::Invalid("Marketplace endpoint is absent".into()))?,
			aidash_integrations::marketplace::Connection {
				connection_id: connection.connection_id,
				owner_account_id: connection.owner_account_id,
				owner_tenant_id: connection.owner_tenant_id.clone(),
				secret_file: connection.secret_file.clone(),
			},
		)
		.map_err(Into::into)
	}
}

impl reinhardt::conf::settings::fragment::SettingsValidation for MarketplaceSettings {
	fn validate(
		&self,
		_: &reinhardt::conf::settings::profile::Profile,
	) -> reinhardt::conf::settings::validation::ValidationResult {
		if !(10..=3600).contains(&self.poll_seconds) {
			return Err(
				reinhardt::conf::settings::validation::ValidationError::Constraint(
					"marketplace.poll_seconds must be 10..3600".into(),
				),
			);
		}
		Ok(())
	}
}
