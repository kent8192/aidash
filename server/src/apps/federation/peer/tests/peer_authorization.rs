//! Real HTTP coverage for peer mappings and discovery under live authority changes.
#![allow(clippy::disallowed_methods)] // Fixtures reach the database outside a tenancy scope.
#[path = "../../../execution/tests/support/endpoint.rs"]
mod endpoint;
#[path = "../../../execution/tests/support/execution.rs"]
mod execution_fixtures;
#[path = "../../../execution/tests/support/native_database.rs"]
mod native_database;
#[path = "../../../execution/tests/support/federation.rs"]
mod peering;
// Discovery uses the successful provider fixture; adapter suites use the other modes.
#[allow(dead_code)]
#[path = "../../../execution/tests/provider_fixtures.rs"]
mod provider_fixtures;

use aidash_server::apps::{
	federation::peer::models::AuthorizationPeerMappingHistory,
	identity::models::AuthorizationCredential,
};
use chrono::{Duration, Utc};
use endpoint::EndpointFixture;
use execution_fixtures::{ExecutionFixture, execution};
use peering::{Bearer, Remote};
use reinhardt::db::orm::Model;
use rstest::{fixture, rstest};
use serde_json::{Value, json};
use std::future::Future;
use uuid::Uuid;

const SOURCE: &str = "aidash://source";

/// The Node of `SOURCE` that sends the requests, and the stand-in Deployment
/// that signs for it.
fn sender() -> String {
	aidash_domain::nodes::NodeName::derived("source").to_string()
}

#[fixture]
fn discovery(
	#[future] execution: ExecutionFixture,
) -> impl Future<Output = (ExecutionFixture, Remote)> {
	let execution = Box::pin(execution);
	async move {
		let execution = execution.await;
		let remote = Remote::start(SOURCE).await;
		let runtime = peering::enable(&execution.app.runtime, &execution.app.context);
		peering::permit(&runtime, &remote.endpoint);
		let owner = crate::endpoint::default_node(&runtime.store, "acme")
			.await
			.node;
		let (status, registered) =
			crate::endpoint::browser_mapping(&execution.app, "acme", "alice")
				.await
				.send(
					"POST",
					&peering::registration_path("acme", owner.as_str()),
					remote.registration(&sender()),
				)
				.await;
		assert_eq!(status, 200, "{registered}");
		(execution, remote)
	}
}

async fn request(
	app: &EndpointFixture,
	token: &str,
	method: &str,
	path: &str,
	value: Value,
) -> (u16, Value) {
	let authorization = format!("Bearer {token}");
	let headers = [("Authorization", authorization.as_str())];
	let response = match method {
		"GET" => async {
			let client = &(app.anonymous);
			let mut request = client.request(http::Method::GET, path);
			for (name, value) in &headers {
				request = request.header(*name, *value);
			}
			request.send().await
		}
		.await
		.unwrap(),
		"POST" => async {
			let client = &(app.anonymous);
			let mut request = client
				.request(http::Method::POST, path)
				.body(bytes::Bytes::copy_from_slice(value.to_string().as_bytes()))
				.header(http::header::CONTENT_TYPE, "application/json");
			for (name, value) in &headers {
				request = request.header(*name, *value);
			}
			request.send().await
		}
		.await
		.unwrap(),
		_ => panic!("unsupported fixture request method: {method}"),
	};
	(response.status_code(), response.json_value().unwrap())
}

async fn receiving_node(
	f: &aidash_server::federation::Federation,
) -> aidash_domain::nodes::NodeName {
	crate::endpoint::default_node(&f.store, "acme").await.node
}

async fn discover(app: &EndpointFixture, bearer: Bearer<'_>, subject: &str) -> (u16, Value) {
	let receiver = crate::endpoint::default_node(&app.runtime.store, "acme").await;
	let response = peering::post_as(
		&app.anonymous,
		&receiver.to_string(),
		bearer,
		subject,
		"/federation/v0.1/scoped/discover",
		&json!({"search":{}}),
	)
	.await;
	(response.status_code(), response.json_value().unwrap())
}

#[rstest]
#[tokio::test]
async fn inbound_discovery_requires_exact_mapping_and_current_local_authority(
	#[future] discovery: (ExecutionFixture, Remote),
) {
	let (execution, remote) = Box::pin(discovery).await;
	let app = &execution.app;
	let f = &app.runtime;
	let mut policy = execution.policy;
	// A peer record is a fixture prerequisite, not a substitute for the inbound
	// middleware: every discovery below passes through real authentication.
	assert_eq!(
		discover(app, Bearer::Signed(&remote, sender()), "bob")
			.await
			.0,
		403
	);
	let subject_token = execution.subject_token.as_str();
	// Peer Mappings are administered through a Tenant Administrator Mapping, which
	// binds only the credential of a service or agent subject (#484).
	// A non-user subject names the Node that owns it, here the receiving Node.
	policy["subjects"]["peer"] = json!({"kind":"service","node":receiving_node(f).await});
	assert_eq!(
		request(
			app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":policy})
		)
		.await
		.0,
		200
	);
	let acme_admin = endpoint::browser_mapping(app, "acme", "alice").await;
	let (status, issued) = acme_admin
		.send(
			"POST",
			"/api/authorization/acme/credentials",
			json!({"subject":"peer"}),
		)
		.await;
	assert_eq!(status, 200, "{issued}");
	let credential = &issued["credential"]["id"];
	let mut mapping = json!({"node":receiving_node(f).await,"source_node":remote.node(&sender()),"source_subject":"bob","credential_id":credential,"enabled":true,"expected_revision":0});
	let path = "/api/authorization/acme/peer-mappings";
	assert_eq!(
		request(app, &f.config.api_token, "POST", path, mapping.clone())
			.await
			.0,
		403
	);
	let (status, binding) = acme_admin.send("POST", path, mapping.clone()).await;
	assert_eq!(status, 200, "{binding}");
	assert_eq!(binding["revision"], 1);
	let (status, mappings) = acme_admin
		.send(
			"GET",
			"/api/authorization/acme/peer-mappings?offset=0&limit=1",
			json!({}),
		)
		.await;
	assert_eq!(status, 200, "{mappings}");
	assert_eq!(mappings, json!([binding]));
	let (status, mappings) = acme_admin
		.send(
			"GET",
			"/api/authorization/acme/peer-mappings?offset=1&limit=1",
			json!({}),
		)
		.await;
	assert_eq!(status, 200, "{mappings}");
	assert_eq!(mappings, json!([]));
	// Another Tenant's mappings are not an acme Subject's to read.
	assert_eq!(
		acme_admin
			.send(
				"GET",
				"/api/authorization/other/peer-mappings?limit=1",
				json!({})
			)
			.await
			.0,
		403
	);
	assert_eq!(
		request(app, &f.config.api_token, "GET", path, json!({}))
			.await
			.0,
		403
	);
	for path in [
		"/api/authorization/acme/peer-mappings?offset=-1",
		"/api/authorization/acme/peer-mappings?limit=0",
		"/api/authorization/acme/peer-mappings?limit=201",
		"/api/authorization/acme/peer-mapping-history?after=-1",
		"/api/authorization/acme/peer-mapping-history?limit=0",
	] {
		let (status, body) = acme_admin.send("GET", path, json!({})).await;
		assert_eq!(status, 400, "{path}: {body}");
	}
	assert_eq!(acme_admin.send("POST", path, mapping.clone()).await.0, 409);
	let stranger = Remote::start(SOURCE).await;
	let unregistered = aidash_domain::nodes::NodeName::derived("forged").to_string();
	for (bearer, subject, expected) in [
		(Bearer::Signed(&remote, sender()), "bob", 200),
		(Bearer::Signed(&remote, sender()), "alice", 403),
		// A Node nobody registered, and a key nobody pinned, answer alike.
		(Bearer::Signed(&remote, unregistered), "bob", 401),
		(Bearer::Signed(&stranger, sender()), "bob", 401),
		(Bearer::Raw(subject_token), "bob", 401),
		(Bearer::Raw(f.config.api_token.as_str()), "bob", 401),
	] {
		let (status, body) = discover(app, bearer, subject).await;
		assert_eq!(status, expected, "{subject}: {body}");
		if status == 200 {
			assert_eq!(body.as_array().unwrap().len(), 1);
			assert_eq!(body[0]["id"], "research");
		}
	}
	mapping["expected_revision"] = json!(1);
	mapping["enabled"] = json!(false);
	assert_eq!(acme_admin.send("POST", path, mapping.clone()).await.0, 200);
	assert_eq!(
		discover(app, Bearer::Signed(&remote, sender()), "bob")
			.await
			.0,
		403
	);
	mapping["expected_revision"] = json!(2);
	mapping["enabled"] = json!(true);
	assert_eq!(acme_admin.send("POST", path, mapping.clone()).await.0, 200);
	// Tenant approval is independent of the mapped subject's wildcard policy.
	aidash_server::authorization::Authorization {
		pool: f.store.pool.clone(),
	}
	.set_catalog(
		"acme",
		&aidash_domain::nodes::NodeName::derived("acme"),
		&aidash_server::registry::EntityRef {
			id: "research".into(),
			version: "1.0.0".into(),
		},
		1,
		false,
		"deployment_administrator",
	)
	.await
	.unwrap();
	assert_eq!(
		discover(app, Bearer::Signed(&remote, sender()), "bob")
			.await
			.1,
		json!([])
	);
	aidash_server::authorization::Authorization {
		pool: f.store.pool.clone(),
	}
	.set_catalog(
		"acme",
		&aidash_domain::nodes::NodeName::derived("acme"),
		&aidash_server::registry::EntityRef {
			id: "research".into(),
			version: "1.0.0".into(),
		},
		2,
		true,
		"deployment_administrator",
	)
	.await
	.unwrap();
	// An explicit deny on the mapped local identity filters the remote result.
	policy["policies"].as_array_mut().unwrap().push(json!({"id":"deny-agent","effect":"deny","subjects":{"any":true},"actions":["agent.execute"],"resources":{"kinds":["agent"]}}));
	assert_eq!(
		request(
			app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":2,"bundle":policy})
		)
		.await
		.0,
		200
	);
	let (status, entries) = discover(app, Bearer::Signed(&remote, sender()), "bob").await;
	assert_eq!(status, 200);
	assert_eq!(entries, json!([]));
	// Credential revocation invalidates even an enabled, previously used map.
	assert_eq!(
		acme_admin
			.send(
				"POST",
				&format!(
					"/api/authorization/acme/credentials/{}/revoke",
					credential.as_str().unwrap()
				),
				json!({})
			)
			.await
			.0,
		200
	);
	assert_eq!(
		discover(app, Bearer::Signed(&remote, sender()), "bob")
			.await
			.0,
		403
	);
	mapping["expected_revision"] = json!(3);
	assert_eq!(acme_admin.send("POST", path, mapping.clone()).await.0, 403);
	mapping["enabled"] = json!(false);
	assert_eq!(acme_admin.send("POST", path, mapping).await.0, 200);
	let count = app
		.database
		.fixture(async |tx| {
			Ok(AuthorizationPeerMappingHistory::objects()
				.all()
				.all_with_db(tx)
				.await?)
		})
		.await
		.len();
	assert_eq!(count, 4, "failed writes must not create history");
	let (status, first) = acme_admin
		.send(
			"GET",
			"/api/authorization/acme/peer-mapping-history?limit=2",
			json!({}),
		)
		.await;
	assert_eq!(status, 200);
	assert_eq!(first.as_array().unwrap().len(), 2);
	assert_eq!(first[0]["revision"], 1);
	let (_, second) = acme_admin
		.send(
			"GET",
			&format!(
				"/api/authorization/acme/peer-mapping-history?after={}&limit=2",
				first[1]["sequence"]
			),
			json!({}),
		)
		.await;
	assert_eq!(second.as_array().unwrap().len(), 2);
	assert_eq!(second[0]["revision"], 3);
	assert_eq!(
		request(
			app,
			&f.config.api_token,
			"GET",
			"/api/authorization/acme/peer-mapping-history",
			json!({})
		)
		.await
		.0,
		403
	);
	assert_eq!(
		acme_admin
			.send(
				"GET",
				"/api/authorization/acme/peer-mapping-history?limit=201",
				json!({})
			)
			.await
			.0,
		400
	);
}

#[rstest]
#[tokio::test]
async fn mappings_cannot_cross_tenants_and_expiry_or_disabled_subject_denies_discovery(
	#[future] discovery: (ExecutionFixture, Remote),
) {
	let (execution, remote) = Box::pin(discovery).await;
	let app = &execution.app;
	let f = &app.runtime;
	let mut policy = execution.policy;
	// A non-user subject names the Node that owns it, here the receiving Node.
	policy["subjects"]["peer"] = json!({"kind":"service","node":receiving_node(f).await});
	assert_eq!(
		request(
			app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":policy})
		)
		.await
		.0,
		200
	);
	// The Mapping binds the credential of a service subject (#484).
	let acme_admin = endpoint::browser_mapping(app, "acme", "alice").await;
	let (status, issued) = acme_admin
		.send(
			"POST",
			"/api/authorization/acme/credentials",
			json!({"subject":"peer"}),
		)
		.await;
	assert_eq!(status, 200, "{issued}");
	let credential = issued["credential"]["id"].clone();
	let mapping = json!({"node":receiving_node(f).await,"source_node":remote.node(&sender()),"source_subject":"bob","credential_id":credential,"enabled":true,"expected_revision":0});
	let mut other_policy = policy.clone();
	other_policy["tenant"] = json!("other");
	crate::endpoint::tenant_root(f, "other").await;
	// The other Tenant's service subject is owned by that Tenant's own Node.
	other_policy["subjects"]["peer"]["node"] =
		json!(crate::endpoint::default_node(&f.store, "other").await.node);
	assert_eq!(
		request(
			app,
			&f.config.api_token,
			"POST",
			"/api/authorization/other",
			json!({"expected_revision":0,"bundle":other_policy})
		)
		.await
		.0,
		200
	);
	let other_admin = endpoint::browser_mapping(app, "other", "alice").await;
	assert_eq!(
		other_admin
			.send(
				"POST",
				"/api/authorization/other/peer-mappings",
				mapping.clone()
			)
			.await
			.0,
		403
	);
	assert_eq!(
		acme_admin
			.send(
				"POST",
				"/api/authorization/acme/peer-mappings",
				mapping.clone()
			)
			.await
			.0,
		200
	);
	let (status, other_issued) = other_admin
		.send(
			"POST",
			"/api/authorization/other/credentials",
			json!({"subject":"peer"}),
		)
		.await;
	assert_eq!(status, 200, "{other_issued}");
	// The same sending subject may be mapped at another Tenant's Node once that
	// Node registers the sender; a conflict within one Node answers 409.
	let other_node = crate::endpoint::default_node(&f.store, "other").await.node;
	assert_eq!(
		other_admin
			.send(
				"POST",
				&peering::registration_path("other", other_node.as_str()),
				remote.registration(&sender())
			)
			.await
			.0,
		200
	);
	let mut other_mapping = mapping.clone();
	other_mapping["node"] = json!(other_node);
	other_mapping["credential_id"] = other_issued["credential"]["id"].clone();
	assert_eq!(
		other_admin
			.send(
				"POST",
				"/api/authorization/other/peer-mappings",
				other_mapping.clone()
			)
			.await
			.0,
		200
	);
	assert_eq!(
		other_admin
			.send(
				"POST",
				"/api/authorization/other/peer-mappings",
				other_mapping
			)
			.await
			.0,
		409
	);
	policy["subjects"]["peer"]["enabled"] = json!(false);
	assert_eq!(
		request(
			app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":2,"bundle":policy})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		discover(app, Bearer::Signed(&remote, sender()), "bob")
			.await
			.0,
		403
	);
	policy["subjects"]["peer"]["enabled"] = json!(true);
	policy["policies"][0]["condition"] = json!({"op":"eq","left":{"source":"environment","path":"/transport"},"right":{"source":"literal","value":"federation"}});
	assert_eq!(
		request(
			app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":3,"bundle":policy})
		)
		.await
		.0,
		200
	);
	assert_eq!(
		discover(app, Bearer::Signed(&remote, sender()), "bob")
			.await
			.0,
		200
	);
	// Expiry is a durable database clock check, including for active mappings.
	let id = Uuid::parse_str(credential.as_str().unwrap()).unwrap();
	app.database
		.fixture(async |tx| {
			let mut credential = AuthorizationCredential::objects()
				.filter(AuthorizationCredential::field_id().eq(id))
				.get_with_db(tx)
				.await?;
			credential.created_at = Utc::now() - Duration::hours(2);
			credential.expires_at = Utc::now() - Duration::hours(1);
			AuthorizationCredential::objects()
				.update_with_conn(tx, &credential)
				.await?;
			Ok(())
		})
		.await;
	assert_eq!(
		discover(app, Bearer::Signed(&remote, sender()), "bob")
			.await
			.0,
		403
	);
}

#[rstest]
#[tokio::test]
async fn node_closure_mapping_creation_requires_the_non_user_subjects_owning_node(
	#[future] discovery: (ExecutionFixture, Remote),
) {
	let (execution, remote) = Box::pin(discovery).await;
	let app = &execution.app;
	let f = &app.runtime;
	let mut policy = execution.policy;
	// Nodes, credentials, registrations and Peer Mappings are Tenant
	// administration, held only through a browser Mapping (#484).
	let acme_admin = endpoint::browser_mapping(app, "acme", "alice").await;
	let (status, sibling) = acme_admin
		.send(
			"POST",
			"/api/tenants/acme/nodes",
			json!({"label":"Service owner"}),
		)
		.await;
	assert_eq!(status, 201, "{sibling}");
	let sibling = sibling["name"].as_str().unwrap();
	policy["subjects"]["owned-service"] = json!({"kind":"service", "node":sibling});
	assert_eq!(
		request(
			app,
			&f.config.api_token,
			"POST",
			"/api/authorization/acme",
			json!({"expected_revision":1,"bundle":policy})
		)
		.await
		.0,
		200
	);
	let (status, issued) = acme_admin
		.send(
			"POST",
			"/api/authorization/acme/credentials",
			json!({"subject":"owned-service"}),
		)
		.await;
	assert_eq!(status, 200, "{issued}");
	let mut mapping = json!({"node":receiving_node(f).await,"source_node":remote.node(&sender()),"source_subject":"service","credential_id":issued["credential"]["id"],"enabled":true,"expected_revision":0});
	let path = "/api/authorization/acme/peer-mappings";
	let (status, value) = acme_admin.send("POST", path, mapping.clone()).await;
	assert_eq!(
		status, 403,
		"another Node cannot receive this service's authority: {value}"
	);
	assert_eq!(
		acme_admin
			.send(
				"POST",
				&peering::registration_path("acme", sibling),
				remote.registration(&sender())
			)
			.await
			.0,
		200
	);
	mapping["node"] = json!(sibling);
	assert_eq!(acme_admin.send("POST", path, mapping).await.0, 200);
}
