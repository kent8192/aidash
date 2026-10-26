use super::*;
use crate::{
	Claims, InMemorySigner, Operation, TokenSigner, TokenSubject,
	jws::{Header, encode},
	mint,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use secrecy::ExposeSecret;
use serde_json::json;

const ISS: &str = "server";
const AUD: &str = "tenant-secret";
const KID: &str =
	"projects/app/locations/global/keyRings/server/cryptoKeys/secret/cryptoKeyVersions/1";

fn signer() -> InMemorySigner {
	InMemorySigner::new(KID.into(), [7; 32])
}

fn keys(signer: &InMemorySigner) -> PublicKeys {
	let mut keys = PublicKeys::default();
	keys.insert(signer.kid().into(), signer.public_key());
	keys
}

fn claims() -> SecretClaims {
	SecretClaims {
		iss: ISS.into(),
		aud: AUD.into(),
		iat: 100,
		exp: 160,
		jti: Uuid::from_u128(1),
		kid: KID.into(),
		tenant: "tenant-a".into(),
		secret: Uuid::from_u128(2),
		version: "3".into(),
		destination: Destination {
			origin: "https://api.example.com".into(),
			path_prefix: "/v1/".into(),
		},
		methods: vec!["GET".into(), "POST".into()],
		attach: Attach::Bearer,
		sub: SecretSubject::Run {
			run: "run-1".into(),
			call: Uuid::from_u128(3),
		},
		max_response_bytes: Some(262_144),
	}
}

fn provider_claims() -> Claims {
	Claims {
		iss: "worker".into(),
		aud: "environment".into(),
		iat: 100,
		exp: 160,
		jti: Uuid::from_u128(4),
		kid: KID.into(),
		tenant: "tenant-a".into(),
		provider: "openrouter".into(),
		credential: Uuid::from_u128(5),
		version: "1".into(),
		sub: TokenSubject::Run {
			run: "run-1".into(),
			call: Uuid::from_u128(3),
		},
		ops: vec![Operation::Chat],
		model: "author/model".into(),
		max_output_tokens: 100,
		cache_breakpoints: false,
		hosts: vec![],
	}
}

#[tokio::test]
async fn round_trips_every_attach_and_subject() {
	let signer = signer();
	let keys = keys(&signer);
	let mut value = claims();
	for attach in [
		Attach::Bearer,
		Attach::Header {
			name: "x-api-key".into(),
		},
	] {
		for sub in [
			SecretSubject::Run {
				run: "run-1".into(),
				call: Uuid::from_u128(3),
			},
			SecretSubject::Operation {
				operation: Uuid::from_u128(6),
				tenant: "tenant-a".into(),
			},
			SecretSubject::Workbench {
				tenant: "tenant-a".into(),
				call: Uuid::from_u128(7),
			},
		] {
			value.attach = attach.clone();
			value.sub = sub;
			let token = mint_secret(&value, &signer).await.unwrap();
			let verified = keys
				.verify_secret(token.expose_secret(), ISS, AUD, 100)
				.unwrap();
			assert_eq!(verified, value);
		}
	}
	assert_eq!(
		serde_json::to_value(Attach::Bearer).unwrap(),
		json!({"type": "bearer"})
	);
	assert_eq!(
		serde_json::to_value(Attach::Header {
			name: "x-api-key".into()
		})
		.unwrap(),
		json!({"type": "header", "name": "x-api-key"})
	);
	assert_eq!(
		serde_json::to_value(SecretSubject::Operation {
			operation: Uuid::from_u128(6),
			tenant: "tenant-a".into(),
		})
		.unwrap(),
		json!({"operation": Uuid::from_u128(6), "tenant": "tenant-a"})
	);
}

#[tokio::test]
async fn neither_token_kind_passes_as_the_other() {
	let signer = signer();
	let keys = keys(&signer);
	let secret = mint_secret(&claims(), &signer).await.unwrap();
	assert_eq!(
		keys.verify(secret.expose_secret(), ISS, AUD, 100)
			.unwrap_err(),
		Failure::InvalidSignature
	);
	let provider = mint(&provider_claims(), &signer).await.unwrap();
	assert_eq!(
		keys.verify_secret(provider.expose_secret(), ISS, AUD, 100)
			.unwrap_err(),
		Failure::InvalidSignature
	);
}

#[tokio::test]
async fn accepts_canonical_https_origins_and_root_prefixes() {
	let signer = signer();
	let keys = keys(&signer);
	for (origin, path_prefix) in [
		("https://api.example.com:8443", "/"),
		("https://127.0.0.1", "/v1/"),
		("https://api.example.com:443", "/a/b/"),
	] {
		let mut value = claims();
		value.destination = Destination {
			origin: origin.into(),
			path_prefix: path_prefix.into(),
		};
		let token = mint_secret(&value, &signer).await.unwrap();
		assert!(
			keys.verify_secret(token.expose_secret(), ISS, AUD, 100)
				.is_ok(),
			"{origin}"
		);
	}
}

fn apply(value: &mut SecretClaims, case: usize) {
	match case {
		0 => value.tenant.clear(),
		1 => value.tenant = "x".repeat(257),
		2 => value.secret = Uuid::nil(),
		3 => value.version = "latest".into(),
		4 => value.version = "01".into(),
		5 => value.version = "0".into(),
		6 => value.destination.path_prefix = "v1/".into(),
		7 => value.destination.path_prefix = "/v1".into(),
		8 => value.destination.path_prefix.clear(),
		9 => value.destination.path_prefix = "/a/../b/".into(),
		10 => value.destination.origin = "http://api.example.com".into(),
		11 => value.destination.origin = "https://API.example.com".into(),
		12 => value.destination.origin = "https://api.example.com/v1".into(),
		13 => value.destination.origin = "https://api.example.com/".into(),
		14 => value.destination.origin = "https://user@api.example.com".into(),
		15 => value.destination.origin = "https://api.example.com?x=1".into(),
		16 => value.destination.origin = "api.example.com".into(),
		17 => value.methods.clear(),
		18 => value.methods = vec!["get".into()],
		19 => value.methods = vec!["GET".into(), "GET".into()],
		20 => value.methods = vec!["CONNECT".into()],
		21 => value.methods = vec!["TRACE".into()],
		22 => value.methods = vec!["".into()],
		23 => value.attach = Attach::Header { name: "".into() },
		24 => {
			value.attach = Attach::Header {
				name: "host".into(),
			};
		}
		25 => {
			value.attach = Attach::Header {
				name: "sec-fetch-mode".into(),
			};
		}
		26 => {
			value.attach = Attach::Header {
				name: "x-forwarded-for".into(),
			};
		}
		27 => {
			value.attach = Attach::Header {
				name: "bad header".into(),
			};
		}
		28 => {
			value.sub = SecretSubject::Run {
				run: "".into(),
				call: Uuid::from_u128(3),
			};
		}
		29 => {
			value.sub = SecretSubject::Run {
				run: "run-1".into(),
				call: Uuid::nil(),
			};
		}
		30 => {
			value.sub = SecretSubject::Operation {
				operation: Uuid::nil(),
				tenant: "tenant-a".into(),
			};
		}
		31 => {
			value.sub = SecretSubject::Operation {
				operation: Uuid::from_u128(6),
				tenant: "tenant-b".into(),
			};
		}
		32 => {
			value.sub = SecretSubject::Workbench {
				tenant: "tenant-a".into(),
				call: Uuid::nil(),
			};
		}
		33 => {
			value.sub = SecretSubject::Workbench {
				tenant: "tenant-b".into(),
				call: Uuid::from_u128(7),
			};
		}
		34 => value.exp = 161,
		35 => value.iat = 101,
		36 => value.jti = Uuid::nil(),
		_ => unreachable!(),
	}
}

#[tokio::test]
async fn rejects_each_violation_with_its_own_code() {
	let signer = signer();
	let keys = keys(&signer);
	for (case, expected) in [
		(0, Failure::Tenant),
		(1, Failure::Tenant),
		(2, Failure::ClaimViolation),
		(3, Failure::ClaimViolation),
		(4, Failure::ClaimViolation),
		(5, Failure::ClaimViolation),
		(6, Failure::Destination),
		(7, Failure::Destination),
		(8, Failure::Destination),
		(9, Failure::Destination),
		(10, Failure::Destination),
		(11, Failure::Destination),
		(12, Failure::Destination),
		(13, Failure::Destination),
		(14, Failure::Destination),
		(15, Failure::Destination),
		(16, Failure::Destination),
		(17, Failure::Method),
		(18, Failure::Method),
		(19, Failure::Method),
		(20, Failure::Method),
		(21, Failure::Method),
		(22, Failure::Method),
		(23, Failure::ClaimViolation),
		(24, Failure::ClaimViolation),
		(25, Failure::ClaimViolation),
		(26, Failure::ClaimViolation),
		(27, Failure::ClaimViolation),
		(28, Failure::ClaimViolation),
		(29, Failure::ClaimViolation),
		(30, Failure::ClaimViolation),
		(31, Failure::Tenant),
		(32, Failure::ClaimViolation),
		(33, Failure::Tenant),
		(34, Failure::ClaimViolation),
		(35, Failure::ClaimViolation),
		(36, Failure::ClaimViolation),
	] {
		let mut value = claims();
		apply(&mut value, case);
		let token = mint_secret(&value, &signer).await.unwrap();
		assert_eq!(
			keys.verify_secret(token.expose_secret(), ISS, AUD, 100)
				.unwrap_err(),
			expected,
			"case {case}"
		);
	}
}

#[tokio::test]
async fn checks_signature_issuer_audience_and_time() {
	let signer = signer();
	let keys = keys(&signer);
	let token = mint_secret(&claims(), &signer).await.unwrap();
	let token = token.expose_secret();
	for (issuer, audience, now, expected) in [
		("other", AUD, 100, Failure::Audience),
		(ISS, "other", 100, Failure::Audience),
		(ISS, AUD, 160, Failure::Expired),
	] {
		assert_eq!(
			keys.verify_secret(token, issuer, audience, now)
				.unwrap_err(),
			expected
		);
	}
	assert_eq!(
		PublicKeys::default()
			.verify_secret(token, ISS, AUD, 100)
			.unwrap_err(),
		Failure::UnknownKid
	);
	let mut wrong = PublicKeys::default();
	wrong.insert(
		KID.into(),
		InMemorySigner::new(KID.into(), [8; 32]).public_key(),
	);
	assert_eq!(
		wrong.verify_secret(token, ISS, AUD, 100).unwrap_err(),
		Failure::InvalidSignature
	);
	for token in ["", "a.b.c.d", "eyJhbGciOiJub25lIn0.e30."] {
		assert_eq!(
			keys.verify_secret(token, ISS, AUD, 100).unwrap_err(),
			Failure::InvalidSignature
		);
	}
}

#[tokio::test]
async fn a_claim_never_names_a_key_other_than_its_header() {
	let signer = signer();
	let keys = keys(&signer);
	let mut value = serde_json::to_value(claims()).unwrap();
	value["kid"] = json!("kms/other");
	let header = Header {
		alg: "EdDSA".into(),
		typ: SECRET_TOKEN_TYPE.into(),
		kid: KID.into(),
	};
	let input = format!("{}.{}", encode(&header).unwrap(), encode(&value).unwrap());
	let signature = signer.sign(input.as_bytes()).await.unwrap();
	let token = format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature));
	assert_eq!(
		keys.verify_secret(&token, ISS, AUD, 100).unwrap_err(),
		Failure::UnknownKid
	);
}

#[tokio::test]
async fn a_token_with_an_unknown_claim_is_refused() {
	let signer = signer();
	let keys = keys(&signer);
	let mut value = serde_json::to_value(claims()).unwrap();
	value["total_ttl"] = json!(60);
	let header = Header {
		alg: "EdDSA".into(),
		typ: SECRET_TOKEN_TYPE.into(),
		kid: KID.into(),
	};
	let input = format!("{}.{}", encode(&header).unwrap(), encode(&value).unwrap());
	let signature = signer.sign(input.as_bytes()).await.unwrap();
	let token = format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature));
	assert_eq!(
		keys.verify_secret(&token, ISS, AUD, 100).unwrap_err(),
		Failure::InvalidSignature
	);
}

#[tokio::test]
async fn a_subject_with_an_unknown_member_is_refused() {
	let signer = signer();
	let keys = keys(&signer);
	let mut value = serde_json::to_value(claims()).unwrap();
	value["sub"]["extra"] = json!(true);
	let header = Header {
		alg: "EdDSA".into(),
		typ: SECRET_TOKEN_TYPE.into(),
		kid: KID.into(),
	};
	let input = format!("{}.{}", encode(&header).unwrap(), encode(&value).unwrap());
	let signature = signer.sign(input.as_bytes()).await.unwrap();
	let token = format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature));
	assert_eq!(
		keys.verify_secret(&token, ISS, AUD, 100).unwrap_err(),
		Failure::InvalidSignature
	);
}

#[tokio::test]
async fn a_token_without_the_optional_response_bound_still_verifies() {
	let signer = signer();
	let keys = keys(&signer);
	let mut value = claims();
	value.max_response_bytes = None;
	let token = mint_secret(&value, &signer).await.unwrap();
	let verified = keys
		.verify_secret(token.expose_secret(), ISS, AUD, 100)
		.unwrap();
	assert_eq!(verified.max_response_bytes, None);
	let payload = token.expose_secret().split('.').nth(1).unwrap();
	let payload: serde_json::Value =
		serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap();
	assert!(payload.get("max_response_bytes").is_none());
}
