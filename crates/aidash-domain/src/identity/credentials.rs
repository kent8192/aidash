//! What a caller presents names its Tenant, so authenticating never reads
//! across Tenants (#263).
//!
//! The secret part of a Subject Credential and the id of a Mapping have fixed
//! lengths, so the Tenant name between them and the prefix is parsed from the
//! right: today's Tenant identifiers may contain `_` and `:`.
//!
//! Both travel in HTTP headers, which carry only visible ASCII, while a Tenant
//! name may hold any other character. The name is therefore percent-encoded:
//! every byte outside visible ASCII, and `%` itself, becomes `%XX`, so an
//! ASCII name without `%` reads as itself.
use crate::tenancy::TenantName;
use uuid::Uuid;

pub const SUBJECT_PREFIX: &str = "aidash_subject_";
/// Two simple-format UUIDs.
const SECRET_LENGTH: usize = 64;
/// The longest Subject Credential: the prefix, a Tenant name of 256 bytes all
/// encoded, `_` and the secret.
pub const SUBJECT_MAX_LENGTH: usize = SUBJECT_PREFIX.len() + 3 * 256 + 1 + SECRET_LENGTH;

/// A new Subject Credential for `tenant`.
pub fn subject_token(tenant: &TenantName) -> String {
	format!(
		"{SUBJECT_PREFIX}{}_{}{}",
		encode(tenant),
		Uuid::new_v4().simple(),
		Uuid::new_v4().simple()
	)
}

/// The Tenant name as a header carries it.
fn encode(tenant: &TenantName) -> String {
	let mut encoded = String::with_capacity(tenant.as_str().len());
	for byte in tenant.as_str().bytes() {
		if byte.is_ascii_graphic() && byte != b'%' {
			encoded.push(char::from(byte));
		} else {
			encoded.push_str(&format!("%{byte:02X}"));
		}
	}
	encoded
}

/// The Tenant name an encoded header segment names, or `None` when the
/// segment is not one [`encode`] writes.
fn decode(segment: &str) -> Option<TenantName> {
	let mut bytes = Vec::with_capacity(segment.len());
	let mut rest = segment.as_bytes();
	while let Some((&byte, tail)) = rest.split_first() {
		match byte {
			b'%' => {
				let hex = std::str::from_utf8(tail.get(..2)?).ok()?;
				let value = u8::from_str_radix(hex, 16).ok()?;
				// Only the bytes `encode` escapes are accepted escaped, so each
				// name has one encoding.
				if (value.is_ascii_graphic() && value != b'%') || hex != format!("{value:02X}") {
					return None;
				}
				bytes.push(value);
				rest = &tail[2..];
			}
			byte if byte.is_ascii_graphic() => {
				bytes.push(byte);
				rest = tail;
			}
			_ => return None,
		}
	}
	TenantName::new(String::from_utf8(bytes).ok()?).ok()
}

/// The Tenant a Subject Credential names, or `None` when it is malformed.
pub fn subject_tenant(token: &str) -> Option<TenantName> {
	if token.len() > SUBJECT_MAX_LENGTH {
		return None;
	}
	let rest = token.strip_prefix(SUBJECT_PREFIX)?;
	let split = rest.len().checked_sub(SECRET_LENGTH + 1)?;
	let (tenant, secret) = (rest.get(..split)?, rest.get(split..)?);
	let secret = secret.strip_prefix('_')?;
	if !secret.bytes().all(|b| b.is_ascii_hexdigit()) {
		return None;
	}
	decode(tenant)
}

const MAPPING_PREFIX: &str = "mapping:";

/// The dashboard's context selector for a Mapping.
pub fn mapping_selector(tenant: &TenantName, mapping: Uuid) -> String {
	format!("{MAPPING_PREFIX}{}:{mapping}", encode(tenant))
}

/// The Tenant and Mapping a context selector names, or `None` when it is
/// malformed.
pub fn parse_mapping_selector(selector: &str) -> Option<(TenantName, Uuid)> {
	let rest = selector.strip_prefix(MAPPING_PREFIX)?;
	let (tenant, id) = rest.rsplit_once(':')?;
	let id = Uuid::try_parse(id).ok()?;
	Some((decode(tenant)?, id))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_subject_token_names_its_tenant_even_with_separators() {
		for name in ["acme", "brave-otter-k3m9q2xa", "team_one", "a:b_c"] {
			let tenant = TenantName::new(name).unwrap();
			let token = subject_token(&tenant);
			assert!(token.len() <= SUBJECT_MAX_LENGTH);
			assert_eq!(subject_tenant(&token), Some(tenant));
		}
	}

	#[test]
	fn malformed_subject_tokens_name_no_tenant() {
		let secret = "0".repeat(SECRET_LENGTH);
		for token in [
			String::new(),
			format!("aidash_subject_{secret}"),
			format!("aidash_subject__{secret}"),
			format!("aidash_subject_acme{secret}"),
			format!("aidash_subject_acme_{}", "g".repeat(SECRET_LENGTH)),
			format!("aidash_desktop_acme_{secret}"),
			format!("aidash_subject_{}_{secret}", "x".repeat(257)),
		] {
			assert_eq!(subject_tenant(&token), None, "{token}");
		}
	}

	/// Header values carry only visible ASCII, whatever the Tenant name holds.
	#[test]
	fn credentials_for_any_tenant_name_are_header_safe() {
		let id = Uuid::new_v4();
		for name in ["企業", "50%off", "café:team_1", &"é".repeat(128)] {
			let tenant = TenantName::new(name).unwrap();
			let token = subject_token(&tenant);
			let selector = mapping_selector(&tenant, id);
			for value in [&token, &selector] {
				assert!(value.bytes().all(|b| b.is_ascii_graphic()), "{value}");
			}
			assert!(token.len() <= SUBJECT_MAX_LENGTH);
			assert_eq!(subject_tenant(&token), Some(tenant.clone()));
			assert_eq!(parse_mapping_selector(&selector), Some((tenant, id)));
		}
		assert_eq!(
			mapping_selector(&TenantName::new("企業").unwrap(), id),
			format!("mapping:%E4%BC%81%E6%A5%AD:{id}")
		);
		// Only `encode`'s own output decodes: no other spelling of a name.
		for selector in [
			format!("mapping:%61cme:{id}"),
			format!("mapping:%e4%bc%81%e6%a5%ad:{id}"),
			format!("mapping:%E4%BC:{id}"),
			format!("mapping:acme%:{id}"),
			format!("mapping:ac me:{id}"),
		] {
			assert_eq!(parse_mapping_selector(&selector), None, "{selector}");
		}
	}

	#[test]
	fn a_mapping_selector_round_trips() {
		let id = Uuid::new_v4();
		for name in ["acme", "a:b", "team_one"] {
			let tenant = TenantName::new(name).unwrap();
			assert_eq!(
				parse_mapping_selector(&mapping_selector(&tenant, id)),
				Some((tenant, id))
			);
		}
		for selector in [
			format!("mapping:{id}"),
			"mapping:acme:not-a-uuid".into(),
			format!("deployment_administrator:acme:{id}"),
			format!("mapping::{id}"),
		] {
			assert_eq!(parse_mapping_selector(&selector), None, "{selector}");
		}
	}
}
