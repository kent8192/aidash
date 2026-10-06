use super::*;
use aidash_domain::{federation::Peer, semantic::Failure};
use async_trait::async_trait;
use rstest::rstest;
use serde_json::json;
use std::sync::{Arc, Mutex};
fn reply(status: u16) -> Reply {
	Reply {
		status,
		semantic_reason: None,
		transaction_pending: false,
		body: Ok(serde_json::to_vec(&json!({"ok":true})).unwrap()),
	}
}
#[rstest]
#[case(401)]
#[case(403)]
#[case(404)]
fn permanent_denials_never_reflect_peer_body(#[case] status: u16) {
	assert!(matches!(classify(reply(status)), Err(Error::Forbidden)));
}
#[rstest]
#[case(400)]
#[case(500)]
#[case(503)]
fn unavailability_has_stable_content_free_error(#[case] status: u16) {
	assert_eq!(
		classify(reply(status)).unwrap_err().to_string(),
		"remote execution authority unavailable"
	);
}
#[rstest]
fn conflict_and_pending_visibility_remain_distinct() {
	assert!(matches!(classify(reply(409)), Err(Error::Conflict(_))));
	let mut pending = reply(503);
	pending.transaction_pending = true;
	assert!(matches!(classify(pending), Err(Error::TransactionPending)));
}
#[rstest]
#[case(401)]
#[case(409)]
#[case(503)]
fn semantic_reason_precedes_generic_failure(#[case] status: u16) {
	let mut response = reply(status);
	response.semantic_reason = Some(Failure::Authority);
	assert!(matches!(
		classify(response),
		Err(Error::RemoteSemantic(Failure::Authority))
	));
}
#[rstest]
fn success_ignores_failure_header() {
	let mut response = reply(200);
	response.semantic_reason = Some(Failure::Authority);
	assert_eq!(
		classify(response).unwrap(),
		serde_json::to_vec(&json!({"ok":true})).unwrap()
	);
}
#[rstest]
fn malformed_success_payload_retains_invalid_response_error() {
	let mut response = reply(200);
	response.body = Err(Error::External("private parse diagnostics".into()));
	assert_eq!(
		classify(response).unwrap_err().to_string(),
		"invalid remote authority response"
	);
}
struct Sources {
	log: Arc<Mutex<Vec<String>>>,
	denied: bool,
}
#[async_trait]
impl Peers for Sources {
	async fn peer(&self, node: &str) -> Result<Peer> {
		self.log.lock().unwrap().push(format!("trust:{node}"));
		if self.denied {
			return Err(Error::Forbidden);
		}
		Ok(Peer {
			node_id: node.into(),
			endpoint: "http://peer".into(),
			credential_env: "peer-key".into(),
			protocol_version: "0.1".into(),
			enabled: true,
		})
	}
}
struct Wire {
	log: Arc<Mutex<Vec<String>>>,
	body: Vec<u8>,
}
#[async_trait]
impl Transport for Wire {
	async fn request(&self, peer: &Peer, path: &str, body: &Value) -> Result<Reply> {
		assert_eq!(body, &json!({"grant_id":"grant"}));
		self.log
			.lock()
			.unwrap()
			.push(format!("request:{}:{path}", peer.node_id));
		Ok(Reply {
			body: Ok(self.body.clone()),
			..reply(200)
		})
	}
}
#[rstest]
#[tokio::test]
async fn current_peer_trust_precedes_each_authority_request() {
	let log = Arc::new(Mutex::new(Vec::new()));
	let client = Client::new(
		Arc::new(Sources {
			log: log.clone(),
			denied: false,
		}),
		Arc::new(Wire {
			log: log.clone(),
			body: b"true".to_vec(),
		}),
	);
	for _ in 0..2 {
		assert!(
			client
				.request::<bool>(
					"aidash://peer",
					"/scoped/verify",
					&json!({"grant_id":"grant"})
				)
				.await
				.unwrap()
		);
	}
	assert_eq!(
		*log.lock().unwrap(),
		[
			"trust:aidash://peer",
			"request:aidash://peer:/scoped/verify",
			"trust:aidash://peer",
			"request:aidash://peer:/scoped/verify"
		]
	);
}
#[rstest]
#[tokio::test]
async fn denied_trust_does_not_contact_peer() {
	let log = Arc::new(Mutex::new(Vec::new()));
	let client = Client::new(
		Arc::new(Sources {
			log: log.clone(),
			denied: true,
		}),
		Arc::new(Wire {
			log: log.clone(),
			body: b"true".to_vec(),
		}),
	);
	assert_eq!(
		client
			.request::<bool>(
				"aidash://peer",
				"/scoped/verify",
				&json!({"grant_id":"grant"})
			)
			.await
			.unwrap_err()
			.to_string(),
		"remote execution authority unavailable"
	);
	assert_eq!(*log.lock().unwrap(), ["trust:aidash://peer"]);
}
#[rstest]
#[tokio::test]
async fn successful_json_still_requires_requested_contract() {
	let log = Arc::new(Mutex::new(Vec::new()));
	let client = Client::new(
		Arc::new(Sources {
			log: log.clone(),
			denied: false,
		}),
		Arc::new(Wire {
			log,
			body: br#"{"private":"details"}"#.to_vec(),
		}),
	);
	assert_eq!(
		client
			.request::<bool>(
				"aidash://peer",
				"/scoped/verify",
				&json!({"grant_id":"grant"})
			)
			.await
			.unwrap_err()
			.to_string(),
		"invalid remote authority response"
	);
}

#[rstest]
#[tokio::test]
async fn typed_authority_reply_rejects_duplicate_fields() {
	#[derive(Debug, serde::Deserialize)]
	struct Receipt {
		id: u64,
	}
	let log = Arc::new(Mutex::new(Vec::new()));
	let client = Client::new(
		Arc::new(Sources {
			log: log.clone(),
			denied: false,
		}),
		Arc::new(Wire {
			log,
			body: br#"{"id":1,"id":2}"#.to_vec(),
		}),
	);
	assert_eq!(
		client
			.request::<Receipt>(
				"aidash://peer",
				"/scoped/verify",
				&json!({"grant_id":"grant"})
			)
			.await
			.unwrap_err()
			.to_string(),
		"invalid remote authority response"
	);
	let clean: Receipt = serde_json::from_slice(br#"{"id":1}"#).unwrap();
	assert_eq!(clean.id, 1);
}
