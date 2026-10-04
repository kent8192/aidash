//! Peer configuration, uniqueness, and audit events through native HTTP and ORM.
use crate::endpoint::{EndpointFixture, assert_json, endpoint};
use crate::isolated;
use aidash_server::{
	Error,
	apps::{execution::models::Event, federation::peer::models::Peer},
	routes,
};
use reinhardt::db::orm::Model;
use reinhardt::test::fixtures::server::test_server_guard;
use rstest::{fixture, rstest};
use serde_json::{Value, json};
use std::future::Future;

const PEER_ENV: &str = "AIDASH_SECRET_PEER_REGISTRATION_FIXTURE";
const PEER_SECRET: &str = "peer-registration-fixture-0123456789-ABCDEFGHIJKLMNOPQRSTUVWXYZ";

fn input(remote: &EndpointFixture) -> Value {
	json!({"node_id":remote.runtime.config.node_id,"endpoint":remote.server.url,
        "credential_env":PEER_ENV,"protocol_version":"0.1","enabled":true})
}
async fn register(local: &EndpointFixture, input: &Value, status: u16) -> Value {
	assert_json(
		local
			.operator
			.post("/api/peers", input, "json")
			.await
			.unwrap(),
		status,
	)
}

struct RegistrationNodes {
	local: EndpointFixture,
	first: EndpointFixture,
	second: EndpointFixture,
}

#[fixture]
fn registration_nodes(
	#[future]
	#[from(endpoint)]
	#[with("aidash://local")]
	local: EndpointFixture,
	#[future]
	#[from(endpoint)]
	#[with("aidash://first")]
	first: EndpointFixture,
	#[future]
	#[from(endpoint)]
	#[with("aidash://second")]
	second: EndpointFixture,
) -> impl Future<Output = RegistrationNodes> {
	// Box before constructing the test future; three native bootstraps otherwise
	// exceed the default libtest thread stack when moved into an async test.
	let local = Box::pin(local);
	let first = Box::pin(first);
	let second = Box::pin(second);
	async move {
		RegistrationNodes {
			local: local.await,
			first: first.await,
			second: second.await,
		}
	}
}

#[rstest]
#[tokio::test]
async fn registration_updates_disable_and_reenable_one_peer_with_atomic_audit_events(
	#[future]
	#[from(endpoint)]
	#[with("aidash://local")]
	local: EndpointFixture,
	#[future]
	#[from(endpoint)]
	#[with("aidash://remote")]
	remote: EndpointFixture,
) {
	if !isolated::isolated_process(&[(PEER_ENV, PEER_SECRET)]).await {
		return;
	}
	// Arrange
	let local = Box::pin(local).await;
	let remote = Box::pin(remote).await;
	let relocated = test_server_guard(
		routes()
			.with_di_context(remote.context.clone())
			.into_server(),
	)
	.await;
	let original = input(&remote);
	// Act
	let created = register(&local, &original, 200).await;
	let mut updated = original.clone();
	updated["endpoint"] = json!(relocated.url);
	let replaced = register(&local, &updated, 200).await;
	let mut disabled = updated.clone();
	disabled["enabled"] = json!(false);
	// Disabling preserves the stored endpoint and does not require a live peer or secret.
	disabled["endpoint"] = json!("http://127.0.0.1:1");
	disabled["credential_env"] = json!("AIDASH_SECRET_MISSING_FIXTURE");
	let stopped = register(&local, &disabled, 200).await;
	// Assert
	assert_eq!(created, original);
	assert_eq!(replaced, updated);
	let mut expected = updated.clone();
	expected["enabled"] = json!(false);
	assert_eq!(stopped, expected);
	assert!(matches!(
		local.runtime.peer(&remote.runtime.config.node_id).await,
		Err(Error::Unauthorized)
	));
	assert_eq!(register(&local, &updated, 200).await, updated);
	for invalid in [
		{
			let mut value = updated.clone();
			value["node_id"] = json!("aidash://local");
			value
		},
		{
			let mut value = updated.clone();
			value["protocol_version"] = json!("99");
			value
		},
		{
			let mut value = updated.clone();
			value["node_id"] = json!("aidash://different-identity");
			value
		},
	] {
		register(&local, &invalid, 400).await;
	}
	let mut missing = updated.clone();
	missing["node_id"] = json!("aidash://missing");
	missing["enabled"] = json!(false);
	register(&local, &missing, 404).await;
	let mut db = local.database.lease.handle();
	let records = Peer::objects().all().all_with_db(&mut db).await.unwrap();
	assert_eq!(records.len(), 1);
	assert_eq!(records[0].node_id, remote.runtime.config.node_id);
	assert_eq!(records[0].endpoint, relocated.url);
	assert_eq!(records[0].credential_env, PEER_ENV);
	assert!(records[0].enabled);
	let events = Event::objects()
		.filter(Event::field_kind().eq("peer.registered".to_owned()))
		.order_by(&["sequence"])
		.all_with_db(&mut db)
		.await
		.unwrap();
	assert_eq!(
		events
			.iter()
			.map(|event| event.data.0["enabled"].clone())
			.collect::<Vec<_>>(),
		[json!(true), json!(true), json!(false), json!(true)]
	);
	assert!(
		events
			.iter()
			.all(|event| event.data.0["node_id"] == remote.runtime.config.node_id)
	);
}

#[rstest]
#[tokio::test]
async fn concurrent_registration_cannot_reuse_one_credential_for_two_node_identities(
	#[future] registration_nodes: RegistrationNodes,
) {
	if !isolated::isolated_process(&[(PEER_ENV, PEER_SECRET)]).await {
		return;
	}
	// Arrange
	let RegistrationNodes {
		local,
		first,
		second,
	} = registration_nodes.await;
	let first_input = input(&first);
	let second_input = input(&second);
	// Act
	let (first_response, second_response) = tokio::join!(
		local.operator.post("/api/peers", &first_input, "json"),
		local.operator.post("/api/peers", &second_input, "json"),
	);
	let first_response = first_response.unwrap();
	let second_response = second_response.unwrap();
	// Assert
	let mut statuses = [first_response.status_code(), second_response.status_code()];
	statuses.sort_unstable();
	assert_eq!(statuses, [200, 400]);
	let winner = if first_response.status_code() == 200 {
		&first_input
	} else {
		&second_input
	};
	let mut db = local.database.lease.handle();
	let records = Peer::objects().all().all_with_db(&mut db).await.unwrap();
	assert_eq!(records.len(), 1);
	assert_eq!(records[0].node_id, winner["node_id"]);
	let events = Event::objects()
		.filter(Event::field_kind().eq("peer.registered".to_owned()))
		.all_with_db(&mut db)
		.await
		.unwrap();
	assert_eq!(events.len(), 1);
	assert_eq!(events[0].data.0["node_id"], winner["node_id"]);
}
