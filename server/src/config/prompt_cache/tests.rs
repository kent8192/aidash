use super::*;
use aidash_domain::context::projection::cache_salt_placeholder;
use rstest::{fixture, rstest};

const KEY: &str = "test-only-prompt-cache-key-0123456789abcdef";
const NODE: &str = "aidash://node-a";

fn tenant_scope(tenant: &str) -> PromptCacheScope<'_> {
	PromptCacheScope::Tenant { node: NODE, tenant }
}

#[fixture]
fn key() -> PromptCacheKey {
	PromptCacheKey::new(7, KEY).unwrap()
}

#[rstest]
fn tenants_with_the_same_key_receive_different_salt_lines(key: PromptCacheKey) {
	// Act
	let acme = key.salt_line(tenant_scope("acme")).unwrap();
	let globex = key.salt_line(tenant_scope("globex")).unwrap();
	// Assert
	assert_ne!(acme, globex);
}

#[rstest]
fn equal_tenant_names_on_different_nodes_receive_different_salt_lines(key: PromptCacheKey) {
	// Act
	let node_a = key.salt_line(tenant_scope("acme")).unwrap();
	let node_b = key
		.salt_line(PromptCacheScope::Tenant {
			node: "aidash://node-b",
			tenant: "acme",
		})
		.unwrap();
	// Assert
	assert_ne!(node_a, node_b);
}

#[rstest]
fn node_and_tenant_boundaries_do_not_alias(key: PromptCacheKey) {
	// Act
	let first = key
		.salt_line(PromptCacheScope::Tenant {
			node: "aidash://node-a",
			tenant: "b",
		})
		.unwrap();
	let second = key
		.salt_line(PromptCacheScope::Tenant {
			node: "aidash://node-",
			tenant: "ab",
		})
		.unwrap();
	// Assert
	assert_ne!(first, second);
}

#[rstest]
fn one_tenant_and_key_always_receive_the_same_salt_line(key: PromptCacheKey) {
	// Arrange
	let again = PromptCacheKey::new(7, KEY).unwrap();
	// Act
	let first = key.salt_line(tenant_scope("acme")).unwrap();
	let second = again.salt_line(tenant_scope("acme")).unwrap();
	// Assert
	assert_eq!(first, second);
}

#[rstest]
fn operator_scope_never_matches_a_tenant_named_like_the_node(key: PromptCacheKey) {
	// Act
	let operator = key
		.salt_line(PromptCacheScope::Operator("aidash://node-a"))
		.unwrap();
	let tenant = key.salt_line(tenant_scope("aidash://node-a")).unwrap();
	// Assert
	assert_ne!(operator, tenant);
}

#[rstest]
fn salt_line_carries_the_key_version_and_a_fixed_width(key: PromptCacheKey) {
	// Act
	let line = key.salt_line(tenant_scope("acme")).unwrap();
	// Assert
	assert!(line.starts_with("Cache scope: k00000007."), "{line}");
	assert_eq!(line.len(), cache_salt_placeholder().len());
	assert!(!line.contains(KEY));
	assert!(!line.contains("acme"));
}

#[rstest]
fn rotating_the_key_changes_the_salt_line(key: PromptCacheKey) {
	// Arrange
	let rotated = PromptCacheKey::new(7, "test-only-rotated-cache-key-fedcba9876543210").unwrap();
	// Act and Assert
	assert_ne!(
		key.salt_line(tenant_scope("acme")).unwrap(),
		rotated.salt_line(tenant_scope("acme")).unwrap()
	);
}

#[rstest]
fn missing_key_fails_instead_of_sending_an_unsalted_request() {
	// Act
	let result = salt(None, tenant_scope("acme"));
	// Assert
	assert!(matches!(
		result,
		Err(Error::Invalid(message))
			if message == "Ordered projection requires a configured prompt cache key"
	));
}

#[rstest]
fn configured_key_salts_through_the_shared_entry_point(key: PromptCacheKey) {
	// Act
	let line = salt(Some(&key), tenant_scope("acme")).unwrap();
	// Assert
	assert_eq!(line, key.salt_line(tenant_scope("acme")).unwrap());
}

#[rstest]
fn debug_shows_only_the_version(key: PromptCacheKey) {
	// Act
	let debug = format!("{key:?}");
	// Assert
	assert!(!debug.contains(KEY));
	assert!(debug.contains("version: 7"), "{debug}");
}

#[rstest]
#[case::short("short-key")]
#[case::repeated("abababababababababababababababab")]
#[case::whitespace("contains whitespace 0123456789abcdefghij")]
fn weak_keys_are_rejected_without_echoing_them(#[case] value: &str) {
	// Act
	let error = PromptCacheKey::new(1, value).unwrap_err();
	// Assert
	assert!(!error.to_string().contains(value));
}

#[rstest]
fn version_zero_is_reserved_for_estimates() {
	// Act and Assert
	assert!(PromptCacheKey::new(0, KEY).is_err());
}
