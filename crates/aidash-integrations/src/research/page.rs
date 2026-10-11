//! Page reading of public HTML and text pages. Every DNS answer is pinned and
//! checked, and each redirect is admitted again before any I/O. Extraction is
//! a bounded tag strip that never executes page content.
use super::admit_public_url;
use crate::research::Failure;
use aidash_application::ports::research::PageProvider;
use aidash_domain::{
	capabilities::outbound::public_ip,
	research::document::{
		Completeness, ExtractedPage, Line, MAX_LINE_BYTES, MAX_LINES, MAX_SNAPSHOT_BYTES,
	},
};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::{StatusCode, Url, header};
use sha2::{Digest, Sha256};
use std::{net::SocketAddr, time::Duration};

const MAX_RAW_BYTES: usize = 4 * 1024 * 1024;
const MAX_REDIRECTS: usize = 3;
const TIMEOUT: Duration = Duration::from_secs(20);
const MEDIA_TYPES: &[&str] = &["text/html", "application/xhtml+xml", "text/plain"];

/// Resolves a host to the addresses a request may connect to.
#[async_trait]
pub trait Resolver: Send + Sync {
	async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, Failure>;
}

/// The system resolver, admitting only publicly routable answers.
pub struct PublicDns;

#[async_trait]
impl Resolver for PublicDns {
	async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, Failure> {
		let addresses = tokio::net::lookup_host((host, port))
			.await
			.map_err(|_| Failure::ProviderRequestRejected)?
			.collect::<Vec<_>>();
		if addresses.is_empty()
			|| addresses.len() > 32
			|| addresses.iter().any(|address| !public_ip(address.ip()))
		{
			return Err(Failure::NonPublicDestination);
		}
		Ok(addresses)
	}
}

pub struct PageReader {
	resolver: Box<dyn Resolver>,
	admit: fn(&Url) -> Result<(), Failure>,
}

impl Default for PageReader {
	fn default() -> Self {
		Self {
			resolver: Box::new(PublicDns),
			admit: admit_public_url,
		}
	}
}

impl PageReader {
	async fn get(&self, url: &Url) -> Result<reqwest::Response, Failure> {
		let host = url.host_str().ok_or(Failure::NonPublicDestination)?;
		let port = url
			.port_or_known_default()
			.ok_or(Failure::NonPublicDestination)?;
		let addresses = self.resolver.resolve(host, port).await?;
		let client = reqwest::Client::builder()
			.no_proxy()
			.user_agent(concat!(
				"aidash/",
				env!("CARGO_PKG_VERSION"),
				" page-reading"
			))
			.redirect(reqwest::redirect::Policy::none())
			.timeout(TIMEOUT)
			.connect_timeout(Duration::from_secs(5))
			.resolve_to_addrs(host, &addresses)
			.build()
			.map_err(|_| Failure::TransportError)?;
		client
			.get(url.clone())
			.header(
				header::ACCEPT,
				"text/html, application/xhtml+xml;q=0.9, text/plain;q=0.8",
			)
			.send()
			.await
			.map_err(|error| {
				if error.is_timeout() {
					Failure::Timeout
				} else if error.is_connect() {
					Failure::ProviderUnavailable
				} else {
					Failure::TransportError
				}
			})
	}
}

#[async_trait]
impl PageProvider for PageReader {
	fn admit(&self, url: &Url) -> Result<(), Failure> {
		(self.admit)(url)
	}
	async fn fetch(&self, url: &Url) -> Result<ExtractedPage, Failure> {
		let mut url = url.clone();
		let operation = async {
			for _ in 0..=MAX_REDIRECTS {
				(self.admit)(&url)?;
				let mut response = self.get(&url).await?;
				if response.status().is_redirection() {
					let location = response
						.headers()
						.get(header::LOCATION)
						.and_then(|value| value.to_str().ok())
						.ok_or(Failure::ProviderResponseInvalid)?;
					url = url
						.join(location)
						.map_err(|_| Failure::ProviderResponseInvalid)?;
					continue;
				}
				if !response.status().is_success() {
					return Err(
						if response.status().is_server_error()
							|| response.status() == StatusCode::TOO_MANY_REQUESTS
						{
							Failure::ProviderUnavailable
						} else {
							Failure::ProviderRequestRejected
						},
					);
				}
				let media_type = response
					.headers()
					.get(header::CONTENT_TYPE)
					.and_then(|value| value.to_str().ok())
					.and_then(|value| value.split(';').next())
					.map(|value| value.trim().to_ascii_lowercase())
					.unwrap_or_else(|| "text/plain".into());
				if !MEDIA_TYPES.contains(&media_type.as_str()) {
					return Err(Failure::UnsupportedMedia);
				}
				let mut bytes = Vec::new();
				while let Some(chunk) = response
					.chunk()
					.await
					.map_err(|_| Failure::ResponseStreamError)?
				{
					if bytes.len().saturating_add(chunk.len()) > MAX_RAW_BYTES {
						return Err(Failure::ResponseTooLarge);
					}
					bytes.extend_from_slice(&chunk);
				}
				return extract(&url, &media_type, &bytes);
			}
			Err(Failure::ProviderRequestRejected)
		};
		tokio::time::timeout(TIMEOUT, operation)
			.await
			.unwrap_or(Err(Failure::Timeout))
	}
}

/// Turn a retrieved body into bounded snapshot lines.
pub fn extract(url: &Url, media_type: &str, bytes: &[u8]) -> Result<ExtractedPage, Failure> {
	let text = String::from_utf8_lossy(bytes);
	let (title, body) = if media_type == "text/plain" {
		(None, text.into_owned())
	} else {
		html_text(&text)
	};
	let mut lines = Vec::new();
	let mut total = 0usize;
	let mut completeness = Completeness::Complete;
	for raw in body.lines() {
		let line = raw
			.chars()
			.map(|c| if c.is_control() && c != '\t' { ' ' } else { c })
			.collect::<String>()
			.split_whitespace()
			.collect::<Vec<_>>()
			.join(" ");
		if line.is_empty() {
			continue;
		}
		let (line, cut) = aidash_domain::research::bounded(&line, MAX_LINE_BYTES);
		if lines.len() == MAX_LINES || total + line.len() > MAX_SNAPSHOT_BYTES {
			completeness = Completeness::TextLimit;
			break;
		}
		if cut {
			completeness = Completeness::TextLimit;
		}
		total += line.len();
		lines.push(Line {
			text: line.to_owned(),
			page: None,
		});
	}
	if lines.is_empty() {
		return Err(Failure::ExtractionFailed);
	}
	Ok(ExtractedPage {
		url: url.to_string(),
		title: title.map(|title| aidash_domain::research::bounded(&title, 512).0.to_owned()),
		media_type: media_type.into(),
		fetched_at: Utc::now(),
		raw_digest: format!("sha256:{:x}", Sha256::digest(bytes)),
		completeness,
		lines,
	})
}

/// Visible text of an HTML document: script, style and template contents are
/// dropped, block elements end lines and character references are decoded.
fn html_text(html: &str) -> (Option<String>, String) {
	const SKIPPED: &[&str] = &["script", "style", "noscript", "template", "svg", "head"];
	const BLOCKS: &[&str] = &[
		"p",
		"div",
		"br",
		"li",
		"ul",
		"ol",
		"tr",
		"h1",
		"h2",
		"h3",
		"h4",
		"h5",
		"h6",
		"section",
		"article",
		"header",
		"footer",
		"pre",
		"blockquote",
		"table",
		"hr",
		"dd",
		"dt",
		"main",
	];
	let mut out = String::new();
	let mut title = None;
	let mut rest = html;
	while let Some(start) = rest.find('<') {
		out.push_str(&decode(&rest[..start]));
		rest = &rest[start..];
		if rest.starts_with("<!--") {
			rest = rest.find("-->").map_or("", |end| &rest[end + 3..]);
			continue;
		}
		let Some(end) = rest.find('>') else {
			break;
		};
		let tag = &rest[1..end];
		rest = &rest[end + 1..];
		let name = tag
			.trim_start_matches('/')
			.split(|c: char| c.is_whitespace() || c == '/')
			.next()
			.unwrap_or_default()
			.to_ascii_lowercase();
		if !tag.starts_with('/') && (name == "title" || SKIPPED.contains(&name.as_str())) {
			let close = format!("</{name}");
			let lower = rest.to_ascii_lowercase();
			let end = lower.find(&close).unwrap_or(rest.len());
			if name == "title" && title.is_none() {
				title = Some(
					decode(&rest[..end])
						.split_whitespace()
						.collect::<Vec<_>>()
						.join(" "),
				);
			}
			// `head` may hold the title; keep scanning it for that only.
			if name == "head" {
				if let Some(found) = html_text(&rest[..end]).0 {
					title.get_or_insert(found);
				}
			}
			rest = &rest[end..];
			rest = rest.find('>').map_or("", |end| &rest[end + 1..]);
			continue;
		}
		if BLOCKS.contains(&name.as_str()) {
			out.push('\n');
		}
	}
	out.push_str(&decode(rest));
	(title.filter(|title| !title.is_empty()), out)
}

fn decode(text: &str) -> String {
	if !text.contains('&') {
		return text.to_owned();
	}
	let mut out = String::with_capacity(text.len());
	let mut rest = text;
	while let Some(start) = rest.find('&') {
		out.push_str(&rest[..start]);
		rest = &rest[start..];
		let end = rest
			.char_indices()
			.take(12)
			.find(|(_, c)| *c == ';')
			.map(|(index, _)| index);
		let decoded = end.and_then(|end| {
			let entity = &rest[1..end];
			let character = match entity {
				"amp" => Some('&'),
				"lt" => Some('<'),
				"gt" => Some('>'),
				"quot" => Some('"'),
				"apos" | "#39" => Some('\''),
				"nbsp" => Some(' '),
				_ => entity
					.strip_prefix("#x")
					.or_else(|| entity.strip_prefix("#X"))
					.and_then(|hex| u32::from_str_radix(hex, 16).ok())
					.or_else(|| entity.strip_prefix('#').and_then(|dec| dec.parse().ok()))
					.and_then(char::from_u32),
			}?;
			Some((character, end))
		});
		match decoded {
			Some((character, end)) => {
				out.push(character);
				rest = &rest[end + 1..];
			}
			None => {
				out.push('&');
				rest = &rest[1..];
			}
		}
	}
	out.push_str(rest);
	out
}

#[cfg(test)]
mod tests;
