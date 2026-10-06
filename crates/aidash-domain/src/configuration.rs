//! Pure validation of configured identities and transport references.
use crate::{Error, Result};

pub fn validate_node_id(id: &str) -> Result<()> {
	let suffix = id.strip_prefix("aidash://").unwrap_or_default();
	if suffix.is_empty()
		|| suffix.len() > 100
		|| !suffix
			.chars()
			.all(|c| c.is_ascii_alphanumeric() || c == '-')
	{
		return Err(Error::Invalid(
			"node id must be aidash:// followed by letters, numbers or hyphens".into(),
		));
	}
	Ok(())
}

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
