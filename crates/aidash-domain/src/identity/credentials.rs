//! What a caller presents names its Tenant, so authenticating never reads
//! across Tenants (#263).
//!
//! The secret part of a Subject Credential and the id of a Mapping have fixed
//! lengths, so the Tenant name between them and the prefix is parsed from the
//! right: today's Tenant identifiers may contain `_` and `:`.
use crate::tenancy::TenantName;
use uuid::Uuid;

pub const SUBJECT_PREFIX: &str = "aidash_subject_";
/// Two simple-format UUIDs.
const SECRET_LENGTH: usize = 64;
/// The longest Subject Credential: the prefix, a Tenant name, `_` and the secret.
pub const SUBJECT_MAX_LENGTH: usize = SUBJECT_PREFIX.len() + 256 + 1 + SECRET_LENGTH;

/// A new Subject Credential for `tenant`.
pub fn subject_token(tenant: &TenantName) -> String {
	format!(
		"{SUBJECT_PREFIX}{tenant}_{}{}",
		Uuid::new_v4().simple(),
		Uuid::new_v4().simple()
	)
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
	TenantName::new(tenant).ok()
}

const MAPPING_PREFIX: &str = "mapping:";

/// The dashboard's context selector for a Mapping.
pub fn mapping_selector(tenant: &TenantName, mapping: Uuid) -> String {
	format!("{MAPPING_PREFIX}{tenant}:{mapping}")
}

/// The Tenant and Mapping a context selector names, or `None` when it is
/// malformed.
pub fn parse_mapping_selector(selector: &str) -> Option<(TenantName, Uuid)> {
	let rest = selector.strip_prefix(MAPPING_PREFIX)?;
	let (tenant, id) = rest.rsplit_once(':')?;
	let id = Uuid::try_parse(id).ok()?;
	Some((TenantName::new(tenant).ok()?, id))
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
			format!("operator:acme:{id}"),
			format!("mapping::{id}"),
		] {
			assert_eq!(parse_mapping_selector(&selector), None, "{selector}");
		}
	}
}
