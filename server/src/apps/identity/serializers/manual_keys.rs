//! Existing record administrators change receiving authentication, not authority.
use crate::{Error, Result};
use aidash_domain::nodes::{DeploymentKeyId, keys::PublicDeploymentKey};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegistrationKeySelector {
	pub source_node: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum KeyAction {
	Quarantine,
	Confirm,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegistrationKeyChange {
	pub source_node: String,
	pub action: KeyAction,
	pub expected_keys_revision: i64,
	pub expected_authorization_revision: i64,
	pub public_key_pem: Option<String>,
	/// Independently supplied fingerprint; never inferred from a public fetch.
	pub expected_key_id: Option<String>,
}

pub(crate) struct KeyChange {
	pub action: KeyAction,
	pub expected_keys_revision: i64,
	pub expected_authorization_revision: i64,
	pub public_key_pem: Option<String>,
	pub expected_key_id: Option<String>,
}

impl RegistrationKeyChange {
	pub fn into_parts(self) -> (String, KeyChange) {
		(
			self.source_node,
			KeyChange {
				action: self.action,
				expected_keys_revision: self.expected_keys_revision,
				expected_authorization_revision: self.expected_authorization_revision,
				public_key_pem: self.public_key_pem,
				expected_key_id: self.expected_key_id,
			},
		)
	}
}
impl KeyChange {
	pub fn replacement(&self) -> Result<Option<PublicDeploymentKey>> {
		if !(1..i64::MAX).contains(&self.expected_keys_revision)
			|| self.expected_authorization_revision < 1
		{
			return Err(Error::Invalid("invalid receiving key revision".into()));
		}
		match self.action {
			KeyAction::Quarantine => {
				if self.public_key_pem.is_some() || self.expected_key_id.is_some() {
					return Err(Error::Invalid(
						"quarantine does not accept replacement material".into(),
					));
				}
				Ok(None)
			}
			KeyAction::Confirm => {
				let (Some(pem), Some(kid)) = (&self.public_key_pem, &self.expected_key_id) else {
					return Err(Error::Invalid(
						"public PEM and independently confirmed key ID are required".into(),
					));
				};
				if pem.len() > 4096 || kid.len() > 128 {
					return Err(Error::Invalid("receiving key material is too long".into()));
				}
				let kid = DeploymentKeyId::parse(kid)?;
				let public = aidash_capability::public_key_bytes(pem)
					.map_err(|_| Error::Invalid("an Ed25519 public key PEM is required".into()))?;
				Ok(Some(PublicDeploymentKey::new(kid, public)?))
			}
		}
	}
	pub fn action_name(&self) -> &'static str {
		match self.action {
			KeyAction::Quarantine => "node_registration.key_quarantine",
			KeyAction::Confirm => "node_registration.key_confirm",
		}
	}
}

#[derive(Serialize, JsonSchema)]
pub(crate) struct ReceivingKeyStatus {
	pub key_id: String,
	pub quarantined: bool,
	pub keys_revision: i64,
	pub authorization_revision: i64,
	pub enabled: bool,
}
