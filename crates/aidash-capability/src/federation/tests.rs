use super::*;
use crate::{Claims, InMemorySigner, Operation, TokenSubject, jws::Header, jws::encode};
use rstest::rstest;
use secrecy::ExposeSecret;

const KID: &str = "kJ2hQx0aEoV4hB1CqVwMr4mP3oY8pQZLQ0wU6o3m1sA";
const NOW: u64 = 1_000;

fn signer() -> InMemorySigner {
	InMemorySigner::new(KID.into(), [7; 32])
}

fn keys() -> PublicKeys {
	let signer = signer();
	let mut keys = PublicKeys::default();
	keys.insert(signer.kid().into(), signer.public_key());
	keys
}

fn claims() -> FederationClaims {
	FederationClaims {
		iss: "aidash://sender".into(),
		sub: "aidash://sender/brave-otter-a1b2c3d4".into(),
		aud: "aidash://receiver/calm-heron-e5f6g7h8".into(),
		subject: "oidc:3b1c6c0e-0d3d-4f0a-9d65-0f1f6d9f7c11".into(),
		iat: NOW,
		exp: NOW + 60,
		jti: Uuid::new_v4(),
		kid: KID.into(),
		protocol: "0.4".into(),
		method: "POST".into(),
		path: "/scoped/discover?limit=5".into(),
		body: body_digest(b"{\"query\":\"x\"}"),
	}
}

fn request() -> RequestBinding<'static> {
	RequestBinding {
		protocol: "0.4",
		method: "POST",
		path: "/scoped/discover?limit=5",
		body: b"{\"query\":\"x\"}",
	}
}

async fn signed(claims: &FederationClaims) -> String {
	claims
		.mint(&signer())
		.await
		.unwrap()
		.expose_secret()
		.to_owned()
}

/// A token signed with `signer()` whose header and payload are chosen freely.
async fn forged(header_kid: &str, payload: &impl Serialize) -> String {
	let header = Header {
		alg: "EdDSA".into(),
		typ: FEDERATION_TOKEN_TYPE.into(),
		kid: header_kid.into(),
	};
	let input = format!("{}.{}", encode(&header).unwrap(), encode(payload).unwrap());
	let signature = signer().sign(input.as_bytes()).await.unwrap();
	format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature))
}

#[tokio::test]
async fn a_minted_token_verifies_against_its_own_request() {
	let minted = claims();
	let token = signed(&minted).await;
	assert_eq!(
		FederationClaims::verify(&token, &keys(), NOW, &request()).unwrap(),
		minted
	);
	assert_eq!(FederationClaims::unverified(&token).unwrap(), minted);
}

#[tokio::test]
async fn the_header_names_the_type_and_the_key_id() {
	let token = signed(&claims()).await;
	let header: Header = jws::decode(token.split('.').next().unwrap()).unwrap();
	assert_eq!(
		(
			header.alg.as_str(),
			header.typ.as_str(),
			header.kid.as_str()
		),
		("EdDSA", "aidash-federation+jwt", KID)
	);
}

#[tokio::test]
async fn a_token_is_minted_only_for_the_signing_key() {
	let mut other = claims();
	other.kid = "another-key-id".into();
	assert!(other.mint(&signer()).await.is_err());
}

#[rstest]
#[case::body_changed(
	b"{\"query\":\"y\"}",
	"POST",
	"/scoped/discover?limit=5",
	FederationFailure::Request
)]
#[case::body_empty(b"", "POST", "/scoped/discover?limit=5", FederationFailure::Request)]
#[case::path_changed(
	b"{\"query\":\"x\"}",
	"POST",
	"/scoped/execution/inspect",
	FederationFailure::Request
)]
#[case::query_changed(
	b"{\"query\":\"x\"}",
	"POST",
	"/scoped/discover?limit=6",
	FederationFailure::Request
)]
#[case::query_dropped(
	b"{\"query\":\"x\"}",
	"POST",
	"/scoped/discover",
	FederationFailure::Request
)]
#[case::method_changed(
	b"{\"query\":\"x\"}",
	"GET",
	"/scoped/discover?limit=5",
	FederationFailure::Request
)]
#[tokio::test]
async fn a_token_serves_only_the_request_it_was_minted_for(
	#[case] body: &'static [u8],
	#[case] method: &'static str,
	#[case] path: &'static str,
	#[case] expected: FederationFailure,
) {
	let token = signed(&claims()).await;
	let other = RequestBinding {
		body,
		method,
		path,
		..request()
	};
	assert_eq!(
		FederationClaims::verify(&token, &keys(), NOW, &other).unwrap_err(),
		expected
	);
}

#[tokio::test]
async fn another_protocol_version_is_refused() {
	let token = signed(&claims()).await;
	let other = RequestBinding {
		protocol: "0.3",
		..request()
	};
	assert_eq!(
		FederationClaims::verify(&token, &keys(), NOW, &other).unwrap_err(),
		FederationFailure::Protocol
	);
}

#[rstest]
#[case::expires_now(|c: &mut FederationClaims| c.exp = NOW, FederationFailure::Expired)]
#[case::expired(|c: &mut FederationClaims| { c.iat = NOW - 120; c.exp = NOW - 60 }, FederationFailure::Expired)]
#[case::issued_31_seconds_ahead(|c: &mut FederationClaims| { c.iat = NOW + IAT_LEEWAY_SECS + 1; c.exp = NOW + IAT_LEEWAY_SECS + 61 }, FederationFailure::ClaimViolation)]
#[case::issued_40_seconds_ahead(|c: &mut FederationClaims| { c.iat = NOW + 40; c.exp = NOW + 100 }, FederationFailure::ClaimViolation)]
#[case::lives_61_seconds(|c: &mut FederationClaims| { c.iat = NOW - 1; c.exp = NOW + 60 }, FederationFailure::ClaimViolation)]
#[case::expires_before_it_is_issued(|c: &mut FederationClaims| { c.iat = NOW; c.exp = NOW - 1 }, FederationFailure::Expired)]
#[case::nil_jti(|c: &mut FederationClaims| c.jti = Uuid::nil(), FederationFailure::ClaimViolation)]
#[tokio::test]
async fn the_time_window_and_the_jti_are_checked(
	#[case] change: fn(&mut FederationClaims),
	#[case] expected: FederationFailure,
) {
	let mut changed = claims();
	change(&mut changed);
	let token = signed(&changed).await;
	assert_eq!(
		FederationClaims::verify(&token, &keys(), NOW, &request()).unwrap_err(),
		expected
	);
}

#[rstest]
#[case::now(0)]
#[case::twenty_seconds_ahead(20)]
#[case::the_whole_leeway(IAT_LEEWAY_SECS)]
#[tokio::test]
async fn a_sender_whose_clock_is_ahead_by_the_leeway_or_less_is_accepted(#[case] ahead: u64) {
	let mut early = claims();
	early.iat = NOW + ahead;
	early.exp = NOW + ahead + 60;
	let token = signed(&early).await;
	FederationClaims::verify(&token, &keys(), NOW, &request()).unwrap();
}

#[tokio::test]
async fn a_token_lives_for_exactly_the_ttl_and_not_a_second_longer() {
	let mut limit = claims();
	limit.iat = NOW - 60 + 1;
	limit.exp = NOW + 1;
	let token = signed(&limit).await;
	FederationClaims::verify(&token, &keys(), NOW, &request()).unwrap();
	let mut longer = claims();
	longer.iat = NOW - 60;
	longer.exp = NOW + 1;
	let token = signed(&longer).await;
	assert_eq!(
		FederationClaims::verify(&token, &keys(), NOW, &request()).unwrap_err(),
		FederationFailure::ClaimViolation
	);
}

#[tokio::test]
async fn an_unknown_key_and_a_wrong_key_are_different_failures() {
	let token = signed(&claims()).await;
	assert_eq!(
		FederationClaims::verify(&token, &PublicKeys::default(), NOW, &request()).unwrap_err(),
		FederationFailure::UnknownKid
	);
	let mut wrong = PublicKeys::default();
	wrong.insert(
		KID.into(),
		InMemorySigner::new(KID.into(), [8; 32]).public_key(),
	);
	assert_eq!(
		FederationClaims::verify(&token, &wrong, NOW, &request()).unwrap_err(),
		FederationFailure::InvalidSignature
	);
}

#[tokio::test]
async fn a_tampered_payload_fails_its_signature() {
	let token = signed(&claims()).await;
	let mut parts: Vec<_> = token.split('.').map(str::to_owned).collect();
	let mut payload: serde_json::Value = jws::decode(&parts[1]).unwrap();
	payload["subject"] = "oidc:someone-else".into();
	parts[1] = encode(&payload).unwrap();
	assert_eq!(
		FederationClaims::verify(&parts.join("."), &keys(), NOW, &request()).unwrap_err(),
		FederationFailure::InvalidSignature
	);
}

#[tokio::test]
async fn the_header_and_claim_key_ids_must_agree() {
	let mut payload = claims();
	payload.kid = "another-key-id".into();
	let token = forged(KID, &payload).await;
	assert_eq!(
		FederationClaims::verify(&token, &keys(), NOW, &request()).unwrap_err(),
		FederationFailure::ClaimViolation
	);
}

#[tokio::test]
async fn a_payload_with_an_unknown_claim_is_refused_even_when_signed() {
	let mut payload = serde_json::to_value(claims()).unwrap();
	payload["admin"] = true.into();
	let token = forged(KID, &payload).await;
	assert_eq!(
		FederationClaims::verify(&token, &keys(), NOW, &request()).unwrap_err(),
		FederationFailure::Malformed
	);
}

#[tokio::test]
async fn the_two_token_types_never_stand_in_for_each_other() {
	let federation = signed(&claims()).await;
	assert_eq!(
		keys()
			.verify(&federation, "aidash://sender", "environment", NOW)
			.unwrap_err(),
		crate::Failure::InvalidSignature
	);
	let capability = crate::mint(
		&Claims {
			iss: "worker".into(),
			aud: "environment".into(),
			iat: NOW,
			exp: NOW + 60,
			jti: Uuid::new_v4(),
			kid: KID.into(),
			tenant: "tenant-a".into(),
			provider: "openrouter".into(),
			credential: Uuid::new_v4(),
			version: "1".into(),
			sub: TokenSubject::Run {
				run: "run-1".into(),
				call: Uuid::new_v4(),
			},
			ops: vec![Operation::Chat],
			model: "author/model".into(),
			max_output_tokens: 100,
			cache_breakpoints: false,
			hosts: vec![],
		},
		&signer(),
	)
	.await
	.unwrap();
	assert_eq!(
		FederationClaims::verify(capability.expose_secret(), &keys(), NOW, &request()).unwrap_err(),
		FederationFailure::Malformed
	);
	assert_eq!(
		FederationClaims::unverified(capability.expose_secret()).unwrap_err(),
		FederationFailure::Malformed
	);
}

#[rstest]
#[case::empty("")]
#[case::two_parts("a.b")]
#[case::four_parts("a.b.c.d")]
#[case::alg_none("eyJhbGciOiJub25lIn0.e30.")]
#[case::not_base64("!.!.!")]
#[tokio::test]
async fn a_token_that_is_not_a_compact_jws_is_malformed(#[case] token: &str) {
	assert_eq!(
		FederationClaims::verify(token, &keys(), NOW, &request()).unwrap_err(),
		FederationFailure::Malformed
	);
	assert_eq!(
		FederationClaims::unverified(token).unwrap_err(),
		FederationFailure::Malformed
	);
}

#[tokio::test]
async fn a_token_over_8192_bytes_is_refused_before_it_is_read() {
	let mut long = claims();
	long.subject = "s".repeat(8192);
	let token = signed(&long).await;
	assert!(token.len() > 8192);
	assert_eq!(
		FederationClaims::verify(&token, &keys(), NOW, &request()).unwrap_err(),
		FederationFailure::Malformed
	);
}

#[test]
fn the_body_digest_is_43_url_safe_characters() {
	let digest = body_digest(b"");
	assert_eq!(digest, "47DEQpj8HBSa-_TImW-5JCeuQeRkm5NMpJWZG3hSuFU");
	assert_eq!(digest.len(), 43);
}
