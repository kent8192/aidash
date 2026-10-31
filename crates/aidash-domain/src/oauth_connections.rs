//! MCP OAuth Connections (ADR 0100): a person's grant to one MCP server, the
//! Clients that may redeem it, and the pure rules that canonicalize a server
//! resource and admit the scopes a person chooses.
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Why a server resource or a requested scope is refused. The domain's
/// `Invalid` error carries the explanation; the name keeps the contract's
/// signatures explicit at the boundary.
pub type Refusal = crate::Error;

/// A Run whose chain root has no Connection for a server fails with this code
/// so the person can connect.
pub const CONNECTION_REQUIRED: &str = "connection_required";
/// The token was accepted but the server asked for another scope.
pub const CONNECTION_SCOPE: &str = "connection_scope_insufficient";
/// The authorization server or a refresh is unavailable; the Connection is
/// kept and the call may be retried.
pub const CONNECTION_UNAVAILABLE: &str = "connection_unavailable";
/// The executing Node asked the owner's Node for an interactive round instead
/// of a single call.
pub const CONNECTION_REMOTE_REFUSED: &str = "connection_remote_refused";
/// The human request kind that asks a person to connect an MCP server.
pub const CONNECTION_REQUEST_KIND: &str = "MCP_CONNECTION";

/// The canonical RFC 8707 resource indicator of an MCP server: an HTTPS URL,
/// or HTTP only for a literal loopback address, with a lowercase scheme and
/// host, no default port, no query, no fragment, and no path trailing `/`.
pub fn canonical_resource(url: &str) -> Result<String, Refusal> {
	let invalid = |detail: &str| Refusal::Invalid(format!("MCP server URL {detail}"));
	if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
		return Err(invalid("must not contain whitespace or control characters"));
	}
	let parsed = url::Url::parse(url).map_err(|_| invalid("must be an absolute URL"))?;
	if !matches!(parsed.scheme(), "http" | "https") {
		return Err(invalid("must use https, or http for a loopback address"));
	}
	if parsed.host().is_none() {
		return Err(invalid("must have a host"));
	}
	if !parsed.username().is_empty() || parsed.password().is_some() || raw_has_userinfo(url) {
		return Err(invalid("must not contain user information"));
	}
	if parsed.scheme() == "http" && !is_loopback(&parsed) {
		return Err(invalid(
			"may use http only for a literal loopback address; use https otherwise",
		));
	}
	if parsed.query().is_some() {
		return Err(invalid("must not contain a query"));
	}
	if parsed.fragment().is_some() {
		return Err(invalid("must not contain a fragment"));
	}

	let host = parsed
		.host_str()
		.ok_or_else(|| invalid("must have a host"))?;
	let mut canonical = String::with_capacity(url.len());
	canonical.push_str(parsed.scheme());
	canonical.push_str("://");
	canonical.push_str(host);
	if let Some(port) = parsed.port() {
		canonical.push(':');
		canonical.push_str(&port.to_string());
	}
	// `Url` keeps a `/` for an empty path; the canonical form has none.
	canonical.push_str(parsed.path().trim_end_matches('/'));
	Ok(canonical)
}

/// `Url` drops empty user information (`https://@host`), so the raw authority
/// is read as well.
fn raw_has_userinfo(url: &str) -> bool {
	let Some((_, rest)) = url.split_once("://") else {
		return false;
	};
	let authority = rest.split(['/', '\\', '?', '#']).next().unwrap_or_default();
	authority.contains('@')
}

fn is_loopback(url: &url::Url) -> bool {
	matches!(url.host(), Some(url::Host::Ipv4(address)) if address.is_loopback())
		|| matches!(url.host(), Some(url::Host::Ipv6(address)) if address.is_loopback())
}

/// A Connection's lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
	/// The person started the authorization; no usable token is stored yet.
	Pending,
	/// Tokens are stored and a Run whose chain starts at the owner may use them.
	Active,
	/// The refresh token is no longer usable; the owner must connect again.
	NeedsReconnection,
}

/// The OAuth client a Connection was issued to: dynamically registered for this
/// Aidash, or a pre-registered `OAuthClient` the Tenant or person supplied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClientRef {
	Dynamic { client_id: String },
	Registered { client: Uuid },
}

/// One person's MCP OAuth Connection. Tokens live in the Secret Store and never
/// appear here.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OAuthConnection {
	pub id: Uuid,
	pub tenant: String,
	/// The user subject that owns the Connection. Nobody else's Runs use it.
	pub subject: String,
	/// The canonical RFC 8707 resource of the MCP server.
	pub resource: String,
	/// The authorization server's issuer, as its metadata states it.
	pub issuer: String,
	pub client: ClientRef,
	pub scopes: Vec<String>,
	/// Store references are adapter metadata, never included in API projections.
	pub secret_resource: String,
	pub pinned_version: String,
	pub state: ConnectionState,
	pub access_expires_at: Option<DateTime<Utc>>,
	pub refresh_expires_at: Option<DateTime<Utc>>,
	pub created_at: DateTime<Utc>,
	pub updated_at: DateTime<Utc>,
	pub revision: i64,
}

/// A pre-registered OAuth client, bound to one authorization server host and
/// owned by a person (`owner` is their subject) or by the Tenant (`None`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OAuthClient {
	pub id: Uuid,
	pub tenant: String,
	pub owner: Option<String>,
	pub issuer: String,
	pub client_id: String,
	pub secret_resource: Option<String>,
	pub pinned_version: Option<String>,
	pub created_at: DateTime<Utc>,
	pub updated_at: DateTime<Utc>,
	pub revision: i64,
}

impl OAuthClient {
	/// The authorization server's host: the binding the client was registered
	/// for. An issuer without a parseable host yields an empty string.
	pub fn host(&self) -> &str {
		let rest = self
			.issuer
			.split_once("://")
			.map(|(_, rest)| rest)
			.unwrap_or(self.issuer.as_str());
		let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
		let authority = authority
			.rsplit_once('@')
			.map(|(_, host)| host)
			.unwrap_or(authority);
		if authority.starts_with('[') {
			let end = authority
				.find(']')
				.map_or(authority.len(), |index| index + 1);
			&authority[..end]
		} else {
			authority.split(':').next().unwrap_or_default()
		}
	}
}

/// The client a Connection attempt uses: the person's own registration wins
/// over the Tenant's, and dynamic registration is the caller's fallback.
pub fn pick_client<'a>(
	person: Option<&'a OAuthClient>,
	tenant: Option<&'a OAuthClient>,
) -> Option<&'a OAuthClient> {
	person.or(tenant)
}

/// Why a requested scope is refused.
fn invalid_scope(scope: &str) -> Refusal {
	Refusal::Invalid(format!("{scope:?} is not a valid OAuth scope token"))
}

/// RFC 6749 §3.3 scope-token: `%x21 / %x23-5B / %x5D-7E`, at least one character.
fn valid_scope_token(scope: &str) -> bool {
	!scope.is_empty()
		&& scope
			.chars()
			.all(|c| c == '\u{21}' || matches!(c, '\u{23}'..='\u{5b}' | '\u{5d}'..='\u{7e}'))
}

/// The scopes a Connection may request: every requested token is valid and is
/// within every bound that is present (the tool's declaration, the server's
/// `scopes_supported`, and the Tenant's ceiling). The result is sorted and
/// deduplicated.
pub fn admissible_scopes(
	requested: &[String],
	declared: Option<&[String]>,
	supported: Option<&[String]>,
	ceiling: Option<&[String]>,
) -> Result<Vec<String>, Refusal> {
	let mut admitted = Vec::with_capacity(requested.len());
	for scope in requested {
		if !valid_scope_token(scope) {
			return Err(invalid_scope(scope));
		}
		for bound in [declared, supported, ceiling].into_iter().flatten() {
			if !bound.iter().any(|allowed| allowed == scope) {
				return Err(Refusal::Invalid(format!(
					"scope {scope:?} is not permitted by the tool, the server or the Tenant"
				)));
			}
		}
		admitted.push(scope.clone());
	}
	admitted.sort();
	admitted.dedup();
	Ok(admitted)
}

#[cfg(test)]
mod tests;
