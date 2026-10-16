//! Page reading of public HTML, text and PDF pages. Every DNS answer is pinned
//! and checked, and each redirect is admitted again before any I/O. HTML
//! extraction is a bounded tag strip that never executes page content; a PDF is
//! parsed only by the bounded child of [`super::pdf`].
use super::{
	admit_public_url,
	pdf::{ChildEngine, MAX_PDF_BYTES, PdfEngine, PdfText},
};
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
use encoding_rs::{Encoding, REPLACEMENT, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252};
use reqwest::{StatusCode, Url, header};
use sha2::{Digest, Sha256};
use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

const MAX_RAW_BYTES: usize = 4 * 1024 * 1024;
const MAX_REDIRECTS: usize = 3;
const TIMEOUT: Duration = Duration::from_secs(20);
const MEDIA_TYPES: &[&str] = &["text/html", "application/xhtml+xml", "text/plain"];
const PDF: &str = "application/pdf";
/// A generic binary type is read only when the body turns out to be a PDF.
const OCTET_STREAM: &str = "application/octet-stream";

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
	/// Deadline of one whole read, redirects, body and PDF parsing included.
	timeout: Duration,
	max_raw_bytes: usize,
	max_pdf_bytes: usize,
	pdf: Arc<dyn PdfEngine>,
}

impl Default for PageReader {
	fn default() -> Self {
		Self {
			resolver: Box::new(PublicDns),
			admit: admit_public_url,
			timeout: TIMEOUT,
			max_raw_bytes: MAX_RAW_BYTES,
			max_pdf_bytes: MAX_PDF_BYTES,
			// A binary that cannot name itself fails each PDF read instead.
			pdf: Arc::new(ChildEngine::new(
				std::env::current_exe().unwrap_or_else(|_| PathBuf::from("aidash")),
			)),
		}
	}
}

/// A retrieved body: a page already extracted, or a PDF still to parse.
enum Retrieved {
	Page(ExtractedPage),
	Pdf { url: Url, bytes: Vec<u8> },
}

impl PageReader {
	/// Parse PDFs with `engine` instead of a child of this binary.
	pub fn with_pdf_engine(mut self, engine: Arc<dyn PdfEngine>) -> Self {
		self.pdf = engine;
		self
	}

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
			.timeout(self.timeout)
			.connect_timeout(Duration::from_secs(5))
			.resolve_to_addrs(host, &addresses)
			.build()
			.map_err(|_| Failure::TransportError)?;
		client
			.get(url.clone())
			.header(
				header::ACCEPT,
				"text/html, application/xhtml+xml;q=0.9, text/plain;q=0.8, application/pdf;q=0.8",
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
		let started = tokio::time::Instant::now();
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
					let next = redirect_target(&url, location)?;
					url = next;
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
				let content_type = response
					.headers()
					.get(header::CONTENT_TYPE)
					.and_then(|value| value.to_str().ok())
					.unwrap_or("text/plain");
				let mut parameters = content_type.split(';');
				let media_type = parameters
					.next()
					.map(|value| value.trim().to_ascii_lowercase())
					.unwrap_or_default();
				let text = MEDIA_TYPES.contains(&media_type.as_str());
				if !text && media_type != PDF && media_type != OCTET_STREAM {
					return Err(Failure::UnsupportedMedia);
				}
				let max_bytes = if text {
					self.max_raw_bytes
				} else {
					self.max_pdf_bytes
				};
				let charset = parameters.find_map(|parameter| {
					let (name, value) = parameter.split_once('=')?;
					name.trim()
						.eq_ignore_ascii_case("charset")
						.then(|| value.trim().trim_matches(['"', '\'']).to_owned())
				});
				let mut bytes = Vec::new();
				while let Some(chunk) = response
					.chunk()
					.await
					.map_err(|_| Failure::ResponseStreamError)?
				{
					if bytes.len().saturating_add(chunk.len()) > max_bytes {
						return Err(Failure::ResponseTooLarge);
					}
					bytes.extend_from_slice(&chunk);
				}
				if text {
					return extract(&url, &media_type, charset.as_deref(), &bytes)
						.map(Retrieved::Page);
				}
				if !bytes.starts_with(b"%PDF-") {
					return Err(Failure::UnsupportedMedia);
				}
				return Ok(Retrieved::Pdf {
					url: url.clone(),
					bytes,
				});
			}
			Err(Failure::ProviderRequestRejected)
		};
		let retrieved = tokio::time::timeout(self.timeout, operation)
			.await
			.unwrap_or(Err(Failure::Timeout))?;
		match retrieved {
			Retrieved::Page(page) => Ok(page),
			Retrieved::Pdf { url, bytes } => {
				// Parsing shares the read's deadline. The page was retrieved, so a
				// parse that runs out of time leaves it unreadable rather than its
				// retrieval uncertain.
				let remaining = self.timeout.saturating_sub(started.elapsed());
				let text = tokio::time::timeout(remaining, self.pdf.extract(bytes.clone()))
					.await
					.map_err(|_| Failure::ExtractionFailed)??;
				pdf_page(&url, &bytes, text)
			}
		}
	}
}

/// A parsed PDF as snapshot lines; each line names its physical page. When
/// some page carries no text, the PDF itself travels with the page so that the
/// host can retain it for viewing that page (ADR 0053).
pub fn pdf_page(url: &Url, bytes: &[u8], text: PdfText) -> Result<ExtractedPage, Failure> {
	if text.lines.is_empty() && text.pages_without_text.is_empty() {
		return Err(Failure::ExtractionFailed);
	}
	Ok(ExtractedPage {
		url: url.to_string(),
		title: text.title,
		media_type: PDF.into(),
		fetched_at: Utc::now(),
		raw_digest: format!("sha256:{:x}", Sha256::digest(bytes)),
		completeness: text.completeness,
		lines: text.lines,
		page_count: Some(text.page_count),
		original: (!text.pages_without_text.is_empty()).then(|| bytes.to_vec()),
		pages_without_text: text.pages_without_text,
	})
}

/// The next address of a redirect. The Web disclosure approved one destination
/// host, so another host needs its own request and approval; an encrypted read
/// is never downgraded. A fragment is never sent, so it is dropped: the page
/// is recorded under the address actually fetched.
pub fn redirect_target(current: &Url, location: &str) -> Result<Url, Failure> {
	let mut next = current
		.join(location)
		.map_err(|_| Failure::ProviderResponseInvalid)?;
	if next.host_str() != current.host_str() {
		return Err(Failure::CrossHostRedirect);
	}
	if current.scheme() == "https" && next.scheme() != "https" {
		return Err(Failure::InsecureRedirect);
	}
	next.set_fragment(None);
	Ok(next)
}

/// Turn a retrieved body into bounded snapshot lines. The body is decoded with
/// its declared `charset` (see [`decode_body`]).
pub fn extract(
	url: &Url,
	media_type: &str,
	charset: Option<&str>,
	bytes: &[u8],
) -> Result<ExtractedPage, Failure> {
	let text = decode_body(media_type, charset, bytes)?;
	let (title, body) = if media_type == "text/plain" {
		(None, text)
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
		page_count: None,
		pages_without_text: Vec::new(),
		original: None,
	})
}

/// The text of a body in its Web encoding: a byte order mark first, then the
/// `charset` of the Content-Type, then for HTML a `<meta>` declaration near
/// the start, and otherwise UTF-8 when the bytes are valid UTF-8 or
/// windows-1252, the Web's fallback. A declared encoding the Encoding
/// Standard does not support is refused rather than decoded lossily.
pub fn decode_body(
	media_type: &str,
	charset: Option<&str>,
	bytes: &[u8],
) -> Result<String, Failure> {
	let declared = match charset {
		Some(label) => Some(
			Encoding::for_label(label.as_bytes())
				.filter(|encoding| *encoding != REPLACEMENT)
				.ok_or(Failure::UnsupportedMedia)?,
		),
		None if media_type != "text/plain" => meta_charset(bytes),
		None => None,
	};
	let encoding = declared.unwrap_or(if std::str::from_utf8(bytes).is_ok() {
		UTF_8
	} else {
		WINDOWS_1252
	});
	// `decode` lets a byte order mark override the declaration.
	Ok(encoding.decode(bytes).0.into_owned())
}

/// The encoding a `<meta charset>` or `<meta http-equiv>` declaration in the
/// first 1024 bytes names. As in browsers, a UTF-16 label there means UTF-8.
fn meta_charset(bytes: &[u8]) -> Option<&'static Encoding> {
	let head = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]).to_ascii_lowercase();
	head.split("<meta")
		.skip(1)
		.find_map(|tag| {
			let tag = &tag[..tag.find('>').unwrap_or(tag.len())];
			let value = &tag[tag.find("charset")? + "charset".len()..];
			let value = value
				.trim_start()
				.strip_prefix('=')?
				.trim_start()
				.trim_start_matches(['"', '\'']);
			let end = value
				.find(|c: char| matches!(c, '"' | '\'' | ';' | '/') || c.is_whitespace())
				.unwrap_or(value.len());
			Encoding::for_label(&value.as_bytes()[..end])
		})
		.map(|encoding| {
			if encoding == UTF_16BE || encoding == UTF_16LE {
				UTF_8
			} else {
				encoding
			}
		})
		.filter(|encoding| *encoding != REPLACEMENT)
}

/// Visible text of an HTML document: script, style and template contents are
/// dropped, block elements end lines and character references are decoded.
fn html_text(html: &str) -> (Option<String>, String) {
	const SKIPPED: &[&str] = &["script", "style", "noscript", "template", "svg", "title"];
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
	// ASCII lowercasing keeps byte offsets, so one copy serves every search and
	// the scan stays linear in the document size.
	let lower = html.to_ascii_lowercase();
	let mut out = String::new();
	let mut title = None;
	let mut position = 0;
	while let Some(found) = html[position..].find('<') {
		let start = position + found;
		out.push_str(&decode(&html[position..start]));
		if html[start..].starts_with("<!--") {
			position = html[start..]
				.find("-->")
				.map_or(html.len(), |end| start + end + 3);
			continue;
		}
		let Some(close) = html[start..].find('>') else {
			position = html.len();
			break;
		};
		let tag = &html[start + 1..start + close];
		position = start + close + 1;
		let name = tag
			.trim_start_matches('/')
			.split(|c: char| c.is_whitespace() || c == '/')
			.next()
			.unwrap_or_default()
			.to_ascii_lowercase();
		if !tag.starts_with('/') && SKIPPED.contains(&name.as_str()) {
			let closing = format!("</{name}");
			let end = lower[position..]
				.find(&closing)
				.map_or(html.len(), |end| position + end);
			if name == "title" && title.is_none() {
				title = Some(
					decode(&html[position..end])
						.split_whitespace()
						.collect::<Vec<_>>()
						.join(" "),
				);
			}
			position = html[end..]
				.find('>')
				.map_or(html.len(), |close| end + close + 1);
			continue;
		}
		if BLOCKS.contains(&name.as_str()) {
			out.push('\n');
		}
	}
	out.push_str(&decode(&html[position..]));
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
