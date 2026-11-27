#[rstest::rstest]
fn oidc_public_origin_uses_canonical_origin_serialization() {
	let origin = reqwest::Url::parse("https://example.com:443/").unwrap();
	assert_eq!(origin.origin().ascii_serialization(), "https://example.com");
}

/// A coordinator derives the same seed for the same manifest, so a
/// resubmission after a crash matches the preflights it already sent; another
/// Deployment's key, or another manifest, derives another (ADR 0151).
#[rstest::rstest]
fn a_transaction_seed_is_stable_per_manifest_and_secret() {
	use crate::apps::federation::transactions::services::seed_with;
	let manifest: aidash_domain::transactions::Manifest =
		serde_json::from_value(serde_json::json!({
			"id": "00000000-0000-0000-0000-000000000007",
			"coordinator": "aidash://alpha/airy-hawk-2lu4as22",
			"isolation": "serializable",
			"deadline": "2030-01-01T00:00:00Z",
			"participants": [{"node_id": "aidash://alpha/airy-hawk-2lu4as22", "mutations": []}]
		}))
		.unwrap();
	let key = super::transaction_seed_key("isolated-settings-test-secret-0123456789").unwrap();
	let seed = seed_with(&key, &manifest).unwrap();
	assert_eq!(seed.len(), 64);
	assert_eq!(seed, seed_with(&key, &manifest).unwrap());
	let other = super::transaction_seed_key("another-settings-test-secret-0123456789").unwrap();
	assert_ne!(seed, seed_with(&other, &manifest).unwrap());
	let mut changed = manifest.clone();
	changed.deadline += chrono::Duration::seconds(1);
	assert_ne!(seed, seed_with(&key, &changed).unwrap());
}
