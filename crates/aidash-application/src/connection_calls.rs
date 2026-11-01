//! The OAuth broker is the only reader on Cloud and Dedicated. The server
//! sends metadata and transient code/verifier bytes, and receives pins only.
use crate::{
	Result,
	plan_connections::RefreshFailure,
	tenant_secrets::{SecretCall, SecretRoute},
};
use aidash_capability::connection::ClientSecretRef;
use async_trait::async_trait;
use secrecy::SecretString;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Clone)]
pub struct ExchangeGrant {
	pub issuer: String,
	pub resource: String,
	pub client_id: String,
	pub client: Option<ClientSecretRef>,
	pub redirect_uri: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
	pub version: String,
	pub expires_in: Option<u64>,
	pub scope: Option<String>,
	#[serde(default)]
	pub refresh: bool,
}
#[async_trait]
pub trait ConnectionCalls: Send + Sync {
	async fn exchange(
		&self,
		tenant: &str,
		connection: Uuid,
		grant: ExchangeGrant,
		code: &str,
		verifier: &SecretString,
	) -> Result<Receipt>;
	async fn refresh(
		&self,
		tenant: &str,
		connection: Uuid,
		version: &str,
	) -> std::result::Result<Receipt, RefreshFailure>;
	async fn revoke(&self, tenant: &str, connection: Uuid, version: &str) -> Result<()>;
	async fn route(
		&self,
		tenant: &str,
		connection: Uuid,
		version: &str,
		call: SecretCall,
	) -> Result<SecretRoute>;
}
