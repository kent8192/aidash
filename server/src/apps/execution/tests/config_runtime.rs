use super::*;
#[rstest::rstest]
fn oidc_public_origin_uses_canonical_origin_serialization() {
	let origin = reqwest::Url::parse("https://example.com:443/").unwrap();
	assert_eq!(origin.origin().ascii_serialization(), "https://example.com");
}

#[rstest::rstest]
fn peer_credentials_reject_short_or_repeated_values() {
	for value in [
		"",
		"short",
		"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
		"abababababababababababababababab",
	] {
		assert!(validate_peer_credential(value).is_err());
	}
	assert!(
		validate_peer_credential(
			"839b16e2f81e28c65ded5407c407305769b28bb14c80a05439d701368994b66c"
		)
		.is_ok()
	);
}
