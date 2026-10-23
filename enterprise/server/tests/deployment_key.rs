//! The Enterprise build installs the Cloud KMS Deployment Key (ADR 0064).
use aidash_enterprise_server::Adapters;
use aidash_server::bootstrap::build::Extension;
use rstest::rstest;

#[rstest]
#[case::empty("")]
#[case::not_a_version("projects/p/locations/l/keyRings/r/cryptoKeys/k")]
#[case::latest("projects/p/locations/l/keyRings/r/cryptoKeys/k/cryptoKeyVersions/latest")]
#[case::traversal("projects/../locations/l/keyRings/r/cryptoKeys/k/cryptoKeyVersions/1")]
#[tokio::test]
async fn a_malformed_key_version_is_refused_before_any_call_to_kms(#[case] version: &str) {
	let error = Adapters.deployment_key(version).await.err().unwrap();
	assert!(
		error
			.to_string()
			.contains("the Deployment Key in Cloud KMS is unavailable"),
		"{error}"
	);
}
