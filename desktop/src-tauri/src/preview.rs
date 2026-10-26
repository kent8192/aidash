//! The only URL the desktop opens for a File preview: the Preview gateway's
//! one-time ticket on the configured preview domain.

use reqwest::{StatusCode, Url};
use serde::Deserialize;

const TICKET_PATH: &str = "/__aidash/ticket";
const MAX_AVAILABILITY_BYTES: usize = 64 * 1024;

/// The Deployment's trusted Preview configuration. The renderer never supplies
/// or widens this value.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Availability {
	pub available: bool,
	pub preview_domain: Option<String>,
}

/// Reads Preview availability through the desktop's authenticated connection.
pub async fn availability(
	client: &reqwest::Client,
	origin: &str,
	token: &str,
) -> Result<Availability, String> {
	let mut response = client
		.get(format!("{origin}/api/previews/availability"))
		.bearer_auth(token)
		.send()
		.await
		.map_err(|_| "Cannot reach Aidash. Check the connection and retry.".to_string())?;
	if response.status() == StatusCode::UNAUTHORIZED {
		return Err("Your saved Aidash session has expired or was revoked. Sign in again.".into());
	}
	if !response.status().is_success() {
		return Err(format!(
			"Aidash rejected the preview availability request ({})",
			response.status().as_u16()
		));
	}
	let mut bytes = Vec::new();
	while let Some(chunk) = response
		.chunk()
		.await
		.map_err(|_| "Incomplete Aidash response; retry the operation".to_string())?
	{
		if bytes.len() + chunk.len() > MAX_AVAILABILITY_BYTES {
			return Err("Aidash preview availability response is too large".into());
		}
		bytes.extend_from_slice(&chunk);
	}
	serde_json::from_slice(&bytes).map_err(|_| {
		"Aidash returned incompatible preview availability. Upgrade the server.".into()
	})
}

/// The trusted domain, or an error when this Deployment has no Preview domain.
pub fn trusted_domain(availability: &Availability) -> Result<&str, String> {
	if !availability.available {
		return Err("This Deployment has no preview domain".into());
	}
	availability
		.preview_domain
		.as_deref()
		.map(str::trim)
		.filter(|domain| !domain.is_empty())
		.ok_or_else(|| "This Deployment has no preview domain".into())
}

/// Validates a renderer-supplied ticket URL against trusted availability.
pub fn validate_for(availability: &Availability, url: &str) -> Result<Url, String> {
	validate(url, trusted_domain(availability)?)
}

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

	#[test]
	fn renderer_supplied_host_cannot_widen_the_domain() {
		let trusted = super::Availability {
			available: true,
			preview_domain: Some(DOMAIN.into()),
		};
		assert!(
			super::validate_for(
				&trusted,
				"https://abc.preview.example/__aidash/ticket?t=secret"
			)
			.is_ok()
		);
		for url in [
			"https://attacker.example/__aidash/ticket?t=secret",
			"https://attacker-preview.example/__aidash/ticket?t=secret",
		] {
			assert!(super::validate_for(&trusted, url).is_err(), "{url}");
		}
	}

	#[test]
	fn unavailable_availability_has_no_trusted_domain() {
		let unavailable = super::Availability {
			available: false,
			preview_domain: Some(DOMAIN.into()),
		};
		assert!(
			super::validate_for(
				&unavailable,
				"https://abc.preview.example/__aidash/ticket?t=secret"
			)
			.is_err()
		);
	}
}
