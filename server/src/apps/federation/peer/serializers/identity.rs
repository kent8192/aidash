//! External protocol contracts.
use schemars::JsonSchema;
use serde::Serialize;

/// `GET /.well-known/aidash`: the Deployment's identity and its Deployment Key.
/// It names no Node; Nodes belong to Tenants and are not public.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DeploymentIdentity {
	pub id: String,
	pub endpoint: String,
	pub protocol_version: String,
	/// Absent when the Deployment has no Deployment Key, so it cannot federate
	/// with other Deployments.
	pub key: Option<DeploymentKeyIdentity>,
	/// A published normal transition, containing public keys and proof only.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub key_rollover: Option<DeploymentRolloverIdentity>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DeploymentKeyIdentity {
	/// The token `kid`, which a Tenant Administrator types to pin this key.
	pub id: String,
	/// SubjectPublicKeyInfo PEM of the Ed25519 public key.
	pub public_key_pem: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DeploymentRolloverIdentity {
	pub successor: Option<DeploymentKeyIdentity>,
	pub retiring: Option<DeploymentKeyIdentity>,
	pub published_at: u64,
	pub activates_at: u64,
	pub retires_at: u64,
	pub transition: Option<String>,
}
impl From<aidash_application::federation::key::DeploymentPublicKey> for DeploymentKeyIdentity {
	fn from(key: aidash_application::federation::key::DeploymentPublicKey) -> Self {
		Self {
			id: key.id,
			public_key_pem: key.public_key_pem,
		}
	}
}
