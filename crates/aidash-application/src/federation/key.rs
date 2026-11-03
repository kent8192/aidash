//! The Deployment Key: the one key a Deployment signs its Federation tokens
//! with. The scheduled ring publishes public transition state (ADR 0084).
use aidash_capability::{SignError, TokenSigner};
use aidash_domain::nodes::DeploymentKeyId;
use async_trait::async_trait;

/// Signing is external, like `TokenSigner`: the adapter receives the exact JWS
/// input, never a digest, and the private key never leaves it. A Deployment
/// without a Deployment Key cannot federate with other Deployments, so the
/// runtime holds an `Option` of this.
#[async_trait]
pub trait DeploymentKey: Send + Sync {
	/// The token `kid`, derived from the public key.
	fn key_id(&self) -> &DeploymentKeyId;
	/// SubjectPublicKeyInfo PEM, what `/.well-known/aidash` publishes.
	fn public_key_pem(&self) -> &str;
	async fn sign(&self, input: &[u8]) -> Result<[u8; 64], SignError>;
}

/// A Deployment Key as the signer of a Federation token.
pub struct Signing<'a>(pub &'a dyn DeploymentKey);

#[async_trait]
impl TokenSigner for Signing<'_> {
	fn kid(&self) -> &str {
		self.0.key_id().as_str()
	}
	async fn sign(&self, input: &[u8]) -> Result<[u8; 64], SignError> {
		self.0.sign(input).await
	}
}

mod ring;
pub use ring::{
	DeploymentKeyLoader, DeploymentKeyPublication, DeploymentKeyRollover, DeploymentKeySource,
	DeploymentKeyStore, DeploymentKeys, DeploymentPublicKey, FederationSigning,
	ScheduledDeploymentKeys,
};
