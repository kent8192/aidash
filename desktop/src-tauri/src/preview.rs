//! The only URL the desktop opens for a File preview: the Preview gateway's
//! one-time ticket on the configured preview domain.

use reqwest::Url;

const TICKET_PATH: &str = "/__aidash/ticket";

/// Validates a Preview ticket URL before handing it to the system browser.
/// The renderer never gets to open an arbitrary `https:` address.
pub fn validate(url: &str, preview_domain: &str) -> Result<Url, String> {
	let domain = preview_domain
		.trim()
		.trim_end_matches('.')
		.to_ascii_lowercase();
	if domain.is_empty() {
		return Err("This Deployment has no preview domain".into());
	}
	let url = Url::parse(url).map_err(|_| "Invalid preview URL")?;
	if url.scheme() != "https" {
		return Err("Preview URLs must use HTTPS".into());
	}
	if !url.username().is_empty() || url.password().is_some() {
		return Err("Preview URLs must not carry credentials".into());
	}
	let host = url.host_str().ok_or("Preview URL has no host")?;
	let host = host.to_ascii_lowercase();
	let on_domain = host == domain
		|| host
			.strip_suffix(&domain)
			.is_some_and(|prefix| prefix.ends_with('.'));
	if !on_domain {
		return Err("Preview URL is not on the configured preview domain".into());
	}
	if url.path() != TICKET_PATH || url.query().is_none() || url.fragment().is_some() {
		return Err("Preview URL is not a ticket".into());
	}
	Ok(url)
}

#[cfg(test)]
mod tests {
	use super::validate;

	const DOMAIN: &str = "preview.example";

	#[test]
	fn accepts_a_ticket_on_a_subdomain_of_the_preview_domain() {
		let url = "https://abc123.preview.example/__aidash/ticket?t=secret";
		assert_eq!(validate(url, DOMAIN).expect("ticket").as_str(), url);
		assert!(validate(url, " Preview.Example. ").is_ok());
	}

	#[test]
	fn rejects_other_schemes_hosts_paths_and_credentials() {
		for url in [
			"http://abc.preview.example/__aidash/ticket?t=secret",
			"https://abc.preview.example.evil/__aidash/ticket?t=secret",
			"https://notpreview.example/__aidash/ticket?t=secret",
			"https://user:pass@abc.preview.example/__aidash/ticket?t=secret",
			"https://abc.preview.example/other?t=secret",
			"https://abc.preview.example/__aidash/ticket",
			"https://abc.preview.example/__aidash/ticket?t=secret#fragment",
		] {
			assert!(validate(url, DOMAIN).is_err(), "{url}");
		}
	}

	#[test]
	fn rejects_an_empty_preview_domain() {
		assert!(validate("https://abc.preview.example/__aidash/ticket?t=secret", " ").is_err());
	}
}
