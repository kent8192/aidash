use crate::{DesktopState, profiles::Profile};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::time::{Duration, Instant};
use tokio::{
	io::{AsyncReadExt, AsyncWriteExt},
	net::TcpListener,
};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

#[derive(Clone, Serialize)]
pub struct Access {
	pub access_token: String,
	pub expires_in: u64,
}
#[derive(Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct Tokens {
	pub access_token: String,
	pub refresh_token: String,
	pub expires_in: u64,
}
pub struct Cached {
	pub token: Zeroizing<String>,
	pub expires: Instant,
}
impl Cached {
	pub fn from_tokens(tokens: &Tokens) -> Self {
		Self {
			token: Zeroizing::new(tokens.access_token.clone()),
			expires: Instant::now() + Duration::from_secs(tokens.expires_in.saturating_sub(15)),
		}
	}
	pub fn access(&self) -> Access {
		Access {
			access_token: self.token.to_string(),
			expires_in: self
				.expires
				.saturating_duration_since(Instant::now())
				.as_secs(),
		}
	}
}
pub fn secret() -> String {
	format!(
		"{}{}",
		uuid::Uuid::new_v4().simple(),
		uuid::Uuid::new_v4().simple()
	)
}
pub fn client() -> Result<Client, String> {
	Client::builder()
		.redirect(reqwest::redirect::Policy::none())
		.timeout(Duration::from_secs(20))
		.build()
		.map_err(|_| "Cannot initialize secure HTTP".into())
}
/// Native network access is limited to these fixed authentication endpoints.
#[derive(Clone, Copy)]
pub enum Endpoint {
	Config,
	Start,
	Exchange,
	Refresh,
	Revoke,
}
impl Endpoint {
	fn path(self) -> &'static str {
		match self {
			Self::Config => "/auth/config",
			Self::Start => "/auth/desktop/start",
			Self::Exchange => "/auth/desktop/exchange",
			Self::Refresh => "/auth/desktop/refresh",
			Self::Revoke => "/auth/desktop/revoke",
		}
	}
}
pub enum Failure {
	Unauthorized,
	Other(String),
}
impl Failure {
	pub fn message(self) -> String {
		match self {
			Self::Unauthorized => {
				"Your saved Aidash session has expired or was revoked. Sign in again.".into()
			}
			Self::Other(message) => message,
		}
	}
}
pub async fn request<T: DeserializeOwned>(
	client: &Client,
	profile: &Profile,
	endpoint: Endpoint,
	body: Option<serde_json::Value>,
) -> Result<T, Failure> {
	let url = format!("{}{}", profile.origin, endpoint.path());
	let request = match body {
		Some(body) => client.post(url).json(&body),
		None => client.get(url),
	};
	let mut response = request.send().await.map_err(|_| {
		Failure::Other("Cannot reach Aidash. Check the connection and retry.".into())
	})?;
	if response.status() == reqwest::StatusCode::UNAUTHORIZED {
		return Err(Failure::Unauthorized);
	}
	if !response.status().is_success() {
		return Err(Failure::Other(format!(
			"Aidash rejected the authentication request ({})",
			response.status().as_u16()
		)));
	}
	let mut bytes = Zeroizing::new(Vec::new());
	while let Some(chunk) = response
		.chunk()
		.await
		.map_err(|_| Failure::Other("Incomplete Aidash response; retry the operation".into()))?
	{
		if bytes.len() + chunk.len() > 65536 {
			return Err(Failure::Other(
				"Aidash authentication response is too large".into(),
			));
		}
		bytes.extend_from_slice(&chunk);
	}
	// Revocation returns 204, represented to this internal caller as null.
	if bytes.is_empty() {
		bytes.extend_from_slice(b"null");
	}
	serde_json::from_slice(&bytes).map_err(|_| {
		Failure::Other(
			"Aidash returned an incompatible authentication response. Upgrade the server.".into(),
		)
	})
}
#[derive(Deserialize)]
pub struct Config {
	pub enabled: bool,
	pub desktop_protocol: Option<u32>,
}
#[derive(Deserialize)]
pub struct Started {
	pub authorization_url: String,
}

pub fn authorization_url(value: &str, profile: &Profile) -> Result<Url, String> {
	let url = Url::parse(value).map_err(|_| "Invalid sign-in URL")?;
	let pairs = url.query_pairs().collect::<Vec<_>>();
	if url.origin().ascii_serialization() != profile.origin
		|| url.path() != "/auth/desktop/authorize"
		|| !url.username().is_empty()
		|| url.password().is_some()
		|| url.fragment().is_some()
		|| pairs.len() != 1
		|| pairs[0].0 != "request"
		|| uuid::Uuid::parse_str(&pairs[0].1).is_err()
	{
		return Err("The server sign-in URL does not match this connection. Use the server's configured public origin.".into());
	}
	Ok(url)
}
fn callback(request: &str, address: &str, expected_state: &str) -> Option<String> {
	let mut lines = request.split("\r\n");
	let mut first = lines.next()?.split_whitespace();
	if first.next()? != "GET" {
		return None;
	}
	let path = first.next()?;
	if first.next()? != "HTTP/1.1" || first.next().is_some() || !path.starts_with("/callback?") {
		return None;
	}
	let hosts = lines
		.filter_map(|line| line.split_once(':'))
		.filter(|(name, _)| name.eq_ignore_ascii_case("host"))
		.map(|(_, v)| v.trim())
		.collect::<Vec<_>>();
	if hosts != [address] {
		return None;
	}
	let url = Url::parse(&format!("http://{address}{path}")).ok()?;
	let pairs = url.query_pairs().collect::<Vec<_>>();
	if pairs.len() != 2
		|| pairs
			.iter()
			.filter(|(k, v)| k == "state" && v == expected_state)
			.count() != 1
	{
		return None;
	}
	let code = pairs.iter().find(|(k, _)| k == "code")?.1.as_ref();
	if !(43..=128).contains(&code.len())
		|| !code
			.bytes()
			.all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
	{
		return None;
	}
	Some(code.to_string())
}
pub async fn receive(
	listener: TcpListener,
	state: &DesktopState,
	generation: u64,
	expected_state: &str,
) -> Result<String, String> {
	let address = listener
		.local_addr()
		.map_err(|_| "Cannot read callback address")?
		.to_string();
	let listen = async {
		loop {
			state.current(generation)?;
			let (mut socket, _) = listener
				.accept()
				.await
				.map_err(|_| "Cannot receive browser sign-in")?;
			let exchange = async {
				let mut data = Zeroizing::new(Vec::new());
				let mut chunk = [0; 1024];
				loop {
					let n = socket
						.read(&mut chunk)
						.await
						.map_err(|_| "Cannot read browser callback")?;
					if n == 0 || data.len() + n > 8192 {
						return Ok::<_, String>(None);
					}
					data.extend_from_slice(&chunk[..n]);
					if data.windows(4).any(|w| w == b"\r\n\r\n") {
						break;
					}
				}
				let code = std::str::from_utf8(&data)
					.ok()
					.and_then(|request| callback(request, &address, expected_state));
				let (status, body) = if code.is_some() {
					("200 OK", "Sign-in received. Return to Aidash Desktop.")
				} else {
					("400 Bad Request", "Invalid sign-in callback.")
				};
				let response = format!(
					"HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; frame-ancestors 'none'\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
					body.len()
				);
				socket
					.write_all(response.as_bytes())
					.await
					.map_err(|_| "Cannot acknowledge callback")?;
				Ok(code)
			};
			if let Ok(Ok(Some(code))) = tokio::time::timeout(Duration::from_secs(5), exchange).await
			{
				return Ok(code);
			}
		}
	};
	let cancelled = state.cancel.notified();
	tokio::pin!(cancelled);
	cancelled.as_mut().enable();
	state.current(generation)?;
	tokio::select! {
		result = tokio::time::timeout(Duration::from_secs(300),listen) => result.map_err(|_| "Browser sign-in timed out. Please retry.".to_string())?,
		_ = cancelled => Err("Sign-in cancelled by connection change".into()),
	}
}
pub fn challenge(verifier: &str) -> String {
	URL_SAFE_NO_PAD.encode(Sha256::digest(verifier))
}
#[cfg(test)]
mod tests {
	use super::*;
	#[test]
	fn callback_is_bound_to_state_host_and_single_values() {
		let code = "a".repeat(64);
		let valid = format!(
			"GET /callback?code={code}&state=expected HTTP/1.1\r\nHost: 127.0.0.1:3210\r\n\r\n"
		);
		assert_eq!(callback(&valid, "127.0.0.1:3210", "expected"), Some(code));
		assert!(callback(&valid, "127.0.0.1:3210", "other").is_none());
		assert!(callback(&valid, "127.0.0.1:3211", "expected").is_none());
		assert!(
			callback(
				&valid.replace("&state=", "&code=duplicate&state="),
				"127.0.0.1:3210",
				"expected"
			)
			.is_none()
		);
	}
	#[test]
	fn opener_cannot_launch_arbitrary_urls() {
		let profile = Profile {
			id: "test".into(),
			name: "test".into(),
			origin: "https://aidash.example".into(),
		};
		let path = format!("/auth/desktop/authorize?request={}", uuid::Uuid::new_v4());
		assert!(authorization_url(&format!("https://aidash.example{path}"), &profile).is_ok());
		assert!(authorization_url(&format!("https://evil.example{path}"), &profile).is_err());
		assert!(authorization_url("file:///tmp/test", &profile).is_err());
	}
}
