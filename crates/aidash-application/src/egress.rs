//! Ports for the internal streaming gateway; authority and counters stay durable.
use crate::{Result, tenant_secrets::SecretRoute};
use aidash_domain::egress::ExternalService;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use secrecy::SecretString;
use std::sync::Arc;
use uuid::Uuid;

pub struct Admission {
	pub gateway_origin: String,
	pub destination: aidash_domain::keyless::Destination,
	pub request: Uuid,
	/// The operation, or a Live preview's session-lifetime owner.
	pub operation: Uuid,
	/// Set when the token is a session token; the request record carries it.
	pub session: Option<Uuid>,
	pub service: String,
	pub definition: ExternalService,
	pub expires_at: DateTime<Utc>,
	/// The value a self-hosted gateway attaches itself. A brokered request
	/// never carries it (ADR 0095).
	pub credential: Option<SecretString>,
	/// Where a request for a secret-bearing External service goes when a Tenant
	/// Secret broker is configured. The broker attaches the value.
	pub route: Option<SecretRoute>,
	pub meter: Arc<dyn Meter>,
}

#[derive(Clone, Copy)]
pub enum Direction {
	Sent,
	Received,
}

#[async_trait]
pub trait Meter: Send + Sync {
	/// Atomically debits both the operation and this service's budget before forwarding bytes.
	async fn charge(&self, direction: Direction, bytes: usize) -> Result<()>;
	/// Completes the metadata-only request record, including failures and partial bodies.
	async fn finish(&self, status: u16, elapsed_ms: u64, error: Option<&str>) -> Result<()>;
}

#[async_trait]
pub trait Authority: Send + Sync {
	/// Rechecks the operation, Run, Tenant, pinned Binding and live Catalog Approval.
	/// The token reaches only this operation's services. `path` excludes the query;
	/// `query_digest` is the only query material admitted into the audit record.
	async fn admit(
		&self,
		token: &str,
		service: &str,
		method: &str,
		path: &str,
		query_digest: &str,
	) -> Result<Admission>;
}
