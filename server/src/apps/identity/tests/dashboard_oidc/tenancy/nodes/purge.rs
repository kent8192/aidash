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
	let mut d = Deployment::new(runtime, open_rules()).await;
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
	let (sibling_journal, source_entry, source_unit) = sibling_reads(
		&d,
		&tenant,
		work[0].0,
		work[1].0,
		&sibling,
		sibling_credential,
	)
	.await;
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
	let revisions_before = d
		.count(&format!(
			"SELECT count(*) FROM authorization_revisions WHERE tenant='{tenant}'"
		))
		.await;
	let deletion = request_purge(&d, &tenant, &target).await;

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
		0
	);
	assert_eq!(d.count(&format!("SELECT count(*) FROM node_deletions WHERE id='{deletion}' AND purged_at IS NOT NULL AND purge_step IS NULL")).await,1);
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
			"SELECT count(*) FROM semantic_remote_reads WHERE entry_id='{source_entry}'"
		))
		.await,
		1
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
	// Deliberate fixture DDL: the production guard must observe the migrated
	// database, not merely compare two checked-in copies of the same list.
	let owner = common::owner_pool(d.federation.store.pool.driver()).await;
	sqlx::query("CREATE TABLE unclassified_node_owned_records (id uuid PRIMARY KEY)")
		.execute(&owner)
		.await
		.unwrap();
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
	sqlx::query("DROP TABLE unclassified_node_owned_records")
		.execute(&owner)
		.await
		.unwrap();
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
	let task = Uuid::new_v4();
	sqlx::query("INSERT INTO tasks (id,tenant,workspace_id,title,description,status,revision,created_by) VALUES ($1,$2,$3,'reader','sibling Home','OPEN',1,'purge-sibling')").bind(task).bind(tenant).bind(sibling_workspace).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	let grant = Uuid::new_v4();
	let admission = Uuid::new_v4();
	let operation = Uuid::new_v4();
	sqlx::query("INSERT INTO authorization_remote_grants (id,tenant,task_id,task_revision,workspace_id,node,node_id,credential_id,root_subject,subject_chain,inspection,expires_at,revoked,semantic) VALUES ($1,$2,$3,1,$4,$5,'aidash://peer', $6,'purge-sibling',ARRAY['purge-sibling'],'{}',clock_timestamp()+interval '1 day',false,'{\"mode\":\"disabled\"}')").bind(grant).bind(tenant).bind(task).bind(sibling_workspace).bind(sibling).bind(credential).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	let home = d
		.federation
		.config
		.deployment
		.node(aidash_domain::nodes::NodeName::parse(sibling).unwrap())
		.to_string();
	sqlx::query("INSERT INTO semantic_remote_operations (id,tenant,home_node,grant_id,admission_id,digest,binding,state) VALUES ($1,$2,$3,$4,$5,'fixture','{}','READY')").bind(operation).bind(tenant).bind(home).bind(grant).bind(admission).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
	sqlx::query("INSERT INTO semantic_remote_reads (grant_id,tenant,admission_id,entry_id,revision,content_digest) VALUES ($1,$2,$3,$4,1,'fixture')").bind(grant).bind(tenant).bind(admission).bind(entry).execute(d.federation.store.pool.fixture_driver()).await.unwrap();

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
		0
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
#[case::legitimate_surviving_unit_read("unit_read")]
#[case::corrupt_sibling_credential_reference("credential")]
#[case::surviving_delegation("delegation")]
#[case::contradictory_home_role("role")]
#[case::ack_without_stop("ack")]
#[case::stop_without_ack("stop")]
#[case::global_coordinator_recovery("coordinator")]
#[case::global_participant_recovery("participant")]
#[case::visibility_gate_before_unlink("gate")]
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
		"unit_read" | "role" => {
			let (journal, _, unit) =
				sibling_reads(&d, &tenant, work[0], work[1], &sibling, sibling_credential).await;
			if blocker == "unit_read" {
				sqlx::query("INSERT INTO memory_remote_reads (grant_id,tenant,unit_id,revision) SELECT grant_id,tenant,$2,1 FROM semantic_remote_operations WHERE id=$1").bind(journal).bind(unit).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
				"surviving memory read"
			} else {
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
		"ack" | "stop" => {
			let result = json!({"termination_confirmed":blocker=="stop","runner_acknowledged":blocker=="ack","effects_may_have_occurred":true});
			sqlx::query("INSERT INTO core_operations (id,area_id,run_id,credential_id,tenant,principal,request_key,digest,kind,state,epoch,generation,revision,policy_revision,subjects,input,result,runner_instance) SELECT $1,id,$2,$3,tenant,'purge-target','fixture','fixture','shell','withdrawn',1,1,1,1,'[\"purge-target\"]','{}',$4,'fixture-runner' FROM core_areas WHERE workspace_id=$5").bind(metadata).bind(Uuid::new_v4()).bind(target_credential).bind(result).bind(work[0]).execute(d.federation.store.pool.fixture_driver()).await.unwrap();
			"confirmed command termination"
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
		"unit_read" => assert_eq!(
			d.count(&format!(
				"SELECT count(*) FROM memory_remote_reads WHERE tenant='{tenant}'"
			))
			.await,
			1
		),
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
