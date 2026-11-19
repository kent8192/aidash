//! The Web addresses page reading accepts.
use url::Url;

/// The longest address page reading accepts, in bytes.
pub const MAX_URL_BYTES: usize = 4096;

/// The host of an absolute http(s) URL without credentials, fragment or
/// control characters. Whether the host is publicly routable is decided only
/// when a page is read.
pub fn candidate_host(raw: &str) -> Option<String> {
	if raw.len() > MAX_URL_BYTES || raw.chars().any(char::is_control) {
		return None;
	}
	let url = Url::parse(raw).ok()?;
	if !matches!(url.scheme(), "http" | "https")
		|| !url.username().is_empty()
		|| url.password().is_some()
		|| url.fragment().is_some()
	{
		return None;
	}
	url.host_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn only_plain_http_addresses_have_a_candidate_host() {
		assert_eq!(
			candidate_host("https://docs.example.com/a").as_deref(),
			Some("docs.example.com")
		);
		assert_eq!(candidate_host("ftp://example.com/"), None);
		assert_eq!(candidate_host("https://user:pass@example.com/"), None);
		assert_eq!(candidate_host("https://example.com/#top"), None);
		assert_eq!(
			candidate_host(&format!("https://e.com/{}", "a".repeat(MAX_URL_BYTES))),
			None
		);
	}
}
