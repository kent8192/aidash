use super::*;

#[test]
fn challenge_is_the_rfc_7636_s256_transform() {
	// RFC 7636 Appendix B.
	let verifier = SecretString::from("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
	assert_eq!(
		challenge(&verifier),
		"E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
	);
}

#[test]
fn begin_binds_a_fresh_state_and_challenge_into_the_consent_url() {
	let first = begin(
		Provider::Openrouter,
		"https://aidash.example/auth/provider-authorizations/openrouter/callback",
		"acme",
	)
	.unwrap();
	let second = begin(
		Provider::Openrouter,
		"https://aidash.example/auth/provider-authorizations/openrouter/callback",
		"acme",
	)
	.unwrap();
	assert_ne!(first.state.expose_secret(), second.state.expose_secret());
	assert_ne!(
		first.verifier.expose_secret(),
		second.verifier.expose_secret()
	);
	let verifier = first.verifier.expose_secret();
	assert!((43..=128).contains(&verifier.len()));
	let url = url::Url::parse(&first.url).unwrap();
	assert_eq!(url.origin().ascii_serialization(), "https://openrouter.ai");
	assert_eq!(url.path(), "/auth");
	let query: std::collections::BTreeMap<_, _> = url.query_pairs().into_owned().collect();
	assert_eq!(
		query["callback_url"],
		"https://aidash.example/auth/provider-authorizations/openrouter/callback"
	);
	assert_eq!(query["code_challenge"], challenge(&first.verifier));
	assert_eq!(query["code_challenge_method"], "S256");
	assert_eq!(query["key_label"], "Aidash · acme");
	assert_eq!(query["state"], first.state.expose_secret());
	// The verifier never leaves the host.
	assert!(!first.url.contains(verifier));
}

#[test]
fn begin_rejects_callbacks_that_could_smuggle_parameters() {
	for callback in [
		"not a url",
		"ftp://aidash.example/callback",
		"https://aidash.example/callback?code=forged",
		"https://aidash.example/callback#fragment",
	] {
		assert!(begin(Provider::Openrouter, callback, "acme").is_err());
	}
}

#[test]
fn key_labels_are_bounded_and_digests_are_unkeyed_sha256() {
	assert_eq!(key_label(&"t".repeat(300)).chars().count(), 64);
	assert_eq!(
		key_digest(&SecretString::from("abc")),
		"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
	);
}
