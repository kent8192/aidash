use super::*;
use crate::ports::federation::authority::Unreachable;
use aidash_domain::{federation::route::Route, semantic::Failure};
use async_trait::async_trait;
use rstest::rstest;
use serde_json::json;
use std::sync::{Arc, Mutex};
fn reply(status: u16) -> Reply {
	Reply {
		status,
		semantic_reason: None,
		execution_refusal: None,
		transaction_pending: false,
		unreachable: None,
		policy_busy: None,
		body: Ok(serde_json::to_vec(&json!({"ok":true})).unwrap()),
	}
}
#[rstest]
#[case(401)]
#[case(403)]
fn permanent_denials_never_reflect_peer_body(#[case] status: u16) {
	assert!(matches!(classify(reply(status)), Err(Error::Forbidden)));
}
#[rstest]
fn a_receiver_that_did_not_find_it_is_no_denial() {
	let mut response = reply(404);
	response.body = Err(Error::External("private receiver text".into()));
	assert!(matches!(
		classify(response),
		Err(Error::NotFound(message)) if message == "the target Node did not find it"
	));
}
#[rstest]
fn receiver_can_share_a_closed_execution_refusal_without_reflecting_its_body() {
	let mut response = reply(403);
	response.execution_refusal =
		Some(aidash_domain::federation::execution::Refusal::CatalogApproval);
	assert!(matches!(
		classify(response),
		Err(Error::RemoteExecutionRefused(
			aidash_domain::federation::execution::Refusal::CatalogApproval
		))
	));
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
fn a_peer_refusing_an_authority_change_keeps_its_wait() {
	let mut busy = reply(503);
	busy.policy_busy = Some(30);
	assert!(matches!(
		classify(busy),
		Err(Error::PolicyBusy { retry_after: 30 })
	));
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
	async fn route(
		&self,
		node: &str,
		request: &crate::federation::inbound::RequestDigest<'_>,
	) -> Result<Route> {
		assert_eq!(request.method, "POST");
		assert_eq!(request.path, "/scoped/verify");
		assert_eq!(
			serde_json::from_slice::<Value>(request.body).unwrap(),
			json!({"grant_id":"grant"})
		);
		self.log.lock().unwrap().push(format!("trust:{node}"));
		if self.denied {
			return Err(Error::Forbidden);
		}
		Ok(crate::test_support::foreign_route(node))
	}
}
struct Wire {
	log: Arc<Mutex<Vec<String>>>,
	body: Vec<u8>,
}
#[async_trait]
impl Transport for Wire {
	async fn request(&self, route: &Route, path: &str, body: &Value) -> Result<Reply> {
		assert_eq!(body, &json!({"grant_id":"grant"}));
		self.log
			.lock()
			.unwrap()
			.push(format!("request:{}:{path}", route.target.deployment));
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
		"forbidden"
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

#[rstest]
#[tokio::test]
async fn personal_call_preserves_peer_result_and_delivery_failure() {
	struct CallWire {
		delivered: bool,
	}
	#[async_trait]
	impl Transport for CallWire {
		async fn request(&self, _: &Route, _: &str, _: &Value) -> Result<Reply> {
			if !self.delivered {
				return Err(Error::ToolNotDelivered {
					message: "peer unavailable".into(),
					retry_after: None,
				});
			}
			Ok(Reply {
				body: Ok(serde_json::to_vec(
					&json!({"outcome":"result","result":{"error":"tool failed"}}),
				)
				.unwrap()),
				..reply(200)
			})
		}
	}
	struct CallPeers;
	#[async_trait]
	impl Peers for CallPeers {
		async fn route(
			&self,
			node: &str,
			request: &crate::federation::inbound::RequestDigest<'_>,
		) -> Result<Route> {
			assert_eq!(request.method, "POST");
			assert_eq!(request.path, "/scoped/execution/commands");
			assert_eq!(
				serde_json::from_slice::<Value>(request.body).unwrap(),
				json!({"operation": aidash_domain::identity::commands::connections::CALL})
			);
			Ok(crate::test_support::foreign_route(node))
		}
	}
	let peers = Arc::new(CallPeers);
	let path = "/scoped/execution/commands";
	let body = json!({"operation": aidash_domain::identity::commands::connections::CALL});
	let delivered = Client::new(peers.clone(), Arc::new(CallWire { delivered: true }));
	let value: Value = delivered
		.request("aidash://peer", path, &body)
		.await
		.unwrap();
	assert_eq!(
		crate::tools::connection_outcome(value, false).unwrap(),
		Some(json!({"error":"tool failed"}))
	);
	let unavailable = Client::new(peers, Arc::new(CallWire { delivered: false }));
	assert!(matches!(
		unavailable
			.request::<Value>("aidash://peer", path, &body)
			.await,
		Err(Error::ToolNotDelivered { .. })
	));
}

#[rstest]
#[case::before_effect(false)]
#[case::after_effect(true)]
fn a_peer_that_could_not_reach_another_node_is_a_typed_unreachable_error(
	#[case] after_effect: bool,
) {
	let found = Unreachable::of(&json!({
		"error": "Node could not be reached", "code": "node_unreachable",
		"node": "aidash://other/sibling", "after_effect": after_effect
	}))
	.unwrap();
	let result = classify(Reply {
		unreachable: Some(found),
		..reply(503)
	});
	assert!(matches!(
		result,
		Err(Error::NodeUnreachable { ref node, after_effect: seen })
			if node == "aidash://other/sibling" && seen == after_effect
	));
}

#[rstest]
fn an_envelope_without_a_node_or_another_code_is_not_unreachable() {
	assert!(Unreachable::of(&json!({"code": "node_unreachable"})).is_none());
	assert!(Unreachable::of(&json!({"code": "other", "node": "aidash://x/y"})).is_none());
	// Without the flag the call is assumed to have taken effect.
	assert!(
		Unreachable::of(&json!({"code": "node_unreachable", "node": "aidash://x/y"}))
			.unwrap()
			.after_effect
	);
}
