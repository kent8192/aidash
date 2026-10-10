use super::*;
use secrecy::ExposeSecret;
fn claims() -> Claims {
	Claims {
		iss: "worker".into(),
		aud: "environment".into(),
		iat: 100,
		exp: 160,
		jti: Uuid::new_v4(),
		kid: "kms/versions/1".into(),
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
	}
}
#[tokio::test]
async fn verifies_and_rejects_each_claim_failure() {
	let signer = InMemorySigner::new("kms/versions/1".into(), [7; 32]);
	let mut keys = PublicKeys::default();
	keys.insert(signer.kid().into(), signer.public_key());
	let token = mint(&claims(), &signer).await.unwrap();
	assert_eq!(
		keys.verify(token.expose_secret(), "worker", "environment", 100)
			.unwrap()
			.model,
		"author/model"
	);
	assert_eq!(
		keys.verify(token.expose_secret(), "worker", "other", 100)
			.unwrap_err(),
		Failure::Audience
	);
	assert_eq!(
		keys.verify(token.expose_secret(), "worker", "environment", 160)
			.unwrap_err(),
		Failure::Expired
	);
	assert_eq!(
		PublicKeys::default()
			.verify(token.expose_secret(), "worker", "environment", 100)
			.unwrap_err(),
		Failure::UnknownKid
	);
	let mut wrong_keys = PublicKeys::default();
	wrong_keys.insert(
		signer.kid().into(),
		InMemorySigner::new(signer.kid().into(), [8; 32]).public_key(),
	);
	assert_eq!(
		wrong_keys
			.verify(token.expose_secret(), "worker", "environment", 100)
			.unwrap_err(),
		Failure::InvalidSignature
	);
	for (change, expected) in [
		(0, Failure::Tenant),
		(1, Failure::Credential),
		(2, Failure::Operation),
		(3, Failure::Model),
		(4, Failure::ClaimViolation),
		(5, Failure::Tenant),
		(6, Failure::Credential),
		(7, Failure::ClaimViolation),
	] {
		let mut value = claims();
		match change {
			0 => value.tenant.clear(),
			1 => value.credential = Uuid::nil(),
			2 => value.ops.clear(),
			3 => value.model = "author/../model".into(),
			4 => value.exp = 161,
			5 => {
				value.sub = TokenSubject::Maintenance {
					maintenance: Maintenance::MemoryIndexing,
					tenant: "tenant-b".into(),
				}
			}
			6 => value.version = "latest".into(),
			7 => value.iat = 101,
			_ => unreachable!(),
		}
		let token = mint(&value, &signer).await.unwrap();
		assert_eq!(
			keys.verify(token.expose_secret(), "worker", "environment", 100)
				.unwrap_err(),
			expected
		);
	}
}

#[tokio::test]
async fn maintenance_subject_records_purpose_separately_from_operation() {
	let signer = InMemorySigner::new("kms/versions/1".into(), [7; 32]);
	let mut keys = PublicKeys::default();
	keys.insert(signer.kid().into(), signer.public_key());
	for (purpose, wire) in [
		(Maintenance::MemoryIndexing, "memory_indexing"),
		(Maintenance::MemoryRetention, "memory_retention"),
		(Maintenance::MemoryReflection, "memory_reflection"),
		(Maintenance::MemoryRetrieval, "memory_retrieval"),
	] {
		for operation in [Operation::Chat, Operation::Embeddings] {
			let mut value = claims();
			value.sub = TokenSubject::Maintenance {
				maintenance: purpose,
				tenant: value.tenant.clone(),
			};
			value.ops = vec![operation];
			let token = mint(&value, &signer).await.unwrap();
			let verified = keys
				.verify(token.expose_secret(), "worker", "environment", 100)
				.unwrap();
			assert_eq!(verified.sub, value.sub);
			assert_eq!(verified.ops, vec![operation]);
			assert_eq!(
				serde_json::to_value(&verified.sub).unwrap(),
				serde_json::json!({"maintenance": wire, "tenant": "tenant-a"})
			);
		}
	}
	// Even a correctly signed token cannot use the obsolete call-kind subjects.
	for obsolete in ["memory_embedding", "memory_model"] {
		let mut value = serde_json::to_value(claims()).unwrap();
		value["sub"] = serde_json::json!({"maintenance": obsolete, "tenant": "tenant-a"});
		let header = Header {
			alg: "EdDSA".into(),
			typ: TOKEN_TYPE.into(),
			kid: signer.kid().into(),
		};
		let input = format!("{}.{}", encode(&header).unwrap(), encode(&value).unwrap());
		let signature = signer.sign(input.as_bytes()).await.unwrap();
		let token = format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature));
		assert_eq!(
			keys.verify(&token, "worker", "environment", 100)
				.unwrap_err(),
			Failure::InvalidSignature
		);
	}
}
#[tokio::test]
async fn enforces_jws_format_and_key_binding() {
	let signer = InMemorySigner::new("kms/versions/1".into(), [7; 32]);
	let mut value = claims();
	value.kid = "other".into();
	assert!(mint(&value, &signer).await.is_err());
	let keys = PublicKeys::default();
	for token in ["", "a.b.c.d", "eyJhbGciOiJub25lIn0.e30."] {
		assert_eq!(
			keys.verify(token, "worker", "environment", 100)
				.unwrap_err(),
			Failure::InvalidSignature
		);
	}
}
