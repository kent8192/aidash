use super::*;
use rstest::{fixture, rstest};
use serde_json::{Value, json};

const NOW: i64 = 1_800_000_000;
#[fixture]
fn claims() -> Value {
	json!({"sub":"subject","sid":"session","jti":"replay","iat":NOW-1,"exp":NOW+300,"events":{"http://schemas.openid.net/event/backchannel-logout":{}}})
}

#[rstest]
#[case("sub")]
#[case("sid")]
fn either_provider_subject_or_session_can_bind_revocation(
	mut claims: Value,
	#[case] missing: &str,
) {
	claims.as_object_mut().unwrap().remove(missing);
	let result = logout_claims(&claims, NOW).unwrap();
	assert_eq!(
		result.subject.as_deref(),
		(missing != "sub").then_some("subject")
	);
	assert_eq!(
		result.session.as_deref(),
		(missing != "sid").then_some("session")
	);
	assert_eq!(result.jti, "replay");
	assert_eq!(result.expires_at.timestamp(), NOW + 300);
}

#[rstest]
#[case("nonce", json!("nonce"), "invalid logout token claims")]
#[case("events", json!({}), "invalid logout token claims")]
#[case("jti", json!(""), "logout token needs a JTI")]
#[case("jti", json!("x".repeat(513)), "logout token needs a JTI")]
#[case("iat", json!(NOW+31), "invalid logout token lifetime")]
#[case("iat", json!(NOW-86_401), "invalid logout token lifetime")]
#[case("exp", json!(NOW-30), "invalid logout token lifetime")]
#[case("exp", json!(NOW+86_401), "invalid logout token lifetime")]
#[case("exp", json!(NOW-1), "invalid logout token lifetime")]
fn invalid_replay_identity_and_lifetime_fail_closed(
	mut claims: Value,
	#[case] key: &str,
	#[case] value: Value,
	#[case] reason: &str,
) {
	claims[key] = value;
	assert!(
		matches!(logout_claims(&claims, NOW), Err(crate::Error::Invalid(message)) if message == reason)
	);
}

#[rstest]
fn token_requires_a_subject_or_session(claims: Value) {
	let mut claims = claims;
	claims.as_object_mut().unwrap().remove("sub");
	claims.as_object_mut().unwrap().remove("sid");
	assert!(
		matches!(logout_claims(&claims, NOW), Err(crate::Error::Invalid(message)) if message == "logout token needs a subject or session ID")
	);
}

#[rstest]
#[case("acme.com", Some("acme.com"))]
#[case("ACME.Com", Some("acme.com"))]
#[case("bücher.example", Some("xn--bcher-kva.example"))]
#[case("xn--bcher-kva.example", Some("xn--bcher-kva.example"))]
#[case("localhost", None)]
#[case("acme.com.", None)]
#[case("-acme.com", None)]
#[case("ac_me.com", None)]
#[case("192.0.2.1", None)]
#[case("[::1]", None)]
#[case("", None)]
fn sign_in_domains_are_canonical_dns_names(#[case] raw: &str, #[case] expected: Option<&str>) {
	assert_eq!(sign_in_domain(raw).as_deref(), expected);
}

#[rstest]
#[case("alice@acme.com", Some("pool-a"))]
#[case("  Alice@ACME.com ", Some("pool-a"))]
#[case("bob@bücher.example", Some("pool-b"))]
#[case("alice@eng.acme.com", None)]
#[case("alice@other.com", None)]
#[case("acme.com", None)]
#[case("@acme.com", None)]
#[case("a@b@acme.com", None)]
#[case("alice@", None)]
fn email_routes_only_by_exact_sign_in_domain(#[case] email: &str, #[case] expected: Option<&str>) {
	let domains = std::collections::BTreeMap::from([
		("acme.com".to_owned(), "pool-a".to_owned()),
		("xn--bcher-kva.example".to_owned(), "pool-b".to_owned()),
	]);
	assert_eq!(routed_gcip_tenant(&domains, email), expected);
}

#[rstest]
fn overlong_addresses_never_route() {
	let domains = std::collections::BTreeMap::from([("acme.com".to_owned(), "pool-a".to_owned())]);
	assert_eq!(
		routed_gcip_tenant(&domains, &format!("{}@acme.com", "a".repeat(65))),
		None
	);
}
