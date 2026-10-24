//! Node purge's primary ownership and physical-erasure boundary (ADR 0083).
use super::*;
use aidash_server::apps::identity::services::node_purge;
use aidash_server::capabilities::services::core::{Profile, Runtime};

/// The worker owns cleanup, so its fixture records an already committed deletion
/// directly. Lifecycle HTTP transitions have their own primary tests.
async fn request_purge(d: &Deployment, tenant: &str, node: &str) -> Uuid {
	let id = Uuid::new_v4();
	sqlx::query("INSERT INTO node_deletions (id,tenant,node,deleted_at,deleted_by_kind,purge_after,purge_step,purge_failures) VALUES ($1,$2,$3,clock_timestamp(),'deployment_administrator',clock_timestamp()+interval '30 days',$4,0)")
		.bind(id).bind(tenant).bind(node).bind(aidash_domain::nodes::deletion::PurgeStep::FIRST.as_str())
		.execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	id
}

async fn object(d: &Deployment, tenant: &str, workspace: Uuid, file: Uuid) {
	let area = Uuid::new_v4();
	sqlx::query("INSERT INTO core_areas (id,tenant,home_node,agent_id,owner,workspace_id,thread_id) VALUES ($1,$2,'fixture-home','agent','owner',$3,$4)")
		.bind(area).bind(tenant).bind(workspace).bind(Uuid::new_v4())
		.execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	sqlx::query("INSERT INTO core_objects (id,tenant,area_id,kind,digest,size) VALUES ($1,$2,$3,'working','digest',4)")
		.bind(file).bind(tenant).bind(area)
		.execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	sqlx::query("INSERT INTO core_quotas (tenant,used_bytes) VALUES ($1,4) ON CONFLICT (tenant) DO UPDATE SET used_bytes=core_quotas.used_bytes+4")
		.bind(tenant).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	tokio::fs::write(
		d.federation
			.store
			.capabilities
			.0
			.storage
			.join(file.simple().to_string()),
		b"data",
	)
	.await
	.unwrap();
}

async fn subjects(
	d: &Deployment,
	tenant: &str,
	target: &str,
	sibling: &str,
) -> (Uuid, Uuid, String) {
	let path = format!("/api/authorization/{tenant}");
	let (status, snapshot) = d
		.as_deployment_administrator("GET", &path, Value::Null)
		.await;
	assert_eq!(status, 200, "{snapshot}");
	let mut bundle = snapshot["bundle"].clone();
	let user = bundle["subjects"]
		.as_object()
		.unwrap()
		.iter()
		.find(|(_, s)| s["kind"] == "user")
		.unwrap()
		.0
		.clone();
	for (subject, node) in [("purge-target", target), ("purge-sibling", sibling)] {
		bundle["subjects"][subject] = json!({"kind":"service","node":node,"groups":["members"]});
	}
	let (status, response) = d
		.as_deployment_administrator(
			"POST",
			&path,
			json!({"expected_revision":snapshot["revision"],"bundle":bundle}),
		)
		.await;
	assert_eq!(status, 200, "{response}");
	let mut ids = Vec::new();
	for subject in ["purge-target", "purge-sibling"] {
		let (status, response) = d
			.as_deployment_administrator(
				"POST",
				&format!("{path}/credentials"),
				json!({"subject":subject}),
			)
			.await;
		assert_eq!(status, 200, "{response}");
		ids.push(Uuid::parse_str(response["credential"]["id"].as_str().unwrap()).unwrap());
	}
	(ids[0], ids[1], user)
}
async fn provisional(d: &Deployment, tenant: &str, owner: &str) -> Uuid {
	let file = Uuid::new_v4();
	let record = Uuid::new_v4();
	sqlx::query("INSERT INTO core_objects (id,tenant,kind,digest,size) VALUES ($1,$2,'reference_original','digest',4)").bind(file).bind(tenant).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	let descriptor = json!({"file_id":file,"path":"original","digest":"digest","size":4,"media_type":"text/plain","scope":"references","provenance":{"kind":"reference","reference_id":record}});
	sqlx::query("INSERT INTO core_records (id,tenant,owner,kind,state,revision,data) VALUES ($1,$2,$3,'reference','ready',1,$4)").bind(record).bind(tenant).bind(owner).bind(json!({"original":descriptor,"chunks":[]})).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	sqlx::query("INSERT INTO core_quotas (tenant,used_bytes) VALUES ($1,4) ON CONFLICT (tenant) DO UPDATE SET used_bytes=core_quotas.used_bytes+4").bind(tenant).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	tokio::fs::write(
		d.federation
			.store
			.capabilities
			.0
			.storage
			.join(file.simple().to_string()),
		b"data",
	)
	.await
	.unwrap();
	file
}
async fn shared_secrets(d: &Deployment, tenant: &str) -> (Uuid, Uuid) {
	let provider = Uuid::now_v7();
	let secret = Uuid::now_v7();
	sqlx::query("INSERT INTO provider_credentials (id,tenant,provider,base_url,secret_resource,pinned_version,fingerprint,last4,state,created_at,revision) VALUES ($1,$2,'openrouter','https://openrouter.ai/api/v1','fixture-provider-resource','fixture-provider-resource/versions/1','0123456789abcdef','1234','active',clock_timestamp(),1)").bind(provider).bind(tenant).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	sqlx::query("INSERT INTO tenant_secrets (id,tenant,name,hosts,secret_resource,pinned_version,fingerprint,last4,state,created_at,updated_at,revision) VALUES ($1,$2,'shared',ARRAY['example.com'],'aidash-store/tenant-secrets/'||$1::text,'aidash-store/tenant-secrets/'||$1::text||'/versions/1','0123456789abcdef','1234','active',clock_timestamp(),clock_timestamp(),1)").bind(secret).bind(tenant).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	(provider, secret)
}

#[rstest::rstest]
#[tokio::test]
async fn node_purge_erases_owned_files_and_rows_and_preserves_sibling_and_tenant_shared_state(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	// A misplaced Tenant selector would erase the sibling; a missing Tenant
	// scope would erase the foreign fixture. Files prove cleanup is physical.
	let mut d = Deployment::new(
		runtime,
		TenancyRules {
			nodes_per_tenant: 2,
			..open_rules()
		},
	)
	.await;
	let storage = tempfile::tempdir().unwrap();
	d.federation.store.capabilities = Runtime::new(Profile {
		storage: storage.path().to_owned(),
		..Profile::default()
	})
	.unwrap();
	let alice = d.person("alice").await;
	let bob = d.person("bob").await;
	let (tenant, context) = d.node_tenant(&alice, "Team").await;
	let sibling = d.nodes_of(&alice, &context, &tenant).await[0]["name"]
		.as_str()
		.unwrap()
		.to_owned();
	let (_, target) = d.create_node(&alice, &context, &tenant, "Delete").await;
	let target = target["name"].as_str().unwrap().to_owned();
	let (foreign_tenant, foreign_context) = d.node_tenant(&bob, "Other").await;
	let foreign = d.nodes_of(&bob, &foreign_context, &foreign_tenant).await[0]["name"]
		.as_str()
		.unwrap()
		.to_owned();
	let mut work = Vec::new();
	for (person, context, tenant, node) in [
		(&alice, &context, &tenant, &target),
		(&alice, &context, &tenant, &sibling),
		(&bob, &foreign_context, &foreign_tenant, &foreign),
	] {
		let (status, workspace) = d.workspace_on(person, context, Some(node)).await;
		assert_eq!(status, 200, "{workspace}");
		let workspace = Uuid::parse_str(workspace["id"].as_str().unwrap()).unwrap();
		let file = Uuid::new_v4();
		object(&d, tenant, workspace, file).await;
		work.push((workspace, file));
	}
	let (target_credential, sibling_credential, user) =
		subjects(&d, &tenant, &target, &sibling).await;
	let (provider, secret) = shared_secrets(&d, &tenant).await;
	let (target_run, sibling_run, sibling_operation, target_operation, descendant) =
		cross_node_execution_evidence(
			&d,
			&tenant,
			&target,
			&sibling,
			work[0].0,
			work[1].0,
			target_credential,
			sibling_credential,
		)
		.await;

	// A present sibling Workspace outranks a target Subject on an area.
	sqlx::query("UPDATE core_areas SET owner='purge-target' WHERE tenant=$1")
		.bind(&tenant)
		.execute(d.federation.store.pool.fixture_driver())
		.await
		.unwrap();
	let owned_orphan = Uuid::new_v4();
	object(&d, &tenant, Uuid::new_v4(), owned_orphan).await;
	sqlx::query("UPDATE core_areas SET owner='purge-target' WHERE tenant=$1 AND workspace_id NOT IN (SELECT id FROM workspaces WHERE tenant=$1)").bind(&tenant).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	let user_orphan = Uuid::new_v4();
	object(&d, &tenant, Uuid::new_v4(), user_orphan).await;
	sqlx::query(
		"UPDATE core_areas SET owner=$2 WHERE id=(SELECT area_id FROM core_objects WHERE id=$1)",
	)
	.bind(user_orphan)
	.bind(&user)
	.execute(d.federation.store.pool.fixture_driver())
	.await
	.unwrap();
	let owned_provisional = provisional(&d, &tenant, "purge-target").await;
	let shared_provisional = provisional(&d, &tenant, &user).await;
	// Task dependencies are maintained by a statement trigger and must disappear
	// through the owned Tasks' cascade, never direct worker DML.
	let mut first = None;
	for title in ["first", "dependent"] {
		let input = match first {
			None => json!({"title":title,"description":"owned"}),
			Some(id) => json!({"title":title,"description":"owned","dependencies":[id]}),
		};
		let (status, task) = d
			.as_person(
				&alice,
				"POST",
				&format!("/api/workspaces/{}/tasks", work[0].0),
				Some(&context),
				input,
			)
			.await;
		assert_eq!(status, 200, "{task}");
		first = Some(task["id"].clone());
	}
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM task_dependencies WHERE tenant='{tenant}'"
		))
		.await,
		1
	);
	let parent = Uuid::parse_str(first.as_ref().unwrap().as_str().unwrap()).unwrap();
	let (status, child) = d
		.as_person(
			&alice,
			"POST",
			&format!("/api/workspaces/{}/tasks", work[0].0),
			Some(&context),
			json!({"title":"child","description":"child A of B","parent_id":parent}),
		)
		.await;
	assert_eq!(status, 200, "{child}");
	let child = Uuid::parse_str(child["id"].as_str().unwrap()).unwrap();
	// Valid under the real parent/dependency writer guards: B owns child A,
	// while B depends on A. The purge must use parent maintenance, not safe-stall.
	sqlx::query("UPDATE tasks SET dependencies=ARRAY[$2::uuid] WHERE id=$1")
		.bind(parent)
		.bind(child)
		.execute(d.federation.store.pool.fixture_driver())
		.await
		.unwrap();

	assert_eq!(
		node_purge::retained_storage_bytes(&d.federation.store, &tenant, &target)
			.await
			.unwrap(),
		12
	);
	assert_eq!(
		node_purge::retained_storage_bytes(&d.federation.store, &tenant, &sibling)
			.await
			.unwrap(),
		4
	);
	let management_path = format!("/api/tenants/{tenant}/nodes?include_deleted=true&limit=2");
	let (status, management) = d
		.as_deployment_administrator("GET", &management_path, Value::Null)
		.await;
	assert_eq!(status, 200, "{management}");
	assert_eq!(
		management
			.as_array()
			.unwrap()
			.iter()
			.find(|n| n["name"] == target)
			.unwrap()["retained_storage_bytes"],
		12
	);
	let (status, ordinary) = d
		.as_person(
			&alice,
			"GET",
			&format!("/api/tenants/{tenant}/nodes"),
			Some(&context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{ordinary}");
	assert!(
		ordinary
			.as_array()
			.unwrap()
			.iter()
			.all(|n| n["retained_storage_bytes"].is_null())
	);
	let (sibling_journal, source_entry, source_unit) = sibling_reads(
		&d,
		&tenant,
		work[0].0,
		work[1].0,
		&sibling,
		sibling_credential,
	)
	.await;
	// Separate Entry-only grant: Unit invalidation must not mask leaked text.
	let entry_grant = Uuid::new_v4();
	let entry_journal = Uuid::new_v4();
	sqlx::query("INSERT INTO authorization_remote_grants(id,tenant,task_id,task_revision,workspace_id,node,node_id,credential_id,root_subject,subject_chain,inspection,expires_at,revoked,semantic) SELECT $1,tenant,task_id,task_revision,workspace_id,node,node_id,credential_id,root_subject,subject_chain,inspection,expires_at,revoked,semantic FROM authorization_remote_grants WHERE id=(SELECT grant_id FROM semantic_remote_operations WHERE id=$2)").bind(entry_grant).bind(sibling_journal).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	sqlx::query("INSERT INTO semantic_remote_operations(id,tenant,home_node,grant_id,admission_id,digest,binding,state,receipt) SELECT $1,tenant,home_node,$2,$3,'entry-digest','{}','READY','{\"result\":{\"matches\":[{\"text\":\"target source\"}]}}' FROM semantic_remote_operations WHERE id=$4").bind(entry_journal).bind(entry_grant).bind(sibling_run).bind(sibling_journal).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	sqlx::query("INSERT INTO semantic_remote_reads(grant_id,tenant,admission_id,entry_id,revision,content_digest) VALUES ($1,$2,$3,$4,1,'fixture')").bind(entry_grant).bind(&tenant).bind(sibling_run).bind(source_entry).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	sqlx::query("INSERT INTO semantic_remote_receipts(operation_id,tenant,run_id,digest,receipt) VALUES ($1,$2,$3,'entry-digest','{\"result\":{\"matches\":[{\"text\":\"target source\"}]}}')").bind(entry_journal).bind(&tenant).bind(sibling_run).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	// Same-Tenant inconsistent incidental metadata is representable in existing
	// data. Ordinary append(workspace) derives the Workspace's own Node; this
	// robustness control must not be mistaken for that normal writer path.
	let sibling_event = Uuid::new_v4();
	sqlx::query("INSERT INTO events(id,tenant,node,workspace_id,kind,data) VALUES ($1,$2,$3,$4,'fixture.workspace',$5)").bind(sibling_event).bind(&tenant).bind(&target).bind(work[1].0).bind(json!({"content":"sibling Event body"})).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	for node in [&target, &sibling] {
		sqlx::query("INSERT INTO authorization_catalog (tenant,node,entry_id,entry_version,enabled,revision) SELECT tenant,$2,id,version,true,1 FROM registry WHERE tenant=$1 ORDER BY id,version LIMIT 1").bind(&tenant).bind(node).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
		sqlx::query("INSERT INTO authorization_catalog_history (tenant,node,entry_id,entry_version,enabled,revision,actor) SELECT tenant,node,entry_id,entry_version,enabled,revision,'fixture' FROM authorization_catalog WHERE tenant=$1 AND node=$2").bind(&tenant).bind(node).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	}
	let prior_cycle = Uuid::new_v4();
	sqlx::query("INSERT INTO node_deletions (id,tenant,node,deleted_at,deleted_by_kind,purge_after,restored_at,restored_by_kind,purge_failures) VALUES ($1,$2,$3,clock_timestamp()-interval '40 days','deployment_administrator',clock_timestamp()-interval '10 days',clock_timestamp()-interval '20 days','deployment_administrator',0)").bind(prior_cycle).bind(&tenant).bind(&target).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	let shared_before = d
		.count(&format!(
			"SELECT count(*) FROM registry WHERE tenant='{tenant}'"
		))
		.await;
	assert!(shared_before > 0);
	// The initial default Node has only Tenant-creation history. Produce an
	// actual separable sibling management event before testing its retention.
	let (status, relabeled) = d
		.as_deployment_administrator(
			"PATCH",
			&format!("/api/tenants/{tenant}/nodes/{sibling}"),
			json!({"label":"Retained sibling"}),
		)
		.await;
	assert_eq!(status, 200, "{relabeled}");
	let sibling_audit: Uuid = sqlx::query_scalar("SELECT id FROM dashboard_administration_history WHERE tenant=$1 AND node=$2 AND action='node.update'")
		.bind(&tenant).bind(&sibling).fetch_one(d.federation.store.pool.fixture_driver()).await.unwrap();
	let revisions_before = d
		.count(&format!(
			"SELECT count(*) FROM authorization_revisions WHERE tenant='{tenant}'"
		))
		.await;
	let deletion = request_purge(&d, &tenant, &target).await;
	let shared_audit = Uuid::new_v4();
	sqlx::query("INSERT INTO dashboard_administration_history (id,occurred_at,action,tenant,subject,node,actor_kind) VALUES ($1,clock_timestamp(),'fixture.shared',$2,$3,NULL,'deployment_administrator')").bind(shared_audit).bind(&tenant).bind(&target).execute(d.federation.store.pool.fixture_driver()).await.unwrap();

	// A real unlink error must preserve row anchors and record a resumable step.
	let path = storage.path().join(work[0].1.simple().to_string());
	tokio::fs::remove_file(&path).await.unwrap();
	tokio::fs::create_dir(&path).await.unwrap();
	for claimed in node_purge::claim(&d.federation.store).await.unwrap() {
		assert!(
			node_purge::purge_node(&d.federation.store, claimed)
				.await
				.is_err()
		);
	}

	assert_eq!(d.count(&format!("SELECT count(*) FROM node_deletions WHERE id='{deletion}' AND purge_step='capability_objects' AND purge_failures=1 AND purge_lease IS NULL AND purge_after>clock_timestamp()+interval '29 days'")).await,1);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM workspaces WHERE id='{}'",
			work[0].0
		))
		.await,
		1
	);
	tokio::fs::remove_dir(&path).await.unwrap();
	tokio::fs::write(&path, b"data").await.unwrap();

	for claimed in node_purge::claim(&d.federation.store).await.unwrap() {
		node_purge::purge_node(&d.federation.store, claimed)
			.await
			.unwrap();
	}

	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM workspaces WHERE id='{}'",
			work[0].0
		))
		.await,
		0
	);
	assert!(!storage.path().join(work[0].1.simple().to_string()).exists());
	for (workspace, file) in &work[1..] {
		assert_eq!(
			d.count(&format!(
				"SELECT count(*) FROM workspaces WHERE id='{workspace}'"
			))
			.await,
			1
		);
		assert_eq!(
			tokio::fs::read(storage.path().join(file.simple().to_string()))
				.await
				.unwrap(),
			b"data"
		);
	}
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM registry WHERE tenant='{tenant}'"
		))
		.await,
		shared_before
	);
	assert!(
		d.count(&format!(
			"SELECT count(*) FROM authorization_revisions WHERE tenant='{tenant}'"
		))
		.await >= revisions_before
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM node_names WHERE name='{target}'"
		))
		.await,
		1
	);
	assert_eq!(
		d.count(&format!("SELECT count(*) FROM nodes WHERE name='{target}'"))
			.await,
		1
	);
	assert_eq!(d.count(&format!("SELECT count(*) FROM node_deletions WHERE id='{deletion}' AND purged_at IS NOT NULL AND purge_step IS NULL")).await,1);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM runs WHERE id='{target_run}'"
		))
		.await,
		0
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM runs WHERE id='{sibling_run}'"
		))
		.await,
		1
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM core_runs WHERE run_id='{target_run}'"
		))
		.await,
		0
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM core_runs WHERE run_id='{sibling_run}'"
		))
		.await,
		1
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM core_operations WHERE id='{sibling_operation}' AND state='uncertain'"
		))
		.await,
		1
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM core_operations WHERE id='{target_operation}'"
		))
		.await,
		0
	);
	assert_eq!(d.count(&format!("SELECT count(*) FROM egress_requests WHERE operation_id='{sibling_operation}' AND run_id='{target_run}'")).await,1);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM egress_requests WHERE operation_id='{target_operation}'"
		))
		.await,
		0
	);
	assert_eq!(d.count(&format!("SELECT count(*) FROM authorization_execution e JOIN authorization_run_reads r USING(run_id) WHERE e.run_id='{sibling_run}' AND e.tenant='{tenant}'")).await,1);
	assert_eq!(d.count(&format!("SELECT count(*) FROM authorization_task_origins WHERE task_id='{descendant}' AND source_run_id='{sibling_run}'")).await,1);
	assert_eq!(d.count(&format!("SELECT count(*) FROM remote_run_message_fences WHERE task_id='{descendant}' AND run_id='{target_run}' AND content='sibling message'")).await,1);

	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM task_dependencies WHERE tenant='{tenant}'"
		))
		.await,
		0
	);
	assert_eq!(
		node_purge::retained_storage_bytes(&d.federation.store, &tenant, &target)
			.await
			.unwrap(),
		0
	);
	assert_eq!(
		node_purge::retained_storage_bytes(&d.federation.store, &tenant, &sibling)
			.await
			.unwrap(),
		4
	);
	let used: i64 = sqlx::query_scalar("SELECT used_bytes FROM core_quotas WHERE tenant=$1")
		.bind(&tenant)
		.fetch_one(d.federation.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(used, 12);
	for file in [owned_orphan, owned_provisional] {
		assert!(!storage.path().join(file.simple().to_string()).exists());
	}
	for file in [user_orphan, shared_provisional] {
		assert!(storage.path().join(file.simple().to_string()).exists());
	}
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM authorization_credentials WHERE id='{target_credential}'"
		))
		.await,
		0
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM authorization_credentials WHERE id='{sibling_credential}'"
		))
		.await,
		1
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM provider_credentials WHERE id='{provider}'"
		))
		.await,
		1
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM tenant_secrets WHERE id='{secret}'"
		))
		.await,
		1
	);
	let (status, policy) = d
		.as_deployment_administrator("GET", &format!("/api/authorization/{tenant}"), Value::Null)
		.await;
	assert_eq!(status, 200, "{policy}");
	assert!(policy["bundle"]["subjects"]["purge-target"].is_null());
	assert!(!policy["bundle"]["subjects"]["purge-sibling"].is_null());
	assert!(!policy["bundle"]["subjects"][&user].is_null());
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM semantic_entries WHERE id='{source_entry}'"
		))
		.await,
		0
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM memory_units WHERE id='{source_unit}'"
		))
		.await,
		0
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM semantic_remote_operations WHERE id='{sibling_journal}'"
		))
		.await,
		1
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM semantic_remote_reads WHERE entry_id='{source_entry}' AND grant_id<>(SELECT grant_id FROM semantic_remote_operations WHERE id='{entry_journal}')"
		))
		.await,
		1
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM memory_remote_reads WHERE unit_id='{source_unit}'"
		))
		.await,
		1
	);
	assert_eq!(d.count(&format!("SELECT count(*) FROM semantic_remote_operations WHERE id='{sibling_journal}' AND receipt IS NULL AND state='INVALIDATED' AND fence>0")).await, 1);
	assert_eq!(d.count(&format!("SELECT count(*) FROM semantic_remote_operations WHERE id='{entry_journal}' AND receipt IS NULL AND state='INVALIDATED' AND fence>0")).await,1);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM semantic_remote_receipts WHERE operation_id='{entry_journal}'"
		))
		.await,
		0
	);
	assert_eq!(d.count(&format!("SELECT count(*) FROM semantic_remote_reads WHERE grant_id='{entry_grant}' AND admission_id='{sibling_run}' AND entry_id='{source_entry}'")).await,1);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM events WHERE id='{sibling_event}'"
		))
		.await,
		1,
		"Workspace ownership wins over incidental Node evidence"
	);

	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM authorization_catalog WHERE tenant='{tenant}' AND node='{target}'"
		))
		.await,
		0
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM authorization_catalog WHERE tenant='{tenant}' AND node='{sibling}'"
		))
		.await,
		1
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM authorization_catalog_history WHERE tenant='{tenant}' AND node='{target}'"
		))
		.await,
		1
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM node_deletions WHERE tenant='{tenant}' AND node='{target}'"
		))
		.await,
		2
	);
	// Retrying a completed sweep neither resurrects nor reclaims sibling files.
	node_purge::sweep(&d.federation.store, 365).await.unwrap();
	assert!(storage.path().join(work[1].1.simple().to_string()).exists());
	let (status, management) = d
		.as_deployment_administrator("GET", &management_path, Value::Null)
		.await;
	assert_eq!(status, 200, "{management}");
	let record = management
		.as_array()
		.unwrap()
		.iter()
		.find(|n| n["name"] == target)
		.unwrap();
	assert_eq!(record["state"], "purged");
	assert_eq!(record["retained_storage_bytes"], 0);
	let live = d.nodes_of(&alice, &context, &tenant).await;
	assert_eq!(live.len(), 1, "retained catalog does not count as live");
	assert_eq!(live[0]["name"], sibling);
	for suffix in ["default", "restore"] {
		let (status, body) = d
			.as_deployment_administrator(
				"POST",
				&format!("/api/tenants/{tenant}/nodes/{target}/{suffix}"),
				Value::Null,
			)
			.await;
		assert_eq!(status, 409, "purged catalog never admits {suffix}: {body}");
	}
	let (status, body) = d.workspace_on(&alice, &context, Some(&target)).await;
	assert_eq!(status, 404, "purged Node refuses fresh Workspace: {body}");
	let (status, replacement) = d
		.create_node(&alice, &context, &tenant, "Replacement")
		.await;
	assert_eq!(
		status, 201,
		"two-Node limit counts only live Nodes despite retained catalog: {replacement}"
	);
	assert_ne!(replacement["name"], target);
	// At day364 audit remains fully retained; final purge, not earlier restore,
	// determines the one deadline for catalog and ALL deletion cycles.
	sqlx::query(
		"UPDATE node_deletions SET purged_at=clock_timestamp()-interval '364 days' WHERE id=$1",
	)
	.bind(deletion)
	.execute(d.federation.store.pool.fixture_driver())
	.await
	.unwrap();
	node_purge::erase_audit(&d.federation.store, 365)
		.await
		.unwrap();
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM nodes WHERE tenant='{tenant}' AND name='{target}'"
		))
		.await,
		1
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM node_deletions WHERE tenant='{tenant}' AND node='{target}'"
		))
		.await,
		2
	);

	assert!(d.count(&format!("SELECT count(*) FROM dashboard_administration_history WHERE tenant='{tenant}' AND node='{target}'")).await > 0);
	sqlx::query(
		"UPDATE node_deletions SET purged_at=clock_timestamp()-interval '366 days' WHERE id=$1",
	)
	.bind(deletion)
	.execute(d.federation.store.pool.fixture_driver())
	.await
	.unwrap();
	node_purge::erase_audit(&d.federation.store, 365)
		.await
		.unwrap();
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM nodes WHERE tenant='{tenant}' AND name='{target}'"
		))
		.await,
		0
	);
	assert!(
		d.nodes_of(&alice, &context, &tenant)
			.await
			.iter()
			.all(|node| node["name"] != target),
		"erasing deletion audit never reopens the catalog"
	);
	for table in [
		"node_deletions",
		"authorization_catalog_history",
		"dashboard_administration_history",
	] {
		assert_eq!(
			d.count(&format!(
				"SELECT count(*) FROM {table} WHERE tenant='{tenant}' AND node='{target}'"
			))
			.await,
			0
		);
	}
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM dashboard_administration_history WHERE id='{shared_audit}'"
		))
		.await,
		1
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM authorization_catalog_history WHERE tenant='{tenant}' AND node='{sibling}'"
		))
		.await,
		1
	);
	assert_eq!(d.count(&format!("SELECT count(*) FROM dashboard_administration_history WHERE id='{sibling_audit}' AND tenant='{tenant}' AND node='{sibling}'")).await, 1);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM node_names WHERE name='{target}'"
		))
		.await,
		1
	);
	assert_eq!(
		d.count(&format!("SELECT count(*) FROM events WHERE id='{sibling_event}' AND tenant='{tenant}' AND workspace_id='{}' AND node='{target}'", work[1].0)).await,
		1,
		"finite emitter catalog audit preserves the sibling Workspace Event unchanged"
	);
	let body: Value = sqlx::query_scalar("SELECT data FROM events WHERE id=$1")
		.bind(sibling_event)
		.fetch_one(d.federation.store.pool.fixture_driver())
		.await
		.unwrap();
	assert_eq!(body, json!({"content":"sibling Event body"}));
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn node_purge_fences_expired_claimants_and_yields_to_interstep_tenant_deletion(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	use aidash_domain::nodes::deletion::PurgeStep;
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let (tenant, context) = d.node_tenant(&alice, "Team").await;
	let (_, target) = d.create_node(&alice, &context, &tenant, "Delete").await;
	let target = target["name"].as_str().unwrap();
	let deletion = request_purge(&d, &tenant, target).await;
	let old = node_purge::claim(&d.federation.store)
		.await
		.unwrap()
		.pop()
		.unwrap();
	assert!(
		node_purge::claim(&d.federation.store)
			.await
			.unwrap()
			.is_empty()
	);
	sqlx::query(
		"UPDATE node_deletions SET purge_leased_until=clock_timestamp()-interval '1 second' WHERE id=$1",
	)
	.bind(deletion)
	.execute(d.federation.store.pool.fixture_driver())
	.await
	.unwrap();
	let fresh = node_purge::claim(&d.federation.store)
		.await
		.unwrap()
		.pop()
		.unwrap();
	assert_ne!(old.purge_lease, fresh.purge_lease);
	assert!(
		node_purge::run_step(&d.federation.store, &old, PurgeStep::CapabilityObjects)
			.await
			.is_err()
	);
	node_purge::run_step(&d.federation.store, &fresh, PurgeStep::CapabilityObjects)
		.await
		.unwrap();
	let (status, response) = d
		.as_deployment_administrator(
			"POST",
			&format!("/api/tenants/{tenant}/deletion"),
			json!({"confirmation":tenant}),
		)
		.await;
	assert_eq!(status, 200, "{response}");
	assert!(
		node_purge::run_step(&d.federation.store, &fresh, PurgeStep::Rows)
			.await
			.is_err()
	);
	assert_eq!(
		d.count(&format!("SELECT count(*) FROM nodes WHERE name='{target}'"))
			.await,
		1
	);
	assert_eq!(d.count(&format!("SELECT count(*) FROM node_deletions WHERE id='{deletion}' AND purge_step='rows' AND purged_at IS NULL")).await,1);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn node_purge_refuses_an_unclassified_migrated_table_before_claiming_or_erasing(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let (tenant, context) = d.node_tenant(&alice, "Team").await;
	let (_, node) = d.create_node(&alice, &context, &tenant, "Delete").await;
	let node = node["name"].as_str().unwrap();
	let deletion = request_purge(&d, &tenant, node).await;
	// Supported fixture DDL: the guard must observe the actual database,
	// not merely compare two checked-in copies of the same table list.
	let owner = common::owner_pool(d.federation.store.pool.driver()).await;
	let create = reinhardt::query::Query::create_table()
		.table("unclassified_node_owned_records")
		.col(
			reinhardt::query::ColumnDef::new("id")
				.uuid()
				.primary_key(true),
		)
		.to_string(reinhardt::query::PostgresQueryBuilder);
	sqlx::query(&create).execute(&owner).await.unwrap();
	assert!(node_purge::sweep(&d.federation.store, 365).await.is_err());
	assert_eq!(
		d.count(&format!("SELECT count(*) FROM nodes WHERE name='{node}'"))
			.await,
		1
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM node_deletions WHERE id='{deletion}' AND purge_lease IS NULL"
		))
		.await,
		1
	);
	let drop = reinhardt::query::Query::drop_table()
		.table("unclassified_node_owned_records")
		.to_string(reinhardt::query::PostgresQueryBuilder);
	sqlx::query(&drop).execute(&owner).await.unwrap();
	owner.close().await;
	d.finish().await;
}

/// A Home-owned sibling journal recalls target-owned source content. The
/// source UUIDs must never become standalone ownership selectors for reads.
async fn sibling_reads(
	d: &Deployment,
	tenant: &str,
	target_workspace: Uuid,
	sibling_workspace: Uuid,
	sibling: &str,
	credential: Uuid,
) -> (Uuid, Uuid, Uuid) {
	let bank = Uuid::new_v4();
	let unit = Uuid::new_v4();
	sqlx::query(
		"INSERT INTO memory_banks (id,home,tenant,workspace_id,revision) VALUES ($1,$2,$3,$4,1)",
	)
	.bind(bank)
	.bind(d.federation.config.deployment.to_string())
	.bind(tenant)
	.bind(target_workspace)
	.execute(d.federation.store.pool.fixture_driver())
	.await
	.unwrap();
	sqlx::query("INSERT INTO memory_units (id,tenant,bank_id,revision,text,kind,learning,verification,entities,evidence,links,learned_at,updated_at,deleted,stale) VALUES ($1,$2,$3,1,'target knowledge','world','fact','supported','[]','[]','[]',clock_timestamp(),clock_timestamp(),false,false)").bind(unit).bind(tenant).bind(bank).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	let path = format!("/api/workspaces/{target_workspace}/semantic");
	let spec = json!({"embedding":{"provider":"openai","endpoint":"http://localhost:9","model":"fixture","model_version":"1","dimensions":8},"vector":{"provider":"postgres","endpoint":"local","credential_env":null},"enabled":true,"auto_context":false,"max_sources":10,"max_results":5,"max_result_tokens":128,"max_input_bytes":256});
	let (status, index) = d
		.as_deployment_administrator(
			"POST",
			&format!("{path}/index"),
			json!({"expected_revision":0,"spec":spec}),
		)
		.await;
	assert_eq!(status, 200, "{index}");
	let (status,entry)=d.as_deployment_administrator("POST",&format!("{path}/entries"),json!({"key":"source","expected_revision":0,"source":{"kind":"memory","text":"target source"},"metadata":{}})).await;
	assert_eq!(status, 200, "{entry}");
	let entry = Uuid::parse_str(entry["id"].as_str().unwrap()).unwrap();
	let subject: String =
		sqlx::query_scalar("SELECT subject FROM authorization_credentials WHERE id=$1")
			.bind(credential)
			.fetch_one(d.federation.store.pool.fixture_driver())
			.await
			.unwrap();
	let task = Uuid::new_v4();
	sqlx::query("INSERT INTO tasks (id,tenant,workspace_id,title,description,status,revision,created_by) VALUES ($1,$2,$3,'reader','sibling Home','OPEN',1,$4)").bind(task).bind(tenant).bind(sibling_workspace).bind(&subject).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	let grant = Uuid::new_v4();
	let admission = Uuid::new_v4();
	let operation = Uuid::new_v4();
	sqlx::query("INSERT INTO authorization_remote_grants (id,tenant,task_id,task_revision,workspace_id,node,node_id,credential_id,root_subject,subject_chain,inspection,expires_at,revoked,semantic) VALUES ($1,$2,$3,1,$4,$5,'aidash://peer', $6,$7,ARRAY[$7],'{}',clock_timestamp()+interval '1 day',false,'{\"mode\":\"disabled\"}')").bind(grant).bind(tenant).bind(task).bind(sibling_workspace).bind(sibling).bind(credential).bind(&subject).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	let home = d
		.federation
		.config
		.deployment
		.node(aidash_domain::nodes::NodeName::parse(sibling).unwrap())
		.to_string();
	sqlx::query("INSERT INTO semantic_remote_operations (id,tenant,home_node,grant_id,admission_id,digest,binding,state) VALUES ($1,$2,$3,$4,$5,'fixture','{}','READY')").bind(operation).bind(tenant).bind(home).bind(grant).bind(admission).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	sqlx::query("INSERT INTO semantic_remote_reads (grant_id,tenant,admission_id,entry_id,revision,content_digest) VALUES ($1,$2,$3,$4,1,'fixture')").bind(grant).bind(tenant).bind(admission).bind(entry).execute(d.federation.store.pool.fixture_driver()).await.unwrap();

	sqlx::query(
		"INSERT INTO memory_remote_reads (grant_id,tenant,unit_id,revision) VALUES ($1,$2,$3,1)",
	)
	.bind(grant)
	.bind(tenant)
	.bind(unit)
	.execute(d.federation.store.pool.fixture_driver())
	.await
	.unwrap();
	sqlx::query("UPDATE semantic_remote_operations SET receipt=$2 WHERE id=$1")
		.bind(operation)
		.bind(json!({"memory":{"banks":[{"recall":{"units":[{"text":"target knowledge"}]}}]}}))
		.execute(d.federation.store.pool.fixture_driver())
		.await
		.unwrap();
	(operation, entry, unit)
}

#[rstest::rstest]
#[case::requested(true)]
#[case::deadline(false)]
#[tokio::test]
async fn node_purge_starts_only_from_a_committed_request_or_due_deadline(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
	#[case] requested: bool,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let (tenant, context) = d.node_tenant(&alice, "Team").await;
	let (_, node) = d.create_node(&alice, &context, &tenant, "Delete").await;
	let node = node["name"].as_str().unwrap();
	if requested {
		request_purge(&d, &tenant, node).await;
	} else {
		sqlx::query("INSERT INTO node_deletions (id,tenant,node,deleted_at,deleted_by_kind,purge_after,purge_failures) VALUES ($1,$2,$3,clock_timestamp()-interval '31 days','deployment_administrator',clock_timestamp()-interval '1 day',0)").bind(Uuid::new_v4()).bind(&tenant).bind(node).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	}
	let before: chrono::DateTime<chrono::Utc> =
		sqlx::query_scalar("SELECT purge_after FROM node_deletions WHERE tenant=$1 AND node=$2")
			.bind(&tenant)
			.bind(node)
			.fetch_one(d.federation.store.pool.fixture_driver())
			.await
			.unwrap();
	let claimed = node_purge::claim(&d.federation.store).await.unwrap();
	assert_eq!(claimed.len(), 1);
	assert_eq!(claimed[0].purge_after, before);
	assert_eq!(claimed[0].purge_step.as_deref(), Some("capability_objects"));
	node_purge::purge_node(&d.federation.store, claimed[0].clone())
		.await
		.unwrap();
	assert_eq!(
		d.count(&format!("SELECT count(*) FROM nodes WHERE name='{node}'"))
			.await,
		1
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM node_names WHERE name='{node}'"
		))
		.await,
		1
	);
	d.finish().await;
}

#[rstest::rstest]
#[case::corrupt_sibling_credential_reference("credential")]
#[case::surviving_delegation("delegation")]
#[case::contradictory_home_role("role")]
#[case::ack_without_stop("ack")]
#[case::stop_without_ack("stop")]
#[case::uncertain_ack_without_stop("uncertain_ack")]
#[case::uncertain_stop_without_ack("uncertain_stop")]
#[case::uncertain_never_dispatched_shortcut("uncertain_never")]
#[case::string_boolean_stop_proof("string_proof")]
#[case::global_coordinator_recovery("coordinator")]
#[case::global_participant_recovery("participant")]
#[case::visibility_gate_before_unlink("gate")]
#[case::corrupt_terminal_digest("atomic_digest")]
#[case::missing_terminal_owner("atomic_owner")]
#[case::missing_terminal_current_trust("atomic_trust")]
#[tokio::test]
async fn node_purge_stalls_before_unlink_and_preserves_surviving_reference_and_recovery_evidence(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
	#[case] blocker: &str,
) {
	let mut d = Deployment::new(runtime, open_rules()).await;
	let storage = tempfile::tempdir().unwrap();
	d.federation.store.capabilities = Runtime::new(Profile {
		storage: storage.path().to_owned(),
		..Profile::default()
	})
	.unwrap();
	let alice = d.person("alice").await;
	let (tenant, context) = d.node_tenant(&alice, "Team").await;
	let sibling = d.nodes_of(&alice, &context, &tenant).await[0]["name"]
		.as_str()
		.unwrap()
		.to_owned();
	let (_, node) = d.create_node(&alice, &context, &tenant, "Delete").await;
	let target = node["name"].as_str().unwrap().to_owned();
	let (target_credential, sibling_credential, _) = subjects(&d, &tenant, &target, &sibling).await;
	let mut work = Vec::new();
	for node in [&target, &sibling] {
		let (status, w) = d.workspace_on(&alice, &context, Some(node)).await;
		assert_eq!(status, 200, "{w}");
		work.push(Uuid::parse_str(w["id"].as_str().unwrap()).unwrap());
	}
	let file = Uuid::new_v4();
	object(&d, &tenant, work[0], file).await;
	let metadata = Uuid::new_v4();
	let expected = match blocker {
		"role" => {
			let (journal, _, _) =
				sibling_reads(&d, &tenant, work[0], work[1], &sibling, sibling_credential).await;
			let home = d
				.federation
				.config
				.deployment
				.node(aidash_domain::nodes::NodeName::parse(&target).unwrap())
				.to_string();
			sqlx::query("UPDATE semantic_remote_operations SET home_node=$2 WHERE id=$1")
				.bind(journal)
				.bind(home)
				.execute(d.federation.store.pool.fixture_driver())
				.await
				.unwrap();
			"contradictory local federation"
		}
		"credential" => {
			let source = d
				.federation
				.config
				.deployment
				.node(aidash_domain::nodes::NodeName::parse(&target).unwrap())
				.to_string();
			// Legacy/corrupt cross-owner credential use cannot be repaired by deleting
			// a sibling's mapping or rewriting its immutable historical evidence.
			sqlx::query("INSERT INTO node_registrations (tenant,node,source_node,atomic_transactions,enabled,revision,actor) VALUES ($1,$2,$3,false,false,1,'fixture')").bind(&tenant).bind(&sibling).bind(&source).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
			for table in [
				"authorization_peer_mappings",
				"authorization_peer_mapping_history",
			] {
				sqlx::query(&format!("INSERT INTO {table} (tenant,node,source_node,source_subject,credential_id,enabled,revision,actor) VALUES ($1,$2,$3,'source',$4,false,1,'fixture')")).bind(&tenant).bind(&sibling).bind(&source).bind(target_credential).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
			}
			"authorization_peer"
		}
		"delegation" => {
			let path = format!("/api/authorization/{tenant}");
			let (status, snapshot) = d
				.as_deployment_administrator("GET", &path, Value::Null)
				.await;
			assert_eq!(status, 200, "{snapshot}");
			let mut bundle = snapshot["bundle"].clone();
			bundle["subjects"]["purge-sibling"]["delegated_by"] = json!("purge-target");
			let (status, response) = d
				.as_deployment_administrator(
					"POST",
					&path,
					json!({"expected_revision":snapshot["revision"],"bundle":bundle}),
				)
				.await;
			assert_eq!(status, 200, "{response}");
			"surviving policy delegation"
		}
		"ack" | "stop" | "uncertain_ack" | "uncertain_stop" | "uncertain_never"
		| "string_proof" => {
			let result = if blocker == "string_proof" {
				json!({"termination_confirmed":"true","runner_acknowledged":"true","effects_may_have_occurred":true})
			} else {
				json!({"termination_confirmed":blocker=="stop" || blocker=="uncertain_stop","runner_acknowledged":blocker=="ack" || blocker=="uncertain_ack","effects_may_have_occurred":blocker!="uncertain_never"})
			};
			let state = if blocker.starts_with("uncertain") {
				"uncertain"
			} else {
				"withdrawn"
			};
			let runner = if blocker == "uncertain_never" {
				None
			} else {
				Some("fixture-runner")
			};
			sqlx::query("INSERT INTO core_operations (id,area_id,run_id,credential_id,tenant,principal,request_key,digest,kind,state,epoch,generation,revision,policy_revision,subjects,input,result,runner_instance) SELECT $1,id,$2,$3,tenant,'purge-target','fixture','fixture','shell',$6,1,1,1,1,'[\"purge-target\"]','{}',$4,$7 FROM core_areas WHERE workspace_id=$5").bind(metadata).bind(Uuid::new_v4()).bind(target_credential).bind(result).bind(work[0]).bind(state).bind(runner).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
			"confirmed command termination"
		}
		"atomic_digest" | "atomic_owner" | "atomic_trust" => {
			let local_name = if blocker == "atomic_owner" {
				aidash_domain::nodes::NodeName::derived("absent-permanent-owner")
			} else {
				aidash_domain::nodes::NodeName::parse(&target).unwrap()
			};
			let local = d.federation.config.deployment.node(local_name).to_string();
			let remote = aidash_domain::nodes::DeploymentId::parse("aidash://atomic-peer")
				.unwrap()
				.node(aidash_domain::nodes::NodeName::derived("peer"))
				.to_string();
			let mut participants = vec![
				json!({"node_id":local,"mutations":[{"kind":"workspace_state","workspace_id":work[0],"expected_revision":0,"state":{}}]}),
				json!({"node_id":remote,"mutations":[]}),
			];
			participants.sort_by(|a, b| a["node_id"].as_str().cmp(&b["node_id"].as_str()));
			let manifest: aidash_domain::transactions::Manifest = serde_json::from_value(json!({
				"id":metadata,"coordinator":remote,"isolation":"serializable",
				"deadline":chrono::Utc::now()+chrono::Duration::minutes(5),"participants":participants
			}))
			.unwrap();
			manifest.validate::<aidash_server::Error>().unwrap();
			let digest = if blocker == "atomic_digest" {
				"corrupt".to_owned()
			} else {
				manifest.digest().unwrap()
			};
			sqlx::query("INSERT INTO atomic_participants(id,coordinator,digest,manifest,phase) VALUES ($1,$2,$3,$4,'COMMITTED')")
				.bind(metadata).bind(&manifest.coordinator).bind(digest).bind(json!(manifest))
				.execute(d.federation.store.pool.fixture_driver()).await.unwrap();
			"atomic recovery registration dependency"
		}
		"coordinator" => {
			sqlx::query("INSERT INTO atomic_coordinators (id,digest,manifest,complete) VALUES ($1,'fixture','{}',false)").bind(metadata).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
			"incomplete atomic recovery"
		}
		"participant" | "gate" => {
			sqlx::query("INSERT INTO atomic_participants (id,coordinator,digest,manifest,phase) VALUES ($1,'aidash://unrelated','fixture','{}',$2)").bind(metadata).bind(if blocker=="gate"{"COMMITTED"}else{"PREPARED"}).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
			if blocker == "gate" {
				sqlx::query("UPDATE atomic_gate SET transaction_id=$1 WHERE singleton")
					.bind(metadata)
					.execute(d.federation.store.pool.fixture_driver())
					.await
					.unwrap();
				"transaction pending"
			} else {
				"nonterminal atomic participant"
			}
		}
		_ => unreachable!(),
	};
	let deletion = request_purge(&d, &tenant, &target).await;
	let claimed = node_purge::claim(&d.federation.store)
		.await
		.unwrap()
		.pop()
		.unwrap();
	let error = node_purge::purge_node(&d.federation.store, claimed)
		.await
		.unwrap_err();
	if blocker == "gate" {
		assert!(
			matches!(error, aidash_server::Error::TransactionPending),
			"{error}"
		);
	} else {
		assert!(error.to_string().contains(expected), "{error}");
	}
	assert_eq!(d.count(&format!("SELECT count(*) FROM node_deletions WHERE id='{deletion}' AND purge_step='capability_objects' AND purge_failures=1 AND purge_lease IS NULL")).await,1);
	assert!(storage.path().join(file.simple().to_string()).exists());
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM core_objects WHERE id='{file}'"
		))
		.await,
		1
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM workspaces WHERE id='{}'",
			work[0]
		))
		.await,
		1
	);
	assert_eq!(d.count(&format!("SELECT count(*) FROM authorization_credentials WHERE id='{target_credential}' OR id='{sibling_credential}'")).await,2);
	match blocker {
		"credential" => {
			for table in [
				"authorization_peer_mappings",
				"authorization_peer_mapping_history",
			] {
				assert_eq!(d.count(&format!("SELECT count(*) FROM {table} WHERE tenant='{tenant}' AND node='{sibling}' AND credential_id='{target_credential}'")).await,1);
			}
		}
		"coordinator" => assert_eq!(
			d.count(&format!(
				"SELECT count(*) FROM atomic_coordinators WHERE id='{metadata}' AND NOT complete"
			))
			.await,
			1
		),
		"participant" | "gate" => assert_eq!(
			d.count(&format!(
				"SELECT count(*) FROM atomic_participants WHERE id='{metadata}'"
			))
			.await,
			1
		),
		_ => {}
	}
	if blocker == "gate" {
		sqlx::query("UPDATE atomic_gate SET transaction_id=NULL WHERE singleton")
			.execute(d.federation.store.pool.fixture_driver())
			.await
			.unwrap();
	}
	d.finish().await;
}

/// Reciprocal native reads must not pin both source Nodes forever. Purging the
/// first source invalidates body caches while keeping the surviving reader's
/// UUID/revision provenance; the second purge can then finish independently.
#[rstest::rstest]
#[tokio::test]
async fn node_purge_drains_mutual_reads_and_invalidates_body_caches_while_preserving_sibling_files(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let mut d = Deployment::new(runtime, open_rules()).await;
	let storage = tempfile::tempdir().unwrap();
	d.federation.store.capabilities = Runtime::new(Profile {
		storage: storage.path().to_owned(),
		..Profile::default()
	})
	.unwrap();
	let alice = d.person("alice").await;
	let (tenant, context) = d.node_tenant(&alice, "Team").await;
	let keeper = d.nodes_of(&alice, &context, &tenant).await[0]["name"]
		.as_str()
		.unwrap()
		.to_owned();
	let (_, sibling) = d.create_node(&alice, &context, &tenant, "Reader").await;
	let sibling = sibling["name"].as_str().unwrap().to_owned();
	let (_, target) = d.create_node(&alice, &context, &tenant, "Delete").await;
	let target = target["name"].as_str().unwrap().to_owned();
	let (target_credential, sibling_credential, _) = subjects(&d, &tenant, &target, &sibling).await;
	let mut work = Vec::new();
	for node in [&target, &sibling, &keeper] {
		let (status, w) = d.workspace_on(&alice, &context, Some(node)).await;
		assert_eq!(status, 200, "{w}");
		let workspace = Uuid::parse_str(w["id"].as_str().unwrap()).unwrap();
		let file = Uuid::new_v4();
		object(&d, &tenant, workspace, file).await;
		work.push((workspace, file));
	}
	let (sibling_journal, _, target_unit) = sibling_reads(
		&d,
		&tenant,
		work[0].0,
		work[1].0,
		&sibling,
		sibling_credential,
	)
	.await;
	let (_, _, sibling_unit) = sibling_reads(
		&d,
		&tenant,
		work[1].0,
		work[0].0,
		&target,
		target_credential,
	)
	.await;
	for node in [&target, &sibling] {
		request_purge(&d, &tenant, node).await;
	}
	let mut claimed = node_purge::claim(&d.federation.store).await.unwrap();
	assert_eq!(claimed.len(), 2);
	let first = claimed.remove(claimed.iter().position(|r| r.node == target).unwrap());
	node_purge::purge_node(&d.federation.store, first)
		.await
		.unwrap();
	assert!(!storage.path().join(work[0].1.simple().to_string()).exists());
	assert_eq!(
		tokio::fs::read(storage.path().join(work[1].1.simple().to_string()))
			.await
			.unwrap(),
		b"data"
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM memory_remote_reads WHERE unit_id='{target_unit}'"
		))
		.await,
		1
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM memory_units WHERE id='{target_unit}'"
		))
		.await,
		0
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM memory_units WHERE id='{sibling_unit}'"
		))
		.await,
		1
	);
	assert_eq!(d.count(&format!("SELECT count(*) FROM semantic_remote_operations WHERE id='{sibling_journal}' AND state='INVALIDATED' AND receipt IS NULL")).await, 1);
	node_purge::purge_node(&d.federation.store, claimed.pop().unwrap())
		.await
		.unwrap();
	assert!(!storage.path().join(work[1].1.simple().to_string()).exists());
	for table in [
		"memory_remote_reads",
		"memory_units",
		"semantic_remote_operations",
	] {
		assert_eq!(
			d.count(&format!(
				"SELECT count(*) FROM {table} WHERE tenant='{tenant}'"
			))
			.await,
			0
		);
	}
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM node_names WHERE tenant='{tenant}'"
		))
		.await,
		3
	);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM nodes WHERE tenant='{tenant}' AND name='{keeper}'"
		))
		.await,
		1
	);
	assert_eq!(
		tokio::fs::read(storage.path().join(work[2].1.simple().to_string()))
			.await
			.unwrap(),
		b"data"
	);
	d.finish().await;
}

async fn cross_node_execution_evidence(
	d: &Deployment,
	tenant: &str,
	target: &str,
	sibling: &str,
	target_workspace: Uuid,
	sibling_workspace: Uuid,
	target_credential: Uuid,
	sibling_credential: Uuid,
) -> (Uuid, Uuid, Uuid, Uuid, Uuid) {
	let pending = aidash_domain::run_state::encode(
		&aidash_domain::run_state::RunState::Completed(aidash_domain::run_state::TerminalState {}),
		&aidash_domain::run_state::RecoveryState::default(),
	)
	.unwrap();
	let mut result = Vec::new();
	// Executor and Workspace Home are deliberately different in both directions.
	for (executor, workspace, credential, subject) in [
		(
			target,
			sibling_workspace,
			sibling_credential,
			"purge-sibling",
		),
		(sibling, target_workspace, target_credential, "purge-target"),
	] {
		let task = Uuid::new_v4();
		let run = Uuid::new_v4();
		let operation = Uuid::new_v4();
		let home = d
			.federation
			.config
			.deployment
			.node(
				aidash_domain::nodes::NodeName::parse(if workspace == target_workspace {
					target
				} else {
					sibling
				})
				.unwrap(),
			)
			.to_string();
		sqlx::query("UPDATE core_areas SET home_node=$2 WHERE tenant=$1 AND workspace_id=$3")
			.bind(tenant)
			.bind(&home)
			.bind(workspace)
			.execute(d.federation.store.pool.fixture_driver())
			.await
			.unwrap();

		sqlx::query("INSERT INTO tasks (id,tenant,workspace_id,title,description,status,revision,created_by) VALUES ($1,$2,$3,'execution','execution','COMPLETED',1,$4)").bind(task).bind(tenant).bind(workspace).bind(subject).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
		sqlx::query("INSERT INTO runs (id,tenant,task_id,workspace_id,node,home_node,agent_id,agent_version,phase,pending,revision) VALUES ($1,$2,$3,$4,$5,$6,'fixture','1.0.0','COMPLETED',$7,1)").bind(run).bind(tenant).bind(task).bind(workspace).bind(executor).bind(&home).bind(&pending).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
		let (execution_credential, execution_subject) = if executor == target {
			(target_credential, "purge-target")
		} else {
			(sibling_credential, "purge-sibling")
		};
		sqlx::query("INSERT INTO authorization_execution(run_id,tenant,task_id,workspace_id,credential_id,root_subject,subject_chain) VALUES ($1,$2,$3,$4,$5,$6,ARRAY[$6,'fixture-agent'])").bind(run).bind(tenant).bind(task).bind(workspace).bind(execution_credential).bind(execution_subject).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
		sqlx::query("INSERT INTO authorization_run_reads(run_id,tenant,workspace_id,resource_kind,resource_id) VALUES ($1,$2,$3,'task',$4)").bind(run).bind(tenant).bind(workspace).bind(task).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
		sqlx::query("INSERT INTO core_runs (run_id,tenant,area_id,sequence,initialized,generation) SELECT $1,$2,id,1,true,generation FROM core_areas WHERE tenant=$2 AND workspace_id=$3").bind(run).bind(tenant).bind(workspace).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
		sqlx::query("INSERT INTO core_operations (id,tenant,area_id,run_id,credential_id,principal,request_key,digest,kind,state,epoch,generation,revision,policy_revision,subjects,input,result,runner_instance) SELECT $1,$2,id,$3,$4,$5,$1::text,'fixture','shell','uncertain',epoch,generation,1,1,'[]','{}','{\"termination_confirmed\":true,\"runner_acknowledged\":true}','fixture-runner' FROM core_areas WHERE tenant=$2 AND workspace_id=$6").bind(operation).bind(tenant).bind(run).bind(credential).bind(subject).bind(workspace).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
		sqlx::query("INSERT INTO egress_requests (id,tenant,run_id,operation_id,service,method,path,query_digest,status,sent_bytes,received_bytes,duration_ms,created_at) VALUES ($1,$2,$3,$4,'fixture','GET','/','digest',200,1,1,1,clock_timestamp())").bind(Uuid::new_v4()).bind(tenant).bind(run).bind(operation).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
		result.push((task, run, operation));
	}
	let descendant = Uuid::new_v4();
	sqlx::query("INSERT INTO tasks (id,tenant,workspace_id,title,description,status,revision,created_by) VALUES ($1,$2,$3,'descendant','descendant','COMPLETED',1,'purge-sibling')").bind(descendant).bind(tenant).bind(sibling_workspace).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	sqlx::query("INSERT INTO authorization_task_origins (task_id,tenant,source_run_id,root_subject,subject_chain) VALUES ($1,$2,$3,'purge-sibling',ARRAY['purge-sibling','fixture-agent'])").bind(descendant).bind(tenant).bind(result[1].1).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	sqlx::query("INSERT INTO remote_run_message_fences (task_id,tenant,idempotency_key,run_id,content,consumed) VALUES ($1,$2,'retained',$3,'sibling message',true)").bind(descendant).bind(tenant).bind(result[0].1).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	(
		result[0].1,
		result[1].1,
		result[0].2,
		result[1].2,
		descendant,
	)
}
