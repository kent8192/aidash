//! A terminal participant is not proof that its remote coordinator saved its
//! reply. Ordinary Node data and finite audit may expire while this exact
//! cryptographic/routing recovery dependency remains usable after restart.
use super::*;
use crate::fixtures::peering;
use aidash_domain::nodes::NodeAddress;
use aidash_server::apps::identity::services::node_purge;

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
	let original_a = trust_row(&a, &target_a, &b.address).await;
	let original_b = trust_row(&b, &target_b, &a.address).await;
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
	let (status, replay) = peer_call(&a, &b, &a.address, "/transactions/finish", &manifest).await;
	assert_eq!(status, 200, "{replay}");
	assert_eq!(replay["phase"], phase);
	let mut fresh = manifest.clone();
	fresh.id = Uuid::new_v4();
	for path in ["/transactions/reserve", "/transactions/prepare"] {
		let (status, body) = peer_call(&a, &b, &a.address, path, &fresh).await;
		assert!((400..500).contains(&status), "{path}: {status}: {body}");
	}
	let admitted: i64 = sqlx::query_scalar("SELECT count(*) FROM atomic_participants WHERE id=$1")
		.bind(fresh.id)
		.fetch_one(b.f.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(
		admitted, 0,
		"fresh admission never creates a recovery journal"
	);
	let (status, body) =
		peer_call(&a, &b, &keeper_a_address, "/transactions/finish", &manifest).await;
	assert!(
		(400..500).contains(&status),
		"wrong caller: {status}: {body}"
	);
	let mut forged = manifest.clone();
	forged.deadline += Duration::seconds(1);
	let (status, body) = peer_call(&a, &b, &a.address, "/transactions/finish", &forged).await;
	assert!(
		(400..500).contains(&status),
		"wrong manifest: {status}: {body}"
	);
	for (node, removed, kept) in [(&a, wa, workspace_a), (&b, wb, workspace_b)] {
		let counts: (i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM workspaces WHERE id=$1),(SELECT count(*) FROM workspaces WHERE id=$2)").bind(removed).bind(kept).fetch_one(node.f.store.pool.fixture_driver()).await.unwrap();
		assert_eq!(counts, (0, 1), "recovery never resurrects ordinary data");
	}
}
