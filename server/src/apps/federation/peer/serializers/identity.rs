//! External protocol contracts.
use schemars::JsonSchema;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct NodeIdentity {
	pub id: String,
	pub endpoint: String,
	pub capabilities: Vec<String>,
	pub clusters: Vec<String>,
	pub protocol_version: String,
}

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
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DeploymentKeyIdentity {
	/// The token `kid`, which a Tenant Administrator types to pin this key.
	pub id: String,
	/// SubjectPublicKeyInfo PEM of the Ed25519 public key.
	pub public_key_pem: String,
}
