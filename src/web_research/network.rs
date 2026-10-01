use super::contracts::MAX_DOWNLOAD;
use crate::{Error, Result, capabilities::network::public_ip};
use reqwest::{Url, header};
use std::{net::SocketAddr, time::Duration};

pub(crate) fn url(value: &str) -> Result<Url> {
	let parsed = Url::parse(value).map_err(|_| Error::Invalid("invalid_public_url".into()))?;
	if value.len() > 4096
		|| value.chars().any(char::is_control)
		|| !matches!(parsed.scheme(), "http" | "https")
		|| parsed.host_str().is_none()
		|| !parsed.username().is_empty()
		|| parsed.password().is_some()
		|| parsed.fragment().is_some()
		|| parsed.port_or_known_default() != Some(if parsed.scheme() == "https" { 443 } else { 80 })
	{
		return Err(Error::Invalid("invalid_public_url".into()));
	}
	for (key, _) in parsed.query_pairs() {
		let key = key.to_ascii_lowercase();
		if key.contains("token")
			|| key.contains("signature")
			|| key.contains("credential")
			|| key.contains("secret")
			|| key.contains("password")
			|| key.contains("authorization")
			|| key.starts_with("x-amz-")
			|| key.starts_with("x-goog-")
			|| matches!(
				key.as_str(),
				"key" | "api_key" | "apikey" | "sig" | "auth" | "access_key"
			) {
			return Err(Error::Invalid("credential_url_forbidden".into()));
		}
	}
	Ok(parsed)
}

pub(crate) async fn client(parsed: &Url, timeout: Duration) -> Result<reqwest::Client> {
	let host = parsed.host_str().ok_or(Error::Forbidden)?;
	let host = host.trim_start_matches('[').trim_end_matches(']');
	let port = parsed.port_or_known_default().ok_or(Error::Forbidden)?;
	let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host, port)).await?.collect();
	if addresses.is_empty() || addresses.len() > 32 || addresses.iter().any(|a| !public_ip(a.ip()))
	{
		return Err(Error::Invalid("non_public_destination".into()));
	}
	Ok(reqwest::Client::builder()
		.no_proxy()
		.retry(reqwest::retry::never())
		.redirect(reqwest::redirect::Policy::none())
		.pool_max_idle_per_host(0)
		.user_agent(concat!(
			"Aidash/",
			env!("CARGO_PKG_VERSION"),
			" web-research"
		))
		.resolve_to_addrs(host, &addresses)
		.timeout(timeout)
		.connect_timeout(timeout.min(Duration::from_secs(5)))
		.build()?)
}
pub(crate) enum Page {
	Redirect(String),
	Retry(u16, Option<String>),
	Body {
		bytes: Vec<u8>,
		media_type: String,
		encoding: String,
	},
	Error(&'static str),
}
pub(crate) struct PageRequest {
	parsed: Url,
	client: reqwest::Client,
}
pub(crate) async fn prepare(value: &str, timeout: Duration) -> Result<PageRequest> {
	let parsed = url(value)?;
	let client = client(&parsed, timeout).await?;
	Ok(PageRequest { parsed, client })
}
pub(crate) async fn fetch(request: PageRequest) -> Result<Page> {
	let PageRequest { parsed, client } = request;
	let mut response = client
		.get(parsed.clone())
		.header(header::ACCEPT, "text/html,text/plain,application/pdf")
		.header(header::ACCEPT_ENCODING, "identity")
		.header(header::CACHE_CONTROL, "no-cache")
		.send()
		.await
		.map_err(|_| Error::External("page_transport_uncertain".into()))?;
	if response.status().is_redirection() {
		let next = response
			.headers()
			.get(header::LOCATION)
			.and_then(|h| h.to_str().ok())
			.ok_or_else(|| Error::Invalid("invalid_redirect".into()))?;
		let next = parsed
			.join(next)
			.map_err(|_| Error::Invalid("invalid_redirect".into()))?;
		url(next.as_str())?;
		if parsed.scheme() == "https" && next.scheme() != "https" {
			return Ok(Page::Error("https_downgrade_forbidden"));
		}
		return Ok(Page::Redirect(next.to_string()));
	}
	let status = response.status().as_u16();
	if matches!(status, 408 | 429 | 500 | 502 | 503 | 504) {
		return Ok(Page::Retry(
			status,
			response
				.headers()
				.get(header::RETRY_AFTER)
				.and_then(|h| h.to_str().ok())
				.map(|s| s.chars().take(128).collect()),
		));
	}
	if !response.status().is_success() {
		return Ok(Page::Error("page_http_error"));
	}
	let media_type = response
		.headers()
		.get(header::CONTENT_TYPE)
		.and_then(|h| h.to_str().ok())
		.unwrap_or("")
		.split(';')
		.next()
		.unwrap_or("")
		.trim()
		.to_ascii_lowercase();
	if !matches!(
		media_type.as_str(),
		"text/plain" | "text/html" | "application/pdf"
	) {
		return Ok(Page::Error("unsupported_media_type"));
	}
	let encoding = response
		.headers()
		.get(header::CONTENT_ENCODING)
		.and_then(|h| h.to_str().ok())
		.unwrap_or("identity")
		.to_ascii_lowercase();
	if !matches!(encoding.as_str(), "identity" | "gzip" | "deflate") {
		return Ok(Page::Error("unsupported_encoding"));
	}
	if response
		.content_length()
		.is_some_and(|n| n > MAX_DOWNLOAD as u64)
	{
		return Ok(Page::Error("download_limit"));
	}
	let mut bytes = vec![];
	while let Some(chunk) = response
		.chunk()
		.await
		.map_err(|_| Error::External("incomplete_download".into()))?
	{
		if bytes.len().saturating_add(chunk.len()) > MAX_DOWNLOAD {
			return Ok(Page::Error("download_limit"));
		}
		bytes.extend_from_slice(&chunk);
	}
	Ok(Page::Body {
		bytes,
		media_type,
		encoding,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	#[tokio::test]
	async fn public_fetch_rejects_local_and_credential_destinations_before_connecting() {
		for address in [
			"http://127.0.0.1/",
			"http://[::1]/",
			"http://169.254.169.254/",
			"http://localhost/",
		] {
			assert!(
				prepare(address, Duration::from_secs(1)).await.is_err(),
				"{address}"
			);
		}
		for address in [
			"https://alice:password@example.com/",
			"https://example.com/?X-Amz-Signature=secret",
			"https://example.com:8443/",
		] {
			assert!(url(address).is_err(), "{address}");
		}
	}
}
