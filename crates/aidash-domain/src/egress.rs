//! External services are immutable Tenant Registry capabilities (ADR 0054).
use crate::{Error, Result, registry::Entry};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The identity used to account for one Scratch Space's traffic.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScopeId {
	pub tenant: String,
	pub node: String,
	pub scratch_space: uuid::Uuid,
}

/// Hosts a Scratch Space may reach by default (#845): each ecosystem's official,
/// anonymously readable package sources and the delivery hosts they send
/// downloads to, and the hosts a tool fetches itself or its runtime from. No
/// unofficial mirrors, no new general-purpose content hosts, and nothing whose
/// terms restrict commercial use (Anaconda's own channels; conda-forge instead).
/// Hosts marked "seen" were reached by the tools on the #845 live rig; the rest
/// come from each project's documentation.
pub const DEFAULT_ALLOWED_HOSTS: &[&str] = &[
	// JavaScript: npm, pnpm and bun (seen); yarn 1 (seen); corepack fetching
	// yarn (seen); JSR.
	"registry.npmjs.org",
	"registry.yarnpkg.com",
	"repo.yarnpkg.com",
	"jsr.io",
	"npm.jsr.io",
	// Python: pip and uv (seen); uv's Python builds (seen); conda-forge.
	"pypi.org",
	"files.pythonhosted.org",
	"releases.astral.sh",
	"conda.anaconda.org",
	// Rust: cargo's sparse index and downloads; rustup toolchains.
	"crates.io",
	"index.crates.io",
	"static.crates.io",
	"static.rust-lang.org",
	// Go: module proxy and checksum database, which also serve toolchains.
	"proxy.golang.org",
	"sum.golang.org",
	// Java: Maven Central, the Gradle plugin portal and Gradle distributions.
	"repo.maven.apache.org",
	"repo1.maven.org",
	"plugins.gradle.org",
	"services.gradle.org",
	"downloads.gradle.org",
	// Ruby, PHP (Composer metadata; packages come from GitHub), .NET, Dart,
	// Elixir and R.
	"rubygems.org",
	"index.rubygems.org",
	"repo.packagist.org",
	"packagist.org",
	"getcomposer.org",
	"api.nuget.org",
	"pub.dev",
	"hex.pm",
	"repo.hex.pm",
	"builds.hex.pm",
	"cloud.r-project.org",
	// GitHub, for sources and release downloads.
	"github.com",
	"api.github.com",
	"codeload.github.com",
	"objects.githubusercontent.com",
	"release-assets.githubusercontent.com",
	// OS packages: Debian (seen), Ubuntu including arm64 ports, Alpine.
	"deb.debian.org",
	"security.debian.org",
	"archive.ubuntu.com",
	"security.ubuntu.com",
	"ports.ubuntu.com",
	"dl-cdn.alpinelinux.org",
	// Anonymous container pulls.
	"registry-1.docker.io",
	"auth.docker.io",
	"production.cloudflare.docker.com",
	"ghcr.io",
	"pkg-containers.githubusercontent.com",
	"quay.io",
	"cdn.quay.io",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedHosts {
	pub hosts: Vec<String>,
	pub allow_all: bool,
}
impl Default for AllowedHosts {
	fn default() -> Self {
		Self {
			hosts: DEFAULT_ALLOWED_HOSTS
				.iter()
				.map(|host| (*host).to_owned())
				.collect(),
			allow_all: false,
		}
	}
}
impl AllowedHosts {
	pub fn permits(&self, host: &str, operator_allows_all: bool) -> bool {
		if host.is_empty() || !host.is_ascii() {
			return false;
		}
		if self.allow_all && operator_allows_all {
			return true;
		}
		let host = host.to_ascii_lowercase();
		self.hosts.iter().any(|pattern| {
			let pattern = pattern.to_ascii_lowercase();
			if let Some(suffix) = pattern.strip_prefix("*.") {
				!suffix.is_empty()
					&& host
						.strip_suffix(suffix)
						.is_some_and(|prefix| prefix.ends_with('.') && prefix.len() > 1)
			} else {
				host == pattern
			}
		})
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DailyBudget {
	pub sent_bytes: u64,
	pub received_bytes: u64,
}
impl Default for DailyBudget {
	fn default() -> Self {
		Self {
			sent_bytes: 100 * (1 << 30),
			received_bytes: 100 * (1 << 30),
		}
	}
}

/// When a scope that may reach any host reaches more distinct hosts in a minute
/// than this, the operator is told; nothing is stopped (#845). Installs touch a
/// handful of hosts, a scan many. Under a host list every other host is refused
/// one connection at a time, so the rule has nothing to add there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnomalyRule {
	pub distinct_hosts_per_minute: u32,
}
impl Default for AnomalyRule {
	fn default() -> Self {
		Self {
			distinct_hosts_per_minute: 100,
		}
	}
}

pub const DISCLOSURE: &str = "Sandboxed code may send this External service anything it can read, including Tenant data in the Working Area. Individual requests are not approved. Aidash's zero data retention and Usage Record guarantees do not apply to these calls.";
pub const UNAVAILABLE: &str = "EGRESS_UNAVAILABLE";
/// A budget counter reached its limit: the gateway stops the stream.
pub const LIMIT_EXCEEDED: &str = "egress_limit_exceeded";
/// The operation token expired or its operation is missing, or the Live preview ended or expired.
pub const OPERATION_EXPIRED: &str = "operation_expired";
/// The Run or operation is no longer active.
pub const OPERATION_INACTIVE: &str = "operation_inactive";

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

	/// Records body bytes that already crossed the gateway. Unlike
	/// [`Self::debit`] it never refuses, so a counter always holds what was
	/// transferred, saturating at `u64::MAX`. The flag reports whether this
	/// debit added bytes and left its counter at or above the limit; the caller
	/// then stops the stream, so a budget is exceeded by at most one debit.
	/// A request is still refused before it starts with [`Self::debit`].
	pub fn record(self, limits: Limits, debit: Debit) -> (Self, bool) {
		let mut next = self;
		let exhausted = match debit {
			Debit::Sent(bytes) => {
				next.sent_bytes = next.sent_bytes.saturating_add(bytes);
				bytes > 0 && next.sent_bytes >= limits.sent_bytes
			}
			Debit::Received(bytes) => {
				next.received_bytes = next.received_bytes.saturating_add(bytes);
				bytes > 0 && next.received_bytes >= limits.received_bytes
			}
			Debit::Request => {
				next.requests = next.requests.saturating_add(1);
				next.requests > limits.requests
			}
		};
		(next, exhausted)
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
	/// The gateway's admission call, in milliseconds. Absent until the outcome
	/// is settled.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub admission_ms: Option<i64>,
	/// From the end of the response until its final debit settled, in
	/// milliseconds. Absent until the outcome is settled.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub settle_ms: Option<i64>,
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
