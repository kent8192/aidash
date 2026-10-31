use super::*;
use crate::{
	InMemorySigner,
	tenant_secret::{Attach, mint_secret},
};
use secrecy::ExposeSecret;
fn subject() -> SecretSubject {
	SecretSubject::Operation {
		operation: Uuid::new_v4(),
		tenant: "tenant".into(),
	}
}
fn claims(operation: ConnectionOperation) -> ConnectionClaims {
	ConnectionClaims {
		iss: "server".into(),
		aud: "tsec".into(),
		iat: 100,
		exp: 160,
		jti: Uuid::new_v4(),
		kid: "key".into(),
		tenant: "tenant".into(),
		connection: Uuid::now_v7(),
		operation,
	}
}
#[tokio::test]
async fn connection_tokens_round_trip_all_operations_and_are_separate_from_other_types() {
	let signer = InMemorySigner::new("key".into(), [9; 32]);
	let mut keys = PublicKeys::default();
	keys.insert("key".into(), signer.public_key());
	let issuers = BTreeMap::from([("key".into(), "server".into())]);
	let destination = Destination {
		origin: "https://mcp.example".into(),
		path_prefix: "/mcp/".into(),
	};
	let operations = vec![
		ConnectionOperation::Exchange {
			issuer: "https://auth.example".into(),
			resource: "https://mcp.example/mcp".into(),
			client_id: "client".into(),
			client: Some(ClientSecretRef {
				kind: ClientKind::Dynamic,
				id: Uuid::now_v7(),
			}),
			redirect_uri: "https://aidash.example/auth/connections/callback".into(),
			sub: subject(),
		},
		ConnectionOperation::Refresh {
			version: "3".into(),
			sub: subject(),
		},
		ConnectionOperation::Revoke {
			version: "3".into(),
		},
		ConnectionOperation::Forward {
			version: "3".into(),
			destination: destination.clone(),
			methods: vec!["POST".into()],
			sub: SecretSubject::Run {
				run: "run".into(),
				call: Uuid::new_v4(),
			},
			max_response_bytes: Some(1024),
		},
	];
	for op in operations {
		let c = claims(op);
		let token = mint_connection(&c, &signer).await.unwrap();
		assert_eq!(
			keys.verify_connection(token.expose_secret(), &issuers, "tsec", 100)
				.unwrap(),
			c
		);
		assert!(
			keys.verify_secret(token.expose_secret(), &issuers, "tsec", 100)
				.is_err()
		);
		assert!(
			keys.verify(token.expose_secret(), "server", "tsec", 100)
				.is_err()
		);
		assert!(
			keys.verify_connection(token.expose_secret(), &issuers, "tsec", 160)
				.is_err()
		);
		assert!(
			keys.verify_connection(token.expose_secret(), &issuers, "other", 100)
				.is_err()
		);
	}
	let secret = SecretClaims {
		iss: "server".into(),
		aud: "tsec".into(),
		iat: 100,
		exp: 160,
		jti: Uuid::new_v4(),
		kid: "key".into(),
		tenant: "tenant".into(),
		secret: Uuid::now_v7(),
		version: "1".into(),
		destination,
		methods: vec!["POST".into()],
		attach: Attach::Bearer,
		sub: subject(),
		max_response_bytes: None,
	};
	let token = mint_secret(&secret, &signer).await.unwrap();
	assert!(
		keys.verify_connection(token.expose_secret(), &issuers, "tsec", 100)
			.is_err()
	);
}
#[test]
fn exchange_cannot_hide_endpoints_or_use_a_run_subject() {
	let mut c = claims(ConnectionOperation::Exchange {
		issuer: "https://auth.example".into(),
		resource: "https://mcp.example/mcp".into(),
		client_id: "c".into(),
		client: None,
		redirect_uri: "https://aidash.example/callback".into(),
		sub: subject(),
	});
	let mut json = serde_json::to_value(&c).unwrap();
	json["operation"]["token_endpoint"] = "https://evil.example/token".into();
	assert!(serde_json::from_value::<ConnectionClaims>(json).is_err());
	if let ConnectionOperation::Exchange { sub, .. } = &mut c.operation {
		*sub = SecretSubject::Run {
			run: "run".into(),
			call: Uuid::new_v4(),
		};
	}
	assert!(c.validate().is_err());
	c.operation = ConnectionOperation::Refresh {
		version: "latest".into(),
		sub: subject(),
	};
	assert!(c.validate().is_err());
	c.operation = ConnectionOperation::Refresh {
		version: "1".into(),
		sub: SecretSubject::Operation {
			operation: Uuid::new_v4(),
			tenant: "other".into(),
		},
	};
	assert!(c.validate().is_err());
}
#[test]
fn sealed_clients_bind_issuer_client_id_redirect_and_refuse_unknown_kinds_versions() {
	let payload = sealed::Payload::DynamicClient {
		issuer: "https://auth.example".into(),
		client_id: "c".into(),
		redirect_uri: "https://aidash.example/callback".into(),
		client_secret: "canary".into(),
	};
	let bytes = sealed::seal(payload).unwrap();
	let sealed::Payload::DynamicClient {
		issuer,
		client_id,
		redirect_uri,
		client_secret,
	} = sealed::open(&bytes).unwrap()
	else {
		panic!("wrong kind")
	};
	assert_eq!(issuer, "https://auth.example");
	assert_eq!(client_id, "c");
	assert_eq!(redirect_uri, "https://aidash.example/callback");
	assert_eq!(client_secret.expose_secret(), "canary");
	let unsupported_version = br#"{"v":2,"kind":"registered_client","issuer":"https://a.example","client_id":"c","client_secret":"canary"}"#;
	let hidden_endpoint = br#"{"v":1,"kind":"registered_client","issuer":"https://a.example","client_id":"c","client_secret":"canary","token_endpoint":"https://evil.example"}"#;
	for bad in [unsupported_version.as_slice(), hidden_endpoint.as_slice()] {
		assert!(sealed::open(bad).is_err());
	}
}
