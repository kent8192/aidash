//! External services are immutable Tenant Registry capabilities (ADR 0054).
use crate::{Error, Result, registry::Entry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const DISCLOSURE: &str = "Sandboxed code may send this External service anything it can read, including Tenant data in the Working Area. Individual requests are not approved. Aidash's zero data retention and Usage Record guarantees do not apply to these calls.";
pub const UNAVAILABLE: &str = "EGRESS_UNAVAILABLE";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
	pub sent_bytes: u64,
	pub received_bytes: u64,
	pub requests: u32,
}
impl Default for Limits {
	fn default() -> Self {
		Self {
			sent_bytes: 1 << 30,
			received_bytes: 1 << 30,
			requests: 1000,
		}
	}
}
impl Limits {
	pub fn validate(&self) -> Result<()> {
		if self.sent_bytes == 0
			|| self.received_bytes == 0
			|| self.requests == 0
			|| self.sent_bytes > i64::MAX as u64
			|| self.received_bytes > i64::MAX as u64
			|| self.requests > i32::MAX as u32
		{
			return Err(Error::Invalid(
				"External service limits must be positive signed integers".into(),
			));
		}
		Ok(())
	}
	pub fn lower_than(&self, ceiling: Self) -> Result<Self> {
		self.validate()?;
		if self.sent_bytes > ceiling.sent_bytes
			|| self.received_bytes > ceiling.received_bytes
			|| self.requests > ceiling.requests
		{
			return Err(Error::Invalid(
				"External service limits exceed Deployment ceilings".into(),
			));
		}
		Ok(*self)
	}
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
	pub sent_bytes: u64,
	pub received_bytes: u64,
	pub requests: u32,
}
pub enum Debit {
	Sent(u64),
	Received(u64),
	Request,
}
impl Usage {
	/// Debit before forwarding. A refused debit leaves every counter unchanged.
	pub fn debit(self, limits: Limits, debit: Debit) -> Result<Self> {
		let exceeded = || Error::Conflict("egress_limit_exceeded".into());
		let mut next = self;
		match debit {
			Debit::Sent(bytes) => {
				next.sent_bytes = next.sent_bytes.checked_add(bytes).ok_or_else(exceeded)?
			}
			Debit::Received(bytes) => {
				next.received_bytes = next
					.received_bytes
					.checked_add(bytes)
					.ok_or_else(exceeded)?
			}
			Debit::Request => next.requests = next.requests.checked_add(1).ok_or_else(exceeded)?,
		}
		if next.sent_bytes > limits.sent_bytes
			|| next.received_bytes > limits.received_bytes
			|| next.requests > limits.requests
		{
			return Err(exceeded());
		}
		Ok(next)
	}
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Authentication {
	#[default]
	Bearer,
	Header {
		name: String,
	},
}
impl Authentication {
	pub fn header(&self) -> &str {
		match self {
			Self::Bearer => "authorization",
			Self::Header { name } => name,
		}
	}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExternalService {
	pub base_url: String,
	pub methods: Vec<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub secret: Option<String>,
	#[serde(default)]
	pub authentication: Authentication,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub limits: Option<Limits>,
}
impl ExternalService {
	pub fn validate(&self) -> Result<()> {
		let url = url::Url::parse(&self.base_url)
			.map_err(|_| Error::Invalid("invalid External service base URL".into()))?;
		if url.scheme() != "https"
			|| url.host_str().is_none()
			|| !url.username().is_empty()
			|| url.password().is_some()
			|| url.query().is_some()
			|| url.fragment().is_some()
		{
			return Err(Error::Invalid("External service requires one HTTPS base URL without credentials, query or fragment".into()));
		}
		let methods = self.methods.iter().collect::<BTreeSet<_>>();
		if methods.is_empty()
			|| methods.len() != self.methods.len()
			|| self.methods.iter().any(|m| {
				!["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"].contains(&m.as_str())
			}) {
			return Err(Error::Invalid("External service methods must be unique HTTP methods; CONNECT and TRACE are forbidden".into()));
		}
		if let Some(secret) = &self.secret {
			crate::tenant_secrets::validate_name(secret)?;
		}
		let header = self.authentication.header().to_ascii_lowercase();
		if header.is_empty()
			|| !header
				.bytes()
				.all(|b| b.is_ascii_alphanumeric() || b == b'-')
			|| matches!(
				header.as_str(),
				"host"
					| "connection" | "transfer-encoding"
					| "content-length"
					| "cookie" | "proxy-authorization"
					| "upgrade" | "te"
					| "trailer" | "expect"
			) || header.starts_with("sec-")
			|| header.starts_with("x-forwarded-")
			|| header.starts_with("x-aidash-")
		{
			return Err(Error::Invalid(
				"invalid External service credential header".into(),
			));
		}
		if let Some(limits) = self.limits {
			limits.validate()?;
		}
		Ok(())
	}
	/// Package indexes alone have response rewriting; callers cannot declare an index.
	pub fn builtin_index(&self, id: &str) -> bool {
		self.secret.is_none()
			&& self.methods == ["GET", "HEAD"]
			&& matches!(
				(id, self.base_url.as_str()),
				("core.npm", "https://registry.npmjs.org/") | ("core.pypi", "https://pypi.org/")
			)
	}
}

pub fn definition(entry: &Entry) -> Result<ExternalService> {
	if entry.kind != "external_service" {
		return Err(Error::Invalid("expected External service".into()));
	}
	let service: ExternalService = serde_json::from_value(entry.config.clone())?;
	service.validate()?;
	Ok(service)
}

#[cfg(test)]
mod tests;

fn operation_via() -> String {
	"operation".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RequestRecord {
	pub id: uuid::Uuid,
	pub tenant: String,
	pub run_id: uuid::Uuid,
	/// `None` for a request made with a Live preview's session token.
	pub operation_id: Option<uuid::Uuid>,
	pub service: String,
	pub method: String,
	pub path: String,
	pub query_digest: String,
	pub status: i32,
	pub sent_bytes: i64,
	pub received_bytes: i64,
	pub duration_ms: i64,
	/// `operation`, `pod` or `page`: which path admitted the request.
	#[serde(default = "operation_via")]
	pub via: String,
	pub error: Option<String>,
	/// The Tenant Secret and pinned version a call carried, and the host it
	/// went to. Absent for a service without a Tenant Secret (ADR 0095).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub secret_id: Option<uuid::Uuid>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub secret_version: Option<String>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub secret_host: Option<String>,
	pub created_at: chrono::DateTime<chrono::Utc>,
}
