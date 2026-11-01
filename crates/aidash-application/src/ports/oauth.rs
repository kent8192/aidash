//! MCP OAuth ports (ADR 0100): discovery of an MCP server's Protected Resource
//! Metadata and authorization server, dynamic client registration, and token
//! exchange, refresh and revocation. Implementations never log or copy token
//! responses into errors.
use crate::Result;
use crate::plan_connections::RefreshFailure;
use async_trait::async_trait;
use secrecy::SecretString;
use url::Url;

/// RFC 9728 Protected Resource Metadata for an MCP server.
pub struct ProtectedResource {
	/// The resource the metadata describes; it must equal the probed resource.
	pub resource: String,
	pub authorization_servers: Vec<String>,
	pub scopes_supported: Option<Vec<String>>,
}

/// RFC 8414 (or OpenID Discovery) metadata for an authorization server.
#[derive(Clone)]
pub struct AuthorizationServer {
	pub issuer: String,
	pub authorization_endpoint: Url,
	pub token_endpoint: Url,
	pub registration_endpoint: Option<Url>,
	pub revocation_endpoint: Option<Url>,
	pub scopes_supported: Option<Vec<String>>,
	pub code_challenge_methods_supported: Vec<String>,
}

/// A client the authorization server issued: dynamically registered, or
/// pre-registered by a person or the Tenant.
pub struct ClientCredentials {
	pub client_id: String,
	pub client_secret: Option<SecretString>,
}

/// Tokens an authorization server issued. A refresh may omit the refresh token
/// and the scope, which keeps the previous grant.
pub struct Tokens {
	pub access_token: SecretString,
	pub refresh_token: Option<SecretString>,
	pub expires_in: Option<u64>,
	pub scope: Option<String>,
}

/// What an MCP server's response asks the caller to do about authentication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Challenge {
	/// The response carries no Bearer challenge the caller must act on.
	None,
	/// A `401` with a Bearer challenge, or a `401` without one (which still
	/// means the server needs a Connection).
	Unauthorized {
		resource_metadata: Option<Url>,
		scope: Option<String>,
	},
	/// A `403` whose Bearer challenge says `error="insufficient_scope"`.
	InsufficientScope { scope: Option<String> },
}

/// Finds an MCP server's Protected Resource Metadata and authorization server,
/// and registers a client with that server.
#[async_trait]
pub trait OAuthDiscovery: Send + Sync {
	/// RFC 9728: probe `resource` (an unauthenticated MCP `initialize` POST); on
	/// 401 read `WWW-Authenticate: Bearer resource_metadata=...`, else fall back
	/// to `/.well-known/oauth-protected-resource{path}`. `Ok(None)` means the
	/// server needs no OAuth.
	async fn protected_resource(&self, resource: &str) -> Result<Option<ProtectedResource>>;
	/// RFC 8414, then OpenID Discovery as a fallback; the issuer must equal the
	/// metadata's `issuer`.
	async fn authorization_server(&self, issuer: &str) -> Result<AuthorizationServer>;
	/// RFC 7591, requesting `token_endpoint_auth_method` `client_secret_post` and
	/// accepting `none`.
	async fn register(
		&self,
		server: &AuthorizationServer,
		redirect_uri: &Url,
		client_name: &str,
	) -> Result<ClientCredentials>;
}

/// Exchanges, refreshes and revokes tokens at one authorization server.
#[async_trait]
pub trait OAuthTokens: Send + Sync {
	async fn exchange(
		&self,
		server: &AuthorizationServer,
		client: &ClientCredentials,
		code: &str,
		verifier: &SecretString,
		redirect_uri: &Url,
		resource: &str,
	) -> Result<Tokens>;
	async fn refresh(
		&self,
		server: &AuthorizationServer,
		client: &ClientCredentials,
		refresh_token: &SecretString,
		resource: &str,
	) -> std::result::Result<Tokens, RefreshFailure>;
	async fn revoke(
		&self,
		server: &AuthorizationServer,
		client: &ClientCredentials,
		token: &SecretString,
	) -> Result<()>;
}

/// Reads a response's authentication challenge. `401` always means the server
/// needs a Connection; a `403` only becomes [`Challenge::InsufficientScope`]
/// when its Bearer challenge says `error="insufficient_scope"`. Other statuses
/// carry no challenge. A malformed `WWW-Authenticate` never panics and degrades
/// to `Unauthorized { resource_metadata: None, scope: None }` on `401`.
pub fn parse_challenge(status: u16, www_authenticate: Option<&str>) -> Challenge {
	let bearer = www_authenticate.and_then(bearer_parameters);
	match status {
		401 => Challenge::Unauthorized {
			resource_metadata: bearer
				.as_ref()
				.and_then(|params| parameter(params, "resource_metadata"))
				.and_then(|value| Url::parse(&value).ok()),
			scope: bearer
				.as_ref()
				.and_then(|params| parameter(params, "scope")),
		},
		403 => match bearer {
			Some(params)
				if parameter(&params, "error").as_deref() == Some("insufficient_scope") =>
			{
				Challenge::InsufficientScope {
					scope: parameter(&params, "scope"),
				}
			}
			_ => Challenge::None,
		},
		_ => Challenge::None,
	}
}

/// The parameters of the first `Bearer` challenge in the header.
fn bearer_parameters(header: &str) -> Option<Vec<(String, String)>> {
	parse_challenges(header)
		.into_iter()
		.find_map(|(scheme, params)| (scheme == "bearer").then_some(params))
}

fn parameter(params: &[(String, String)], name: &str) -> Option<String> {
	params
		.iter()
		.find(|(key, _)| key == name)
		.map(|(_, value)| value.clone())
		.filter(|value| !value.is_empty())
}

/// RFC 7235 challenges: one or more `scheme [auth-params]`. A comma separates
/// a parameter from the next one or a challenge from the next challenge; a
/// token followed by `=` starts a parameter, anything else starts a scheme.
fn parse_challenges(header: &str) -> Vec<(String, Vec<(String, String)>)> {
	let mut parser = Parser {
		text: header,
		at: 0,
	};
	let mut challenges = Vec::new();
	loop {
		parser.skip_separators();
		if parser.peek().is_none() {
			break;
		}
		let Some(scheme) = parser.token() else {
			break;
		};
		let scheme = scheme.to_ascii_lowercase();
		let mut params = Vec::new();
		loop {
			let before = parser.at;
			parser.skip_whitespace();
			if parser.peek().is_none() {
				break;
			}
			if parser.peek() == Some(',') {
				parser.next();
				parser.skip_whitespace();
			}
			let lookahead = parser.at;
			let Some(name) = parser.token() else {
				break;
			};
			parser.skip_whitespace();
			if parser.peek() != Some('=') {
				parser.at = lookahead;
				break;
			}
			parser.next();
			parser.skip_whitespace();
			let Some(value) = parser.value() else {
				parser.at = before;
				break;
			};
			params.push((name.to_ascii_lowercase(), value));
		}
		challenges.push((scheme, params));
	}
	challenges
}

struct Parser<'a> {
	text: &'a str,
	at: usize,
}

impl Parser<'_> {
	fn peek(&self) -> Option<char> {
		self.text[self.at..].chars().next()
	}

	fn next(&mut self) -> Option<char> {
		let character = self.peek()?;
		self.at += character.len_utf8();
		Some(character)
	}

	fn skip_whitespace(&mut self) {
		while matches!(self.peek(), Some(' ' | '\t')) {
			self.at += 1;
		}
	}

	fn skip_separators(&mut self) {
		loop {
			self.skip_whitespace();
			if self.peek() == Some(',') {
				self.next();
			} else {
				break;
			}
		}
	}

	fn token(&mut self) -> Option<String> {
		let start = self.at;
		while let Some(character) = self.peek() {
			if !is_token_character(character) {
				break;
			}
			self.at += character.len_utf8();
		}
		(self.at > start).then(|| self.text[start..self.at].to_owned())
	}

	fn value(&mut self) -> Option<String> {
		if self.peek() != Some('"') {
			return self.token();
		}
		self.next();
		let mut value = String::new();
		loop {
			match self.next()? {
				'"' => return Some(value),
				'\\' => value.push(self.next()?),
				character => value.push(character),
			}
		}
	}
}

/// RFC 7230 `tchar`.
fn is_token_character(character: char) -> bool {
	character.is_ascii_alphanumeric()
		|| matches!(
			character,
			'!' | '#'
				| '$' | '%' | '&'
				| '\'' | '*' | '+'
				| '-' | '.' | '^'
				| '_' | '`' | '|'
				| '~'
		)
}

/// The Tenant's settings a Connection attempt is bounded by: the OAuth Clients
/// registered in advance for an authorization server, and the Tenant's ceiling
/// on the scopes a Connection to one MCP server may request.
#[async_trait]
pub trait ConnectionPolicy: Send + Sync {
	/// The person's own registered client for `issuer`, then the Tenant's.
	async fn registered_clients(
		&self,
		tenant: &str,
		subject: &str,
		issuer: &str,
	) -> Result<(
		Option<aidash_domain::oauth_connections::OAuthClient>,
		Option<aidash_domain::oauth_connections::OAuthClient>,
	)>;
	/// The Tenant's scope ceiling for a canonical resource, if it set one.
	async fn scope_ceiling(&self, tenant: &str, resource: &str) -> Result<Option<Vec<String>>>;
}

#[cfg(test)]
mod tests;
