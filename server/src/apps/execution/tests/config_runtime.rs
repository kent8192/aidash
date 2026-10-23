#[rstest::rstest]
fn oidc_public_origin_uses_canonical_origin_serialization() {
	let origin = reqwest::Url::parse("https://example.com:443/").unwrap();
	assert_eq!(origin.origin().ascii_serialization(), "https://example.com");
}
