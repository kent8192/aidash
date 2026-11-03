use aidash_capability::{
	InMemorySigner, PublicKeys, TokenSigner,
	federation::{rollover::*, *},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use secrecy::ExposeSecret;
use sha2::{Digest, Sha256};
use uuid::Uuid;

const PUBLISHED: u64 = 1_000;
const ACTIVATES: u64 = PUBLISHED + 86_400;
const RETIRES: u64 = ACTIVATES + 604_800;

fn signer(byte: u8) -> InMemorySigner {
	let preliminary = InMemorySigner::new(String::new(), [byte; 32]);
	InMemorySigner::new(
		URL_SAFE_NO_PAD.encode(Sha256::digest(preliminary.public_key().to_bytes())),
		[byte; 32],
	)
}
fn trusted(signer: &InMemorySigner) -> TrustedRolloverKey {
	TrustedRolloverKey {
		kid: signer.kid().into(),
		public_key_pem: signer.public_key_pem(),
	}
}
fn transition() -> RolloverClaims {
	RolloverClaims {
		deployment: "aidash://sender".into(),
		predecessor_kid: signer(1).kid().into(),
		successor_kid: signer(2).kid().into(),
		successor_public_key: URL_SAFE_NO_PAD.encode(signer(2).public_key().to_bytes()),
		published_at: PUBLISHED,
		activates_at: ACTIVATES,
		retires_at: RETIRES,
	}
}
fn request() -> RequestBinding<'static> {
	RequestBinding {
		protocol: "0.4",
		method: "POST",
		path: "/scoped/discover?limit=5",
		body: b"payload",
	}
}
async fn outer(proof: String) -> FederationClaims {
	FederationClaims {
		iss: "aidash://sender".into(),
		sub: "aidash://sender/brave-otter-a1b2c3d4".into(),
		aud: "aidash://receiver".into(),
		subject: "agent:abc".into(),
		iat: ACTIVATES,
		exp: ACTIVATES + 60,
		jti: Uuid::new_v4(),
		kid: signer(2).kid().into(),
		protocol: "0.4".into(),
		method: "POST".into(),
		path: request().path.into(),
		body: body_digest(request().body),
		transition: Some(proof),
	}
}
async fn token(claims: &FederationClaims, signer: &InMemorySigner) -> String {
	claims
		.mint(signer)
		.await
		.unwrap()
		.expose_secret()
		.to_owned()
}

#[tokio::test]
async fn successor_proof_is_usable_only_during_exact_overlap() {
	let claims = transition();
	let proof = mint_transition(&claims, &signer(1)).await.unwrap();
	for (now, accepted) in [
		(ACTIVATES - 1, false),
		(ACTIVATES, true),
		(RETIRES - 1, true),
		(RETIRES, false),
	] {
		assert_eq!(
			verify_transition(&proof, &trusted(&signer(1)), "aidash://sender", now).is_ok(),
			accepted
		);
	}
	assert_eq!(
		verify_transition(&proof, &trusted(&signer(1)), "aidash://sender", ACTIVATES).unwrap(),
		claims
	);
	assert!(verify_transition(&proof, &trusted(&signer(1)), "aidash://other", ACTIVATES).is_err());
	assert!(verify_transition(&proof, &trusted(&signer(2)), "aidash://sender", ACTIVATES).is_err());
}

#[tokio::test]
async fn predecessor_must_authenticate_the_exact_successor_and_schedule() {
	let valid = transition();
	for edit in [
		|c: &mut RolloverClaims| c.successor_kid = signer(3).kid().into(),
		|c: &mut RolloverClaims| c.successor_public_key = URL_SAFE_NO_PAD.encode([0; 31]),
		|c: &mut RolloverClaims| c.activates_at -= 1,
		|c: &mut RolloverClaims| c.retires_at += 1,
		|c: &mut RolloverClaims| {
			c.successor_kid = c.predecessor_kid.clone();
			c.successor_public_key = URL_SAFE_NO_PAD.encode(signer(1).public_key().to_bytes());
		},
	] {
		let mut changed = valid.clone();
		edit(&mut changed);
		let forged = raw_transition(&changed, &signer(1)).await;
		assert!(
			verify_transition(&forged, &trusted(&signer(1)), "aidash://sender", ACTIVATES).is_err()
		);
	}
	// A validly shaped proof signed by an attacker under the predecessor's ID.
	let forged = raw_transition(
		&valid,
		&InMemorySigner::new(signer(1).kid().into(), [3; 32]),
	)
	.await;
	assert_eq!(
		verify_transition(&forged, &trusted(&signer(1)), "aidash://sender", ACTIVATES).unwrap_err(),
		FederationFailure::InvalidSignature
	);
}
async fn raw_transition(claims: &RolloverClaims, signer: &InMemorySigner) -> String {
	let header =
		serde_json::json!({"alg":"EdDSA", "typ":ROLLOVER_TOKEN_TYPE, "kid":claims.predecessor_kid});
	let input = format!(
		"{}.{}",
		URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).unwrap()),
		URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).unwrap())
	);
	format!(
		"{input}.{}",
		URL_SAFE_NO_PAD.encode(signer.sign(input.as_bytes()).await.unwrap())
	)
}

#[tokio::test]
async fn proof_admits_only_a_successor_signed_request_with_all_original_checks() {
	let proof = mint_transition(&transition(), &signer(1)).await.unwrap();
	let claims = outer(proof.clone()).await;
	let valid = token(&claims, &signer(2)).await;
	assert_eq!(
		FederationClaims::verify_with_transition(
			&valid,
			&trusted(&signer(1)),
			"aidash://sender",
			ACTIVATES,
			&request()
		)
		.unwrap(),
		claims
	);
	assert!(
		FederationClaims::verify_with_transition(
			&proof,
			&trusted(&signer(1)),
			"aidash://sender",
			ACTIVATES,
			&request()
		)
		.is_err()
	);
	let attacker = InMemorySigner::new(signer(2).kid().into(), [3; 32]);
	let forged = token(&claims, &attacker).await;
	assert_eq!(
		FederationClaims::verify_with_transition(
			&forged,
			&trusted(&signer(1)),
			"aidash://sender",
			ACTIVATES,
			&request()
		)
		.unwrap_err(),
		FederationFailure::InvalidSignature
	);
	for binding in [
		RequestBinding {
			method: "GET",
			..request()
		},
		RequestBinding {
			path: "/scoped/other?limit=5",
			..request()
		},
		RequestBinding {
			path: "/scoped/discover?limit=6",
			..request()
		},
		RequestBinding {
			body: b"changed",
			..request()
		},
	] {
		assert_eq!(
			FederationClaims::verify_with_transition(
				&valid,
				&trusted(&signer(1)),
				"aidash://sender",
				ACTIVATES,
				&binding
			)
			.unwrap_err(),
			FederationFailure::Request
		);
	}
	let mut changed = claims.clone();
	changed.exp += 1;
	assert_eq!(
		FederationClaims::verify_with_transition(
			&token(&changed, &signer(2)).await,
			&trusted(&signer(1)),
			"aidash://sender",
			ACTIVATES,
			&request()
		)
		.unwrap_err(),
		FederationFailure::ClaimViolation
	);
	let mut changed = claims.clone();
	changed.iss = "aidash://other".into();
	assert!(
		FederationClaims::verify_with_transition(
			&token(&changed, &signer(2)).await,
			&trusted(&signer(1)),
			"aidash://sender",
			ACTIVATES,
			&request()
		)
		.is_err()
	);
	let mut wrong_deployment = transition();
	wrong_deployment.deployment = "aidash://other".into();
	let mut changed = claims.clone();
	changed.iss = "aidash://other".into();
	changed.transition = Some(
		mint_transition(&wrong_deployment, &signer(1))
			.await
			.unwrap(),
	);
	assert!(
		FederationClaims::verify_with_transition(
			&token(&changed, &signer(2)).await,
			&trusted(&signer(1)),
			"aidash://sender",
			ACTIVATES,
			&request(),
		)
		.is_err()
	);
	let mut changed = claims.clone();
	changed.iat = ACTIVATES - 1;
	changed.exp = ACTIVATES + 59;
	assert!(
		FederationClaims::verify_with_transition(
			&token(&changed, &signer(2)).await,
			&trusted(&signer(1)),
			"aidash://sender",
			ACTIVATES,
			&request(),
		)
		.is_err()
	);
	let mut changed = claims.clone();
	changed.transition = None;
	assert!(
		FederationClaims::verify_with_transition(
			&token(&changed, &signer(2)).await,
			&trusted(&signer(1)),
			"aidash://sender",
			ACTIVATES,
			&request(),
		)
		.is_err()
	);
	let mut changed = claims.clone();
	changed.kid = signer(3).kid().into();
	assert!(
		FederationClaims::verify_with_transition(
			&token(&changed, &signer(3)).await,
			&trusted(&signer(1)),
			"aidash://sender",
			ACTIVATES,
			&request()
		)
		.is_err()
	);
}

#[tokio::test]
async fn embedded_proof_shares_the_original_total_8192_byte_limit() {
	let proof = mint_transition(&transition(), &signer(1)).await.unwrap();
	let mut claims = outer(proof).await;
	let mut keys = PublicKeys::default();
	keys.insert(signer(2).kid().into(), signer(2).public_key());
	// Choose a payload accepted on its own but too large with the proof embedded.
	claims.subject = "s".repeat(5_000);
	let with_proof = token(&claims, &signer(2)).await;
	claims.transition = None;
	let without_proof = token(&claims, &signer(2)).await;
	assert!(without_proof.len() <= 8192 && with_proof.len() > 8192);
	FederationClaims::verify(&without_proof, &keys, ACTIVATES, &request()).unwrap();
	assert_eq!(
		FederationClaims::verify_with_transition(
			&with_proof,
			&trusted(&signer(1)),
			"aidash://sender",
			ACTIVATES,
			&request()
		)
		.unwrap_err(),
		FederationFailure::Malformed
	);
}

#[tokio::test]
async fn trusted_successor_retries_do_not_need_to_reapply_the_old_proof() {
	let proof = mint_transition(&transition(), &signer(1)).await.unwrap();
	let mut claims = outer(proof).await;
	let mut keys = PublicKeys::default();
	keys.insert(signer(2).kid().into(), signer(2).public_key());
	for now in [ACTIVATES, RETIRES, RETIRES + 60] {
		claims.iat = now;
		claims.exp = now + 60;
		let compact = token(&claims, &signer(2)).await;
		assert_eq!(
			FederationClaims::verify(&compact, &keys, now, &request()).unwrap(),
			claims
		);
	}
}

#[tokio::test]
async fn transition_minting_rejects_invalid_claims_and_a_different_signer() {
	assert!(mint_transition(&transition(), &signer(3)).await.is_err());
	let mut claims = transition();
	claims.retires_at += 1;
	assert!(mint_transition(&claims, &signer(1)).await.is_err());
}
