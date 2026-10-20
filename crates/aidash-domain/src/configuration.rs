//! Pure validation of configured identities and transport references.
use crate::{Error, Result};

pub fn validate_endpoint(endpoint: &str) -> Result<()> {
	let url =
		url::Url::parse(endpoint).map_err(|_| Error::Invalid("invalid endpoint URL".into()))?;
	if !matches!(url.scheme(), "http" | "https")
		|| url.host_str().is_none()
		|| !url.username().is_empty()
		|| url.password().is_some()
		|| url.query().is_some()
		|| url.fragment().is_some()
	{
		return Err(Error::Invalid(
			"endpoint requires HTTP(S) without inline credentials, query or fragment".into(),
		));
	}
	Ok(())
}

/// Federation bearer credentials must not cross a plaintext network hop.
pub fn validate_peer_endpoint(endpoint: &str) -> Result<()> {
	validate_endpoint(endpoint)?;
	let url = url::Url::parse(endpoint)
		.map_err(|_| Error::Invalid("invalid peer endpoint URL".into()))?;
	if url.scheme() == "https"
		|| matches!(url.host(), Some(url::Host::Ipv4(address)) if address.is_loopback())
		|| matches!(url.host(), Some(url::Host::Ipv6(address)) if address.is_loopback())
	{
		return Ok(());
	}
	Err(Error::Invalid(
		"peer endpoint requires HTTPS or a literal loopback HTTP address".into(),
	))
}

pub fn validate_secret_reference(name: &str) -> Result<()> {
	if !name.starts_with("AIDASH_SECRET_")
		|| !name
			.chars()
			.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
	{
		return Err(Error::Invalid(
			"credential references must use AIDASH_SECRET_* environment variables".into(),
		));
	}
	Ok(())
}
/// Compare fixed-size digests without leaking a matching prefix or secret length.
pub fn same_secret(a: &str, b: &str) -> bool {
	use sha2::{Digest, Sha256};
	let a = Sha256::digest(a.as_bytes());
	let b = Sha256::digest(b.as_bytes());
	a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn validate_peer_credential(value: &str) -> Result<()> {
	if value.len() < 32
		|| !value.bytes().all(|b| b.is_ascii_graphic())
		|| value
			.bytes()
			.collect::<std::collections::HashSet<_>>()
			.len() < 8
	{
		return Err(Error::Invalid("peer credentials require at least 32 ASCII characters and 8 distinct characters; use a randomly generated token".into()));
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::validate_peer_endpoint;

	#[test]
	fn peer_credentials_require_encrypted_or_literal_loopback_transport() {
		for endpoint in [
			"https://peer.example.test:8443",
			"http://127.0.0.1:8000",
			"http://[::1]:8000",
		] {
			assert!(validate_peer_endpoint(endpoint).is_ok(), "{endpoint}");
		}
		for endpoint in [
			"http://peer.example.test",
			"http://localhost:8000",
			"http://192.168.1.10:8000",
			"http://[::ffff:127.0.0.1]:8000",
		] {
			assert!(validate_peer_endpoint(endpoint).is_err(), "{endpoint}");
		}
	}
}
