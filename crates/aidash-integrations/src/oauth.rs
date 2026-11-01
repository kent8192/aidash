//! MCP authorization over HTTP (ADR 0100): an unauthenticated `initialize`
//! probe (RFC 9728), authorization server metadata (RFC 8414, then OpenID
//! Discovery), dynamic client registration (RFC 7591), and token exchange,
//! refresh and revocation (RFC 6749, RFC 7009, RFC 8707). Token responses and
//! discovery bodies never enter errors or logs; every request uses the
//! public-address-only client, follows no redirects and is bounded in size.
use crate::{Error, Result};
use aidash_application::{
	plan_connections::RefreshFailure,
	ports::oauth::{
		AuthorizationServer, Challenge, ClientCredentials, OAuthDiscovery, OAuthTokens,
		ProtectedResource, Tokens, parse_challenge,
	},
};
use async_trait::async_trait;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::json;
use std::{fmt, net::IpAddr, time::Duration};
use url::Url;

/// One request, including the body, must finish within this.
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Discovery and token bodies are small; anything larger is refused.
const MAX_RESPONSE_BYTES: usize = 64 * 1024;
/// The MCP revision this probe announces.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// The MCP OAuth adapter: discovery, registration and token operations against
/// the authorization server an MCP server points to.
#[derive(Clone)]
pub struct McpOAuth {
	client: reqwest::Client,
	// Only `for_test` can enable non-public destinations.
	allow_private: bool,
}

impl fmt::Debug for McpOAuth {
	/// Prints no client configuration and can never print a token.
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str("McpOAuth")
	}
}

impl McpOAuth {
	/// A client that reaches only public addresses, follows no redirects, and
	/// bounds every request's time.
	pub fn new() -> Result<Self> {
		Ok(Self {
			client: crate::keyless::public_client(IO_TIMEOUT, CONNECT_TIMEOUT)?,
			allow_private: false,
		})
	}

	/// Points the adapter at a fake server on loopback. Compiled only into tests
	/// and `test-support` builds, which no production dependency enables.
	#[cfg(any(test, feature = "test-support"))]
	pub fn for_test() -> Result<Self> {
		let client = reqwest::Client::builder()
			.timeout(IO_TIMEOUT)
			.connect_timeout(CONNECT_TIMEOUT)
			.redirect(reqwest::redirect::Policy::none())
			.no_proxy()
			.build()
			.map_err(crate::http_error)?;
		Ok(Self {
			client,
			allow_private: true,
		})
	}

	/// DNS checks run when connecting; literal addresses skip DNS in reqwest
	/// and must be admitted here. Validate every destination, including port
	/// values supplied directly by a caller rather than through discovery.
	fn request(&self, method: reqwest::Method, url: &Url) -> Result<reqwest::RequestBuilder> {
		if !is_secure(url) {
			return Err(invalid(
				"OAuth endpoint must use HTTPS, or HTTP only for a literal loopback address, without inline credentials or fragments",
			));
		}
		if self.allow_private {
			return Ok(self.client.request(method, url.clone()));
		}
		let ip = match url.host() {
			Some(url::Host::Ipv4(ip)) => Some(IpAddr::V4(ip)),
			Some(url::Host::Ipv6(ip)) => Some(IpAddr::V6(ip)),
			_ => None,
		};
		if ip.is_some_and(|ip| !aidash_domain::capabilities::outbound::public_ip(ip)) {
			return Err(Error::Forbidden);
		}
		Ok(self.client.request(method, url.clone()))
	}

	/// The unauthenticated MCP `initialize` an RFC 9728 probe starts with.
	async fn initialize(&self, endpoint: &Url) -> Result<reqwest::Response> {
		let body = json!({
			"jsonrpc": "2.0",
			"id": 1,
			"method": "initialize",
			"params": {
				"protocolVersion": PROTOCOL_VERSION,
				"capabilities": {},
				"clientInfo": {"name": "aidash", "version": env!("CARGO_PKG_VERSION")},
			},
		});
		self.request(reqwest::Method::POST, endpoint)?
			.timeout(IO_TIMEOUT)
			.header("accept", "application/json, text/event-stream")
			.header("mcp-protocol-version", PROTOCOL_VERSION)
			.json(&body)
			.send()
			.await
			.map_err(send_error)
	}

	async fn get(&self, url: &Url) -> Result<reqwest::Response> {
		self.request(reqwest::Method::GET, url)?
			.timeout(IO_TIMEOUT)
			.header("accept", "application/json")
			.send()
			.await
			.map_err(send_error)
	}

	/// Reads a bounded JSON document. Only fixed messages leave this method.
	async fn document<T: DeserializeOwned>(&self, response: reqwest::Response) -> Result<T> {
		let body = body(response).await?;
		serde_json::from_slice(&body).map_err(|_| malformed())
	}

	/// One Protected Resource Metadata document, or a fixed failure.
	async fn resource_document(&self, url: &Url) -> Result<ProtectedResourceDocument> {
		let response = self.get(url).await?;
		if !response.status().is_success() {
			return Err(external(
				"MCP server did not publish Protected Resource Metadata",
			));
		}
		self.document(response).await
	}

	/// One authorization server's metadata, or `None` when the document is not
	/// there. A document that is there but names another issuer is refused by
	/// the caller, never skipped in favour of the next well-known location.
	async fn authorization_document(
		&self,
		url: &Url,
	) -> Result<Option<AuthorizationServerDocument>> {
		let response = self.get(url).await?;
		if !response.status().is_success() {
			return Ok(None);
		}
		self.document(response).await.map(Some)
	}
}

#[async_trait]
impl OAuthDiscovery for McpOAuth {
	async fn protected_resource(&self, resource: &str) -> Result<Option<ProtectedResource>> {
		let canonical = aidash_domain::oauth_connections::canonical_resource(resource)?;
		let endpoint =
			Url::parse(resource).map_err(|_| invalid("MCP server URL is not an absolute URL"))?;
		let response = self.initialize(&endpoint).await?;
		let status = response.status();
		if status.is_success() {
			return Ok(None);
		}
		if status != reqwest::StatusCode::UNAUTHORIZED {
			return Err(external("MCP server did not answer the initialize probe"));
		}
		let hint = response
			.headers()
			.get_all("www-authenticate")
			.iter()
			.find_map(
				|value| match parse_challenge(status.as_u16(), value.to_str().ok()) {
					Challenge::Unauthorized {
						resource_metadata, ..
					} => resource_metadata,
					_ => None,
				},
			);
		if let Some(url) = hint {
			let url = secure(&url).ok_or_else(|| {
				invalid(
					"resource_metadata must be an HTTPS URL, or HTTP only for a loopback address",
				)
			})?;
			let document = self.resource_document(&url).await?;
			return validate_resource(document, &canonical, self.allow_private).map(Some);
		}
		for url in protected_resource_fallbacks(&endpoint) {
			if let Ok(document) = self.resource_document(&url).await {
				return validate_resource(document, &canonical, self.allow_private).map(Some);
			}
		}
		Err(external(
			"MCP server did not publish Protected Resource Metadata",
		))
	}

	async fn authorization_server(&self, issuer: &str) -> Result<AuthorizationServer> {
		let issuer_url = secure_str(issuer)
			.ok_or_else(|| invalid("authorization server must be an HTTPS URL"))?;
		if issuer_url.query().is_some() || issuer_url.fragment().is_some() {
			return Err(invalid(
				"authorization server issuer must not contain a query or fragment",
			));
		}
		for url in [rfc8414_url(&issuer_url), openid_url(&issuer_url)] {
			if let Some(document) = self.authorization_document(&url).await? {
				return validate_authorization(document, issuer);
			}
		}
		Err(external("authorization server metadata is unavailable"))
	}

	async fn register(
		&self,
		server: &AuthorizationServer,
		redirect_uri: &Url,
		client_name: &str,
	) -> Result<ClientCredentials> {
		let endpoint = server.registration_endpoint.as_ref().ok_or_else(|| {
			invalid("authorization server does not support dynamic client registration")
		})?;
		let response = self
			.request(reqwest::Method::POST, endpoint)?
			.timeout(IO_TIMEOUT)
			.header("accept", "application/json")
			.json(&json!({
				"client_name": client_name,
				"redirect_uris": [redirect_uri.as_str()],
				"grant_types": ["authorization_code", "refresh_token"],
				"response_types": ["code"],
				"token_endpoint_auth_method": "client_secret_post",
			}))
			.send()
			.await
			.map_err(send_error)?;
		if !response.status().is_success() {
			return Err(external(
				"authorization server refused the dynamic client registration",
			));
		}
		let registered: RegistrationResponse = self.document(response).await?;
		if registered.client_id.trim().is_empty() {
			return Err(malformed());
		}
		Ok(ClientCredentials {
			client_id: registered.client_id,
			client_secret: registered.client_secret.map(SecretString::from),
		})
	}
}

#[async_trait]
impl OAuthTokens for McpOAuth {
	async fn exchange(
		&self,
		server: &AuthorizationServer,
		client: &ClientCredentials,
		code: &str,
		verifier: &SecretString,
		redirect_uri: &Url,
		resource: &str,
	) -> Result<Tokens> {
		let form = token_form(
			[
				("grant_type", "authorization_code"),
				("code", code),
				("redirect_uri", redirect_uri.as_str()),
				("code_verifier", verifier.expose_secret()),
				("resource", resource),
			],
			client,
		);
		let response = self
			.request(reqwest::Method::POST, &server.token_endpoint)?
			.timeout(IO_TIMEOUT)
			.header("accept", "application/json")
			.form(&form)
			.send()
			.await
			.map_err(send_error)?;
		if !response.status().is_success() {
			return Err(if response.status().is_client_error() {
				invalid("authorization server rejected the authorization code")
			} else {
				external("authorization server is unavailable")
			});
		}
		let body = body(response).await?;
		tokens(&body).ok_or_else(malformed)
	}

	async fn refresh(
		&self,
		server: &AuthorizationServer,
		client: &ClientCredentials,
		refresh_token: &SecretString,
		resource: &str,
	) -> std::result::Result<Tokens, RefreshFailure> {
		let form = token_form(
			[
				("grant_type", "refresh_token"),
				("refresh_token", refresh_token.expose_secret()),
				("resource", resource),
			],
			client,
		);
		let response = self
			.request(reqwest::Method::POST, &server.token_endpoint)
			.map_err(RefreshFailure::Unavailable)?
			.timeout(IO_TIMEOUT)
			.header("accept", "application/json")
			.form(&form)
			.send()
			.await;
		let response = match response {
			Ok(response) => response,
			// A refused destination never reached the server.
			Err(error) if crate::keyless::refused(&error) => {
				return Err(RefreshFailure::Unavailable(Error::Forbidden));
			}
			// The connection was never established, so nothing was sent.
			Err(error) if error.is_connect() => {
				return Err(RefreshFailure::Unavailable(external(
					"authorization server is unavailable",
				)));
			}
			// A timeout or a broken transfer after sending may hide a rotated grant.
			Err(_) => {
				return Err(RefreshFailure::Ambiguous(external(
					"the token refresh outcome is unknown",
				)));
			}
		};
		if response.status().is_success() {
			let body = match body(response).await {
				Ok(body) => body,
				Err(_) => {
					return Err(RefreshFailure::Ambiguous(external(
						"the token refresh outcome is unknown",
					)));
				}
			};
			return match tokens(&body) {
				Some(tokens) => Ok(tokens),
				None => Err(RefreshFailure::Ambiguous(external(
					"the token refresh outcome is unknown",
				))),
			};
		}
		if response.status() == reqwest::StatusCode::BAD_REQUEST {
			// Only `invalid_grant` ends the grant. A body that cannot be read is
			// not proof of it, so the tokens are kept.
			let code = body(response)
				.await
				.ok()
				.and_then(|body| serde_json::from_slice::<OAuthError>(&body).ok())
				.and_then(|error| error.error);
			if code.as_deref() == Some("invalid_grant") {
				return Err(RefreshFailure::Terminal);
			}
		}
		Err(RefreshFailure::Unavailable(external(
			"authorization server refused the token refresh",
		)))
	}

	async fn revoke(
		&self,
		server: &AuthorizationServer,
		client: &ClientCredentials,
		token: &SecretString,
	) -> Result<()> {
		let Some(endpoint) = &server.revocation_endpoint else {
			return Ok(());
		};
		let form = token_form([("token", token.expose_secret())], client);
		let response = self
			.request(reqwest::Method::POST, endpoint)?
			.timeout(IO_TIMEOUT)
			.header("accept", "application/json")
			.form(&form)
			.send()
			.await
			.map_err(send_error)?;
		if response.status().is_success() {
			return Ok(());
		}
		Err(if response.status().is_client_error() {
			invalid("authorization server refused the token revocation")
		} else {
			external("authorization server is unavailable")
		})
	}
}

/// A token request's form: the grant's own fields, then client authentication.
fn token_form<'a>(
	fields: impl IntoIterator<Item = (&'a str, &'a str)>,
	client: &'a ClientCredentials,
) -> Vec<(&'a str, &'a str)> {
	let mut form: Vec<(&str, &str)> = fields.into_iter().collect();
	form.push(("client_id", client.client_id.as_str()));
	if let Some(secret) = &client.client_secret {
		form.push(("client_secret", secret.expose_secret()));
	}
	form
}

#[derive(Deserialize)]
struct ProtectedResourceDocument {
	resource: String,
	authorization_servers: Vec<String>,
	#[serde(default)]
	scopes_supported: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct AuthorizationServerDocument {
	issuer: String,
	authorization_endpoint: String,
	token_endpoint: String,
	#[serde(default)]
	registration_endpoint: Option<String>,
	#[serde(default)]
	revocation_endpoint: Option<String>,
	#[serde(default)]
	scopes_supported: Option<Vec<String>>,
	#[serde(default)]
	code_challenge_methods_supported: Vec<String>,
}

#[derive(Deserialize)]
struct RegistrationResponse {
	client_id: String,
	#[serde(default)]
	client_secret: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
	access_token: String,
	#[serde(default)]
	refresh_token: Option<String>,
	#[serde(default)]
	expires_in: Option<i64>,
	#[serde(default)]
	scope: Option<String>,
	#[serde(default)]
	token_type: Option<String>,
}

#[derive(Deserialize)]
struct OAuthError {
	error: Option<String>,
}

/// A usable token document, or `None` when a required field is missing or the
/// server issued a token type Aidash cannot send.
fn tokens(body: &[u8]) -> Option<Tokens> {
	let document: TokenResponse = serde_json::from_slice(body).ok()?;
	if document.access_token.is_empty() {
		return None;
	}
	if document
		.token_type
		.as_deref()
		.is_some_and(|kind| !kind.eq_ignore_ascii_case("bearer"))
	{
		return None;
	}
	Some(Tokens {
		access_token: SecretString::from(document.access_token),
		refresh_token: document.refresh_token.map(SecretString::from),
		expires_in: document
			.expires_in
			.and_then(|seconds| u64::try_from(seconds).ok()),
		scope: document.scope,
	})
}

/// The metadata must describe exactly the canonical resource, name at least one
/// authorization server, and point only at secure URLs.
fn validate_resource(
	document: ProtectedResourceDocument,
	canonical: &str,
	allow_private: bool,
) -> Result<ProtectedResource> {
	let described = aidash_domain::oauth_connections::canonical_resource(&document.resource)
		.map_err(|_| invalid("Protected Resource Metadata names an invalid resource"))?;
	if described != canonical {
		return Err(invalid(
			"MCP server's Protected Resource Metadata describes another resource",
		));
	}
	if document.authorization_servers.is_empty() {
		return Err(invalid(
			"Protected Resource Metadata names no authorization server",
		));
	}
	for server in &document.authorization_servers {
		if secure_str(server).is_none_or(|url| url.scheme() != "https" && !allow_private) {
			return Err(invalid(
				"a Protected Resource authorization server must be an HTTPS URL",
			));
		}
	}
	Ok(ProtectedResource {
		resource: canonical.to_owned(),
		authorization_servers: document.authorization_servers,
		scopes_supported: document.scopes_supported,
	})
}

/// The metadata must name the requested issuer, use secure endpoints, and
/// support PKCE `S256` (the MCP specification requires it).
fn validate_authorization(
	document: AuthorizationServerDocument,
	issuer: &str,
) -> Result<AuthorizationServer> {
	if document.issuer != issuer {
		return Err(invalid(
			"authorization server metadata names another issuer",
		));
	}
	let endpoint = |value: &str| {
		secure_str(value).ok_or_else(|| {
			invalid(
				"an authorization server endpoint must be an HTTPS URL, or HTTP only for a loopback address",
			)
		})
	};
	let authorization_endpoint = endpoint(&document.authorization_endpoint)?;
	let token_endpoint = endpoint(&document.token_endpoint)?;
	let registration_endpoint = document
		.registration_endpoint
		.as_deref()
		.map(endpoint)
		.transpose()?;
	let revocation_endpoint = document
		.revocation_endpoint
		.as_deref()
		.map(endpoint)
		.transpose()?;
	if !document
		.code_challenge_methods_supported
		.iter()
		.any(|method| method == "S256")
	{
		return Err(invalid("authorization server does not support PKCE S256"));
	}
	Ok(AuthorizationServer {
		issuer: issuer.to_owned(),
		authorization_endpoint,
		token_endpoint,
		registration_endpoint,
		revocation_endpoint,
		scopes_supported: document.scopes_supported,
		code_challenge_methods_supported: document.code_challenge_methods_supported,
	})
}

/// RFC 8414 §3.1: the well-known segment goes between the host and the issuer's
/// path, so an issuer with a path keeps it after the segment.
fn rfc8414_url(issuer: &Url) -> Url {
	let mut url = issuer.clone();
	url.set_path(&format!(
		"/.well-known/oauth-authorization-server{}",
		issuer.path().trim_end_matches('/')
	));
	url
}

/// OpenID Discovery appends its segment to the issuer.
fn openid_url(issuer: &Url) -> Url {
	let mut url = issuer.clone();
	url.set_path(&format!(
		"{}/.well-known/openid-configuration",
		issuer.path().trim_end_matches('/')
	));
	url
}

/// RFC 9728's fallback: the path-qualified well-known URL first, then the one at
/// the origin. `{origin}` is the resource's scheme, host and port.
fn protected_resource_fallbacks(resource: &Url) -> Vec<Url> {
	let path = resource.path().trim_end_matches('/');
	let mut qualified = resource.clone();
	qualified.set_path(&format!("/.well-known/oauth-protected-resource{path}"));
	qualified.set_query(None);
	qualified.set_fragment(None);
	let mut origin = resource.clone();
	origin.set_path("/.well-known/oauth-protected-resource");
	origin.set_query(None);
	origin.set_fragment(None);
	let mut fallbacks = vec![qualified];
	if origin != fallbacks[0] {
		fallbacks.push(origin);
	}
	fallbacks
}

/// Whether a URL is HTTPS, or HTTP only for a literal loopback address (the
/// rule `canonical_resource` applies), without inline credentials.
fn is_secure(url: &Url) -> bool {
	if !url.username().is_empty()
		|| url.password().is_some()
		|| url.fragment().is_some()
		|| url.host().is_none()
	{
		return false;
	}
	match url.scheme() {
		"https" => true,
		"http" => {
			matches!(url.host(), Some(url::Host::Ipv4(address)) if address.is_loopback())
				|| matches!(url.host(), Some(url::Host::Ipv6(address)) if address.is_loopback())
		}
		_ => false,
	}
}

fn secure(url: &Url) -> Option<Url> {
	is_secure(url).then(|| url.clone())
}

fn secure_str(value: &str) -> Option<Url> {
	let url = Url::parse(value).ok()?;
	is_secure(&url).then_some(url)
}

async fn body(response: reqwest::Response) -> Result<Vec<u8>> {
	crate::response::bytes(response, MAX_RESPONSE_BYTES)
		.await
		.map_err(|_| external("OAuth response could not be read within the 64 KiB limit"))
}

fn send_error(error: reqwest::Error) -> Error {
	if crate::keyless::refused(&error) {
		Error::Forbidden
	} else {
		external("OAuth endpoint is unavailable")
	}
}

fn invalid(message: &str) -> Error {
	Error::Invalid(message.into())
}

fn external(message: &str) -> Error {
	Error::External(message.into())
}

fn malformed() -> Error {
	external("authorization server returned an unexpected response")
}

#[cfg(test)]
mod tests;
