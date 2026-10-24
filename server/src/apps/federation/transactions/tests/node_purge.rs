//! A terminal participant is not proof that its remote coordinator saved its
//! reply. Ordinary Node data and finite audit may expire while this exact
//! cryptographic/routing recovery dependency remains usable after restart.
use super::*;
use crate::fixtures::peering;
use aidash_application::{
	federation::{
		inbound::{self, Inbound, Proof, RequestDigest},
		key::Signing,
	},
	ports::federation::inbound::{InboundRepository, ReceivingKeyOwner},
};
use aidash_capability::{
	InMemorySigner, TokenSigner,
	federation::{
		FederationClaims, body_digest,
		rollover::{RolloverClaims, mint_transition},
	},
};
use aidash_domain::{
	nodes::{DeploymentKeyId, NodeAddress},
	tenancy::TenantName,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use secrecy::ExposeSecret;
#[path = "../../../execution/tests/support/federation_clock.rs"]
mod federation_clock;
use aidash_server::apps::identity::services::node_purge;
use federation_clock::Clock;

async fn peer_call(
	source: &Node,
	target: &Node,
	sender: &str,
	path: &str,
	manifest: &aidash_server::transactions::Manifest,
) -> (u16, Value) {
	let body = serde_json::to_vec(manifest).unwrap();
	let token = peering::token(
		source.f.config.deployment_key.as_deref().unwrap(),
		source.f.config.deployment.as_str(),
		sender,
		&target.address,
		"",
		("POST", path, &body),
	)
	.await;
	let headers = peering::peer_headers(&token);
	let mut request = target
		.peer_client
		.request(http::Method::POST, &format!("/federation/v0.1{path}"))
		.body(bytes::Bytes::from(body))
		.header(http::header::CONTENT_TYPE, "application/json");
	for (name, value) in &headers {
		request = request.header(*name, value.as_str());
	}
	let response = request.send().await.unwrap();
	(
		response.status_code(),
		response.json_value().unwrap_or(Value::Null),
	)
}

async fn keeper(node: &Node) -> (String, Uuid) {
	let (status, created) = node
		.post("/api/tenants/acme/nodes", &json!({"label":"Keeper"}))
		.await;
	assert_eq!(status, 201, "{created}");
	let name = created["name"].as_str().unwrap().to_owned();
	let (status, body) = node
		.post(
			&format!("/api/tenants/acme/nodes/{name}/default"),
			&json!({}),
		)
		.await;
	assert_eq!(status, 200, "{body}");
	let workspace = node
		.f
		.store
		.create_workspace(
			&aidash_application::tenancy::TenancyScope::tenant("acme").unwrap(),
			"alice",
			"Keeper",
			"Must survive",
		)
		.await
		.unwrap();
	(name, workspace.id)
}

async fn file(node: &Node, workspace: Uuid) -> Uuid {
	let area = Uuid::new_v4();
	let id = Uuid::new_v4();
	let driver = node.f.store.pool.fixture_driver();
	sqlx::query("INSERT INTO core_areas(id,tenant,home_node,agent_id,owner,workspace_id,thread_id) VALUES ($1,'acme',$2,'fixture','alice',$3,$4)")
        .bind(area).bind(&node.address).bind(workspace).bind(Uuid::new_v4()).execute(driver).await.unwrap();
	sqlx::query("INSERT INTO core_objects(id,tenant,area_id,kind,digest,size) VALUES ($1,'acme',$2,'working','fixture',4)").bind(id).bind(area).execute(driver).await.unwrap();
	sqlx::query("INSERT INTO core_quotas(tenant,used_bytes) VALUES ('acme',4) ON CONFLICT(tenant) DO UPDATE SET used_bytes=core_quotas.used_bytes+4").execute(driver).await.unwrap();
	tokio::fs::write(
		node.f
			.store
			.capabilities
			.0
			.storage
			.join(id.simple().to_string()),
		b"data",
	)
	.await
	.unwrap();
	id
}

async fn trust_row(node: &Node, owner: &str, source: &str) -> Value {
	sqlx::query_scalar(
		"SELECT to_jsonb(r) FROM node_registrations r WHERE tenant='acme' AND node=$1 AND source_node=$2",
	)
	.bind(owner)
	.bind(source)
	.fetch_one(node.f.store.pool.fixture_driver())
	.await
	.unwrap()
}

async fn disable(node: &Node, target: &str, source: &str) -> Value {
	let mut value = peering::registration(source, None);
	value["enabled"] = json!(false);
	value["atomic_transactions"] = json!(false);
	value["expected_revision"] = json!(1);
	let (status, body) = node
		.post(&peering::registration_path("acme", target), &value)
		.await;
	assert_eq!(status, 200, "{body}");
	let row = trust_row(node, target, source).await;
	assert_eq!(row["enabled"], false);
	assert_eq!(row["atomic_transactions"], false);
	assert_eq!(row["revision"], 2);
	row
}

async fn signed_call(
	target: &Node,
	claims: &FederationClaims,
	signer: &dyn TokenSigner,
	body: &[u8],
) -> (u16, Value) {
	let token = claims.mint(signer).await.unwrap();
	let mut request = target
		.peer_client
		.request(
			http::Method::from_bytes(claims.method.as_bytes()).unwrap(),
			&format!("/federation/v0.1{}", claims.path),
		)
		.body(bytes::Bytes::copy_from_slice(body))
		.header(http::header::CONTENT_TYPE, "application/json");
	for (name, value) in peering::peer_headers(token.expose_secret()) {
		request = request.header(name, value.as_str());
	}
	let response = request.send().await.unwrap();
	(
		response.status_code(),
		response.json_value().unwrap_or(Value::Null),
	)
}

async fn successor(
	source: &Node,
	target: &Node,
	method: &str,
	path: &str,
	body: &[u8],
	seed: u8,
) -> (InMemorySigner, FederationClaims, u64) {
	let raw = InMemorySigner::new("temporary".into(), [seed; 32]);
	let next = InMemorySigner::new(
		DeploymentKeyId::of_public_key(&raw.public_key().to_bytes()).to_string(),
		[seed; 32],
	);
	let old = source.f.config.deployment_key.as_deref().unwrap();
	let now = Utc::now().timestamp() as u64;
	let retires = now - 3600 + 604800;
	let transition = mint_transition(
		&RolloverClaims {
			deployment: source.f.config.deployment.to_string(),
			predecessor_kid: old.key_id().to_string(),
			successor_kid: next.kid().into(),
			successor_public_key: URL_SAFE_NO_PAD.encode(next.public_key().to_bytes()),
			published_at: now - 90000,
			activates_at: now - 3600,
			retires_at: retires,
		},
		&Signing(old),
	)
	.await
	.unwrap();
	let claims = FederationClaims {
		iss: source.f.config.deployment.to_string(),
		sub: source.address.clone(),
		aud: target.address.clone(),
		subject: String::new(),
		iat: now,
		exp: now + 60,
		jti: Uuid::new_v4(),
		kid: next.kid().into(),
		protocol: aidash_domain::federation::protocol::CURRENT.into(),
		method: method.into(),
		path: path.into(),
		body: body_digest(body),
		transition: Some(transition),
	};
	(next, claims, retires)
}

async fn retired_admission(
	source: &Node,
	target: &Node,
	claims: &FederationClaims,
	signer: &dyn TokenSigner,
	body: &[u8],
	after: u64,
) -> aidash_application::Result<aidash_application::federation::inbound::Admitted> {
	let token = FederationClaims {
		iat: after,
		exp: after + 60,
		jti: Uuid::new_v4(),
		transition: None,
		..claims.clone()
	}
	.mint(signer)
	.await
	.unwrap();
	inbound::accept(
		Inbound {
			receiver: NodeAddress::parse(&target.address).unwrap(),
			sender: NodeAddress::parse(&source.address).unwrap(),
			subject: String::new(),
			proof: Proof::Token(token.expose_secret().to_owned()),
		},
		&RequestDigest {
			method: &claims.method,
			path: &claims.path,
			body,
		},
		&Clock(&target.f, after),
	)
	.await
}

async fn purge_and_expire(node: &Node, target: &str) {
	let path = format!("/api/tenants/acme/nodes/{target}");
	let (status, deleted) = node
		.post(&format!("{path}/deletion"), &json!({"confirmation":target}))
		.await;
	assert_eq!(status, 200, "{deleted}");
	let (status, purging) = node.post(&format!("{path}/purge"), &json!({})).await;
	assert_eq!(status, 200, "{purging}");
	let deletion = node_purge::claim(&node.f.store)
		.await
		.unwrap()
		.into_iter()
		.find(|row| row.node == target)
		.unwrap();
	node_purge::purge_node(&node.f.store, deletion)
		.await
		.unwrap();
	sqlx::query("UPDATE node_deletions SET purged_at=clock_timestamp()-interval '366 days' WHERE tenant='acme' AND node=$1 AND purged_at IS NOT NULL").bind(target).execute(node.f.store.pool.fixture_driver()).await.unwrap();
	node_purge::erase_audit(&node.f.store, 365).await.unwrap();
	for table in [
		"nodes",
		"node_deletions",
		"node_registration_history",
		"authorization_catalog_history",
		"dashboard_administration_history",
	] {
		let key = if table == "nodes" { "name" } else { "node" };
		let count: i64 = sqlx::query_scalar(&format!(
			"SELECT count(*) FROM {table} WHERE tenant='acme' AND {key}=$1"
		))
		.bind(target)
		.fetch_one(node.f.store.pool.fixture_driver())
		.await
		.unwrap();
		assert_eq!(count, 0, "{table}");
	}
	let count: i64 =
		sqlx::query_scalar("SELECT count(*) FROM node_names WHERE tenant='acme' AND name=$1")
			.bind(target)
			.fetch_one(node.f.store.pool.fixture_driver())
			.await
			.unwrap();
	assert_eq!(count, 1);
	let (status, live) = node.get("/api/tenants/acme/nodes").await;
	assert_eq!(status, 200, "{live}");
	assert!(
		live.as_array()
			.unwrap()
			.iter()
			.all(|row| row["name"] != target)
	);
}

#[rstest]
#[case::committed(false)]
#[case::aborted(true)]
#[tokio::test]
async fn node_purge_terminal_reply_retries_after_physical_erasure_audit_expiry_and_restart(
	#[future] pair: Option<Pair>,
	#[case] abort: bool,
) {
	let Some((mut a, mut b, manifest, wa, wb)) = Box::pin(pair).await else {
		return;
	};
	let storage_a = tempfile::tempdir().unwrap();
	let storage_b = tempfile::tempdir().unwrap();
	for (node, storage) in [(&mut a, &storage_a), (&mut b, &storage_b)] {
		node.f.store.capabilities = aidash_server::capabilities::services::core::Runtime::new(
			aidash_server::capabilities::services::core::Profile {
				storage: storage.path().to_owned(),
				..Default::default()
			},
		)
		.unwrap();
	}
	let target_a = NodeAddress::parse(&a.address).unwrap().node.to_string();
	let target_b = NodeAddress::parse(&b.address).unwrap().node.to_string();
	let (keeper_a, workspace_a) = keeper(&a).await;
	let (_, workspace_b) = keeper(&b).await;
	let keeper_a_address =
		a.f.config
			.deployment
			.node(aidash_domain::nodes::NodeName::parse(&keeper_a).unwrap())
			.to_string();
	let (status, body) = b
		.post(
			&peering::registration_path("acme", &target_b),
			&peering::registration(
				&keeper_a_address,
				Some((&a.f.config.endpoint, &peering::key_id(&a.f))),
			),
		)
		.await;
	assert_eq!(status, 200, "{body}");
	let (target_file_a, kept_file_a) = (file(&a, wa).await, file(&a, workspace_a).await);
	let (target_file_b, kept_file_b) = (file(&b, wb).await, file(&b, workspace_b).await);
	a.submit(&manifest).await;
	if abort {
		steps(&a, manifest.id, 2).await;
		coordinator::abort(&a.f, manifest.id).await.unwrap();
	} else {
		for _ in 0..16 {
			if a.status(manifest.id).await.visible {
				break;
			}
			steps(&a, manifest.id, 1).await;
		}
		assert!(a.status(manifest.id).await.visible);
	}
	// Use actual signed peer HTTP and the real reverse GET-decision owner, but
	// deliberately do not run the coordinator's vote persistence. This is the
	// durable lost-before-acknowledgement schedule, not a process-kill claim.
	let phase = if abort { "ABORTED" } else { "COMMITTED" };
	let (status, reply) = peer_call(&a, &b, &a.address, "/transactions/finish", &manifest).await;
	assert_eq!(status, 200, "{reply}");
	assert_eq!(reply["phase"], phase);
	assert!(!a.status(manifest.id).await.complete);
	let acknowledged: i64 = sqlx::query_scalar(
		"SELECT count(*) FROM atomic_votes WHERE transaction_id=$1 AND node_id=$2 AND phase=$3",
	)
	.bind(manifest.id)
	.bind(&b.address)
	.bind(phase)
	.fetch_one(a.f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(
		acknowledged, 0,
		"terminal reply was not saved at the coordinator"
	);
	let original_b = disable(&b, &target_b, &a.address).await;
	purge_and_expire(&b, &target_b).await;
	assert!(
		!storage_b
			.path()
			.join(target_file_b.simple().to_string())
			.exists()
	);
	assert!(
		storage_b
			.path()
			.join(kept_file_b.simple().to_string())
			.exists()
	);
	assert_eq!(trust_row(&b, &target_b, &a.address).await, original_b);
	let unrelated: i64 = sqlx::query_scalar(
		"SELECT count(*) FROM node_registrations WHERE tenant='acme' AND node=$1 AND source_node=$2",
	)
	.bind(&target_b)
	.bind(&keeper_a_address)
	.fetch_one(b.f.store.pool.fixture_driver())
	.await
	.unwrap();
	assert_eq!(unrelated, 0);
	b.restart().await;
	let done = complete(&a, manifest.id).await;
	assert!(done.complete);
	assert_eq!(
		done.decision.as_deref(),
		Some(if abort { "ABORT" } else { "COMMIT" })
	);
	// A completed coordinator journal still serves exact decision retries.
	let original_a = disable(&a, &target_a, &b.address).await;
	purge_and_expire(&a, &target_a).await;
	assert!(
		!storage_a
			.path()
			.join(target_file_a.simple().to_string())
			.exists()
	);
	assert!(
		storage_a
			.path()
			.join(kept_file_a.simple().to_string())
			.exists()
	);
	assert_eq!(trust_row(&a, &target_a, &b.address).await, original_a);
	a.restart().await;
	let body = serde_json::to_vec(&manifest).unwrap();
	let (next_a, finish_claims, retires_a) =
		successor(&a, &b, "POST", "/transactions/finish", &body, 31).await;
	// All negatives carry a valid successor proof after both catalog/audit rows
	// are actually absent. Refusal must precede any retained-trust mutation.
	let mut fresh = manifest.clone();
	fresh.id = Uuid::new_v4();
	let mut forged = manifest.clone();
	forged.deadline += Duration::seconds(1);
	for (path, request, sender) in [
		("/transactions/reserve", &fresh, a.address.as_str()),
		("/transactions/prepare", &fresh, a.address.as_str()),
		("/transactions/finish", &forged, a.address.as_str()),
		("/transactions/finish", &manifest, keeper_a_address.as_str()),
	] {
		let bytes = serde_json::to_vec(request).unwrap();
		let denied = FederationClaims {
			path: path.into(),
			sub: sender.into(),
			body: body_digest(&bytes),
			jti: Uuid::new_v4(),
			..finish_claims.clone()
		};
		let (status, value) = signed_call(&b, &denied, &next_a, &bytes).await;
		assert_eq!(status, 401, "{path}: {value}");
		assert_eq!(trust_row(&b, &target_b, &a.address).await, original_b);
	}
	let admitted: i64 = sqlx::query_scalar("SELECT count(*) FROM atomic_participants WHERE id=$1")
		.bind(fresh.id)
		.fetch_one(b.f.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(admitted, 0, "fresh work creates no journal");
	let (status, replay) = signed_call(&b, &finish_claims, &next_a, &body).await;
	assert_eq!(status, 200, "physical-erasure successor Finish: {replay}");
	assert_eq!(replay["phase"], phase);
	// The reverse decision owner independently learns the participant's new key.
	let decision_path = format!("/transactions/{}/decision", manifest.id);
	let (next_b, decision_claims, retires_b) =
		successor(&b, &a, "GET", &decision_path, &[], 32).await;
	let (status, decision) = signed_call(&a, &decision_claims, &next_b, &[]).await;
	assert_eq!(
		status, 200,
		"physical-erasure successor decision: {decision}"
	);
	let owner_b = ReceivingKeyOwner::Registration {
		tenant: TenantName::new("acme").unwrap(),
		node: NodeAddress::parse(&b.address).unwrap().node,
		source_node: NodeAddress::parse(&a.address).unwrap(),
	};
	let owner_a = ReceivingKeyOwner::Registration {
		tenant: TenantName::new("acme").unwrap(),
		node: NodeAddress::parse(&a.address).unwrap().node,
		source_node: NodeAddress::parse(&b.address).unwrap(),
	};
	let learned_b = b.f.receiving_keys(&owner_b).await.unwrap().unwrap();
	let learned_a = a.f.receiving_keys(&owner_a).await.unwrap().unwrap();
	for (learned, expected) in [(&learned_b, next_a.kid()), (&learned_a, next_b.kid())] {
		assert_eq!(learned.keys.current.key_id.as_str(), expected);
		assert_eq!(learned.keys_revision, 2);
	}
	for (node, target, source, before) in [
		(&a, &target_a, &b.address, &original_a),
		(&b, &target_b, &a.address, &original_b),
	] {
		let after = trust_row(node, target, source).await;
		for field in [
			"enabled",
			"atomic_transactions",
			"revision",
			"actor",
			"created_at",
			"updated_at",
			"source_endpoint",
		] {
			assert_eq!(
				after.get(field).unwrap(),
				before.get(field).unwrap(),
				"{field}"
			);
		}
		assert_eq!(after["revision"], 2);
		assert_eq!(after["enabled"], false);
	}
	a.restart().await;
	b.restart().await;
	assert_eq!(
		a.f.receiving_keys(&owner_a).await.unwrap().unwrap(),
		learned_a
	);
	assert_eq!(
		b.f.receiving_keys(&owner_b).await.unwrap().unwrap(),
		learned_b
	);
	// Only admission time is virtual. Restarted production adapters retain all
	// authority/key reads; actual terminal work fetches reverse decision HTTP
	// at wall-clock time, not a claim of a future HTTP listener.
	let after = retires_a.max(retires_b) + 1;
	let admission = retired_admission(&a, &b, &finish_claims, &next_a, &body, after)
		.await
		.unwrap();
	let terminal = participant::finish(&b.f, Some(&admission), &a.address, &manifest)
		.await
		.unwrap();
	assert_eq!(terminal.phase, phase);
	retired_admission(&b, &a, &decision_claims, &next_b, &[], after)
		.await
		.unwrap();
	for (source, target, current, bytes) in [
		(&a, &b, &finish_claims, body.as_slice()),
		(&b, &a, &decision_claims, &[][..]),
	] {
		let old = source.f.config.deployment_key.as_deref().unwrap();
		let old_claims = FederationClaims {
			kid: old.key_id().to_string(),
			..current.clone()
		};
		assert!(matches!(
			retired_admission(source, target, &old_claims, &Signing(old), bytes, after).await,
			Err(aidash_application::Error::Unauthorized)
		));
	}
	// A fixture quarantine models retained current state. This is not manual
	// 8b API acceptance and never clears quarantine or restores authority.
	let mut quarantined = learned_b.keys.clone();
	quarantined.quarantine();
	sqlx::query("UPDATE node_registrations SET source_keys=$1,keys_revision=keys_revision+1 WHERE tenant='acme' AND node=$2 AND source_node=$3")
        .bind(serde_json::to_string(&quarantined).unwrap()).bind(&target_b).bind(&a.address).execute(b.f.store.pool.fixture_driver()).await.unwrap();
	let held = trust_row(&b, &target_b, &a.address).await;
	let denied = FederationClaims {
		jti: Uuid::new_v4(),
		transition: None,
		..finish_claims.clone()
	};
	let (status, value) = signed_call(&b, &denied, &next_a, &body).await;
	assert_eq!(status, 401, "quarantined exact recovery refuses: {value}");
	assert_eq!(trust_row(&b, &target_b, &a.address).await, held);
	for (node, removed, kept) in [(&a, wa, workspace_a), (&b, wb, workspace_b)] {
		let counts: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM workspaces WHERE id=$1),(SELECT count(*) FROM workspaces WHERE id=$2)").bind(removed).bind(kept).fetch_one(node.f.store.pool.fixture_driver()).await.unwrap();
		assert_eq!(counts, (0, 1), "recovery never resurrects ordinary data");
	}
}
