use super::{Refusal, closed_keys, required_string};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use url::{Host, Url};

pub(in crate::agent_plugin) const KEYS: [&str; 3] = ["type", "url", "headers"];

/// Aidash's public-address rule (#376) is applied later, in the application layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RemoteServer {
	#[serde(with = "url_text")]
	#[schemars(with = "String")]
	pub url: Url,
	#[serde(default)]
	pub headers: Vec<Header>,
}

/// The name keeps the casing the plugin wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Header {
	pub name: String,
	pub value: String,
}

mod url_text {
	use serde::{Deserialize, Deserializer, Serializer, de::Error};
	use url::Url;

	pub fn serialize<S: Serializer>(url: &Url, serializer: S) -> Result<S::Ok, S::Error> {
		serializer.serialize_str(url.as_str())
	}

	pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Url, D::Error> {
		Url::parse(&String::deserialize(deserializer)?).map_err(D::Error::custom)
	}
}

/// Validates a `streamable-http` or `sse` entry; both have the same shape.
pub(super) fn parse(object: &Map<String, Value>) -> Result<super::McpServer, Refusal> {
	closed_keys(object, &KEYS)?;
	let url = endpoint(required_string(object, "url")?)?;
	let headers = match object.get("headers") {
		None => Vec::new(),
		Some(value) => headers(value)?,
	};
	Ok(super::McpServer::Remote(RemoteServer { url, headers }))
}

/// An absolute HTTP(S) URL without user information or fragment; plain HTTP only for loopback.
fn endpoint(raw: &str) -> Result<Url, Refusal> {
	let invalid = |detail: &str| Refusal::invalid(format!("url {detail}"));
	if raw.chars().any(|c| c.is_whitespace() || c.is_control()) {
		return Err(invalid("must not contain whitespace or control characters"));
	}
	let url = Url::parse(raw).map_err(|_| invalid("must be an absolute URL"))?;
	if !matches!(url.scheme(), "http" | "https") {
		return Err(invalid("must use http or https"));
	}
	if has_user_information(raw, &url) {
		return Err(invalid("must not contain user information"));
	}
	if url.fragment().is_some() {
		return Err(invalid("must not contain a fragment"));
	}
	let loopback = match url.host() {
		Some(Host::Domain(domain)) => domain == "localhost",
		Some(Host::Ipv4(address)) => address.is_loopback(),
		Some(Host::Ipv6(address)) => address.to_canonical().is_loopback(),
		None => return Err(invalid("must have a host")),
	};
	if url.scheme() == "http" && !loopback {
		return Err(invalid("must use https unless the host is loopback"));
	}
	Ok(url)
}

/// `Url` drops empty user information (`https://@host`), so the raw authority is read as well.
fn has_user_information(raw: &str, url: &Url) -> bool {
	let after_scheme = raw.get(url.scheme().len() + 1..).unwrap_or_default();
	let authority = after_scheme
		.trim_start_matches(['/', '\\'])
		.split(['/', '\\', '?', '#'])
		.next()
		.unwrap_or_default();
	authority.contains('@') || !url.username().is_empty() || url.password().is_some()
}

/// RFC 9110 token names and values without control characters other than horizontal tab; the
/// same name in two casings is a duplicate.
fn headers(value: &Value) -> Result<Vec<Header>, Refusal> {
	let invalid = || Refusal::invalid("headers must be an object of valid header fields");
	let members = value.as_object().ok_or_else(invalid)?;
	let mut seen = BTreeSet::new();
	let mut headers = Vec::with_capacity(members.len());
	for (name, value) in members {
		let value = value.as_str().ok_or_else(invalid)?;
		if http::header::HeaderName::from_bytes(name.as_bytes()).is_err()
			|| http::header::HeaderValue::from_str(value).is_err()
			|| !seen.insert(name.to_ascii_lowercase())
		{
			return Err(Refusal::invalid(format!("header {name} is invalid")));
		}
		headers.push(Header {
			name: name.clone(),
			value: value.to_owned(),
		});
	}
	headers.sort_by(|a, b| a.name.cmp(&b.name));
	Ok(headers)
}
