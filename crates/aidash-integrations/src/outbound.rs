//! Approved outbound HTTP pins every DNS answer and verifies each redirect before I/O.
use crate::{Error, Result};
use aidash_application::ports::capabilities::outbound::{FetchPolicy, OutboundTransport};
use aidash_domain::capabilities::outbound::{permitted_origin, public_ip};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{net::SocketAddr, time::Duration as StdDuration};
pub struct OutboundHttp;
#[async_trait]
impl OutboundTransport for OutboundHttp {
	async fn fetch(
		&self,
		url: &str,
		targets: &Value,
		policy: &FetchPolicy,
	) -> Result<(u16, Vec<u8>, String)> {
		let mut url = url.to_owned();
		for _ in 0..5 {
			let (parsed, origin) = permitted_origin(&url, &policy.origins)?;
			if !targets
				.as_array()
				.is_some_and(|a| a.contains(&json!(origin)))
			{
				return Err(Error::Forbidden);
			}
			let host = parsed.host_str().ok_or(Error::Forbidden)?;
			let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host, 443))
				.await
				.map_err(|error| Error::Port(Box::new(error)))?
				.collect();
			if addresses.is_empty()
				|| addresses.len() > 32
				|| addresses.iter().any(|a| !public_ip(a.ip()))
			{
				return Err(Error::Forbidden);
			}
			let client = reqwest::Client::builder()
				.no_proxy()
				.user_agent(concat!("Aidash/", env!("CARGO_PKG_VERSION")))
				.redirect(reqwest::redirect::Policy::none())
				.timeout(StdDuration::from_secs(10))
				.resolve_to_addrs(host, &addresses)
				.build()
				.map_err(crate::http_error)?;
			let mut response = client
				.get(parsed)
				.header("Accept", "text/plain, application/json;q=0.9, */*;q=0.1")
				.send()
				.await
				.map_err(crate::http_error)?;
			if response.status().is_redirection() {
				let next = response
					.headers()
					.get(reqwest::header::LOCATION)
					.and_then(|h| h.to_str().ok())
					.ok_or_else(|| Error::Invalid("INVALID_REDIRECT".into()))?;
				url = response
					.url()
					.join(next)
					.map_err(|_| Error::Invalid("INVALID_REDIRECT".into()))?
					.to_string();
				continue;
			}
			let status = response.status().as_u16();
			let mut bytes = vec![];
			while let Some(chunk) = response.chunk().await.map_err(crate::http_error)? {
				if (bytes.len() + chunk.len()) as u64 > policy.output_bytes {
					return Err(Error::Invalid("OUTBOUND_RESPONSE_LIMIT".into()));
				}
				bytes.extend_from_slice(&chunk);
			}
			return Ok((status, bytes, url));
		}
		Err(Error::Invalid("OUTBOUND_REDIRECT_LIMIT".into()))
	}
}
#[cfg(test)]
mod tests;
