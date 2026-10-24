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
