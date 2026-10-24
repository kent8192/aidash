//! Tenant Deletion, restore, leaving and Personal Tenant re-creation (#239,
//! ADR 0060), over HTTP and PostgreSQL.
use super::*;

impl Deployment {
	/// A Tenant `person` creates, and its Tenant Creator's context selector.
	async fn tenant_of(&self, person: &Person, label: &str) -> (String, String) {
		let (status, created) = self
			.as_person(person, "POST", "/api/tenants", None, json!({"label":label}))
			.await;
		assert_eq!(status, 201, "{created}");
		let tenant = created["name"].as_str().unwrap().to_owned();
		let context = self.context(person, &tenant).await;
		(tenant, context)
	}

	async fn delete(&self, person: &Person, context: &str, tenant: &str) -> (u16, Value) {
		self.as_person(
			person,
			"POST",
			&format!("/api/tenants/{tenant}/deletion"),
			Some(context),
			json!({"confirmation":tenant}),
		)
		.await
	}

	async fn restore(&self, person: &Person, context: &str, tenant: &str) -> (u16, Value) {
		self.as_person(
			person,
			"POST",
			&format!("/api/tenants/{tenant}/restore"),
			Some(context),
			Value::Null,
		)
		.await
	}

	async fn session(&self, person: &Person) -> Value {
		let (status, session) = self
			.as_person(person, "GET", "/auth/session", None, Value::Null)
			.await;
		assert_eq!(status, 200, "{session}");
		session
	}

	async fn deletions(&self, tenant: &str, condition: &str) -> i64 {
		sqlx::query_scalar(&format!(
			"SELECT count(*) FROM tenant_deletions WHERE tenant = $1 AND {condition}"
		))
		.bind(tenant)
		.fetch_one(self.federation.store.pool.fixture_driver())
		.await
		.unwrap()
	}
}

fn deleted<'a>(session: &'a Value, tenant: &str) -> Option<&'a Value> {
	session["deleted_tenants"]
		.as_array()
		.unwrap()
		.iter()
		.find(|row| row["tenant"] == tenant)
}

#[rstest::rstest]
#[tokio::test]
async fn deletion_removes_administrator_data_access_but_keeps_restore_available(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let carol = d.person("carol").await;
	sqlx::query("INSERT INTO dashboard_deployment_administrator_grants (identity_id) VALUES ($1)")
		.bind(carol.identity)
		.execute(d.federation.store.pool.driver())
		.await
		.unwrap();
	let (tenant, context) = d.tenant_of(&alice, "Deleted data").await;
	let scope = aidash_application::tenancy::TenancyScope::tenant(&tenant).unwrap();
	let workspace = d
		.federation
		.store
		.create_workspace(&scope, "alice", "Private workspace", "Keep data private")
		.await
		.unwrap();
	let task = d
		.federation
		.store
		.create_task(
			&scope,
			workspace.id,
			&aidash_domain::NewTask {
				title: "Private task".into(),
				description: "Deleted Tenant data".into(),
				requirements: json!({}),
				dependencies: vec![],
				parent_id: None,
			},
			"alice",
			None,
		)
		.await
		.unwrap();
	let path = format!("/api/workspaces/{}", workspace.id);
	let tenant_event = d
		.federation
		.store
		.emit(
			&scope,
			None,
			"tenant.fixture",
			json!({"content":"Tenant-only event"}),
		)
		.await
		.unwrap();
	let (status, body) = d
		.as_deployment_administrator("GET", &path, Value::Null)
		.await;
	assert_eq!(status, 200, "{body}");
	// A protected read waits for the same policy lock that deletion owns.
	{
		let mut policy_lock = d.federation.store.pool.begin(&scope).await.unwrap();
		let lock = reinhardt::query::Query::select()
			.column(reinhardt::query::Alias::new("tenant"))
			.from(reinhardt::query::Alias::new("authorization_bundles"))
			.and_where(
				reinhardt::query::Expr::col("tenant").eq(reinhardt::query::Expr::value(&tenant)),
			)
			.lock(reinhardt::query::LockType::Update)
			.to_string(reinhardt::query::PostgresQueryBuilder);
		let _: String = aidash_server::database::native::query_scalar(&lock)
			.scalar_one(&mut *policy_lock)
			.await
			.unwrap();
		let read = d.as_deployment_administrator("GET", &path, Value::Null);
		tokio::pin!(read);
		assert!(
			tokio::time::timeout(std::time::Duration::from_millis(250), &mut read)
				.await
				.is_err(),
			"the read must retain a policy lease"
		);
		policy_lock.rollback().await.unwrap();
		let (status, body) = read.await;
		assert_eq!(status, 200, "{body}");
	}
	d.federation
		.store
		.keyless
		.permit_origin_of("http://localhost:9")
		.unwrap();
	let index = json!({"expected_revision":0,"spec":{"embedding":{"provider":"openai","endpoint":"http://localhost:9","model":"fixture","model_version":"1","dimensions":8},"vector":{"provider":"postgres","endpoint":"local","credential_env":null},"enabled":true,"auto_context":false,"max_sources":10,"max_results":5,"max_result_tokens":128,"max_input_bytes":256}});
	let (status, body) = d.delete(&alice, &context, &tenant).await;
	assert_eq!(status, 200, "{body}");
	let request = Uuid::new_v4();
	for (method, endpoint, input) in [
		("POST", format!("{path}/semantic/index"), index),
		(
			"GET",
			format!("/api/authorization/{tenant}/revisions"),
			Value::Null,
		),
		(
			"GET",
			format!("/api/authorization/{tenant}/decisions"),
			Value::Null,
		),
		(
			"GET",
			format!("/api/generation/{tenant}/policies"),
			Value::Null,
		),
		(
			"GET",
			format!("/api/generation/{tenant}/requests"),
			Value::Null,
		),
		(
			"GET",
			format!("/api/generation/{tenant}/requests/{request}/spec"),
			Value::Null,
		),
		(
			"GET",
			format!("/api/generation/{tenant}/requests/{request}/history"),
			Value::Null,
		),
		(
			"GET",
			format!("/api/generation/{tenant}/requests/{request}/usage"),
			Value::Null,
		),
		(
			"POST",
			format!("/api/generation/{tenant}/requests/{request}/control"),
			json!({"action":"stop","reason":"Deleted Tenant"}),
		),
	] {
		let (status, body) = d
			.as_deployment_administrator(method, &endpoint, input.clone())
			.await;
		assert_eq!(status, 410, "{method} {endpoint}: {body}");
		let (status, body) = d
			.as_person(
				&carol,
				method,
				&endpoint,
				Some("deployment_administrator"),
				input,
			)
			.await;
		assert_eq!(status, 410, "browser {method} {endpoint}: {body}");
	}

	for method in ["GET", "PATCH"] {
		let (status, body) = d
			.as_deployment_administrator(
				method,
				&path,
				json!({"revision":workspace.revision,"state":{}}),
			)
			.await;
		assert_eq!(status, 410, "{method}: {body}");
		assert_eq!(body["error"], "tenant_deleted");
		let (status, body) = d
			.as_person(
				&carol,
				method,
				&path,
				Some("deployment_administrator"),
				json!({"revision":workspace.revision,"state":{}}),
			)
			.await;
		assert_eq!(status, 410, "browser {method}: {body}");
		assert_eq!(body["error"], "tenant_deleted");
	}
	let (status, page) = d
		.as_deployment_administrator("GET", "/api/tasks", Value::Null)
		.await;
	assert_eq!(status, 200, "{page}");
	assert!(
		page["tasks"]
			.as_array()
			.unwrap()
			.iter()
			.all(|row| row["id"] != task.id.to_string()),
		"{page}"
	);
	let (status, state) = d
		.as_deployment_administrator("GET", "/api/state", Value::Null)
		.await;
	assert_eq!(status, 200, "{state}");
	assert!(
		state["workspaces"]
			.as_array()
			.unwrap()
			.iter()
			.all(|row| row["id"] != workspace.id.to_string()),
		"{state}"
	);
	assert!(
		state["events"]
			.as_array()
			.unwrap()
			.iter()
			.all(|row| row["workspace_id"] != workspace.id.to_string()),
		"{state}"
	);
	assert!(
		state["events"]
			.as_array()
			.unwrap()
			.iter()
			.all(|row| row["id"] != tenant_event.id.to_string()),
		"{state}"
	);

	let (status, body) = d
		.as_deployment_administrator(
			"POST",
			&format!("/api/tenants/{tenant}/restore"),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = d
		.as_deployment_administrator("GET", &path, Value::Null)
		.await;
	assert_eq!(status, 200, "{body}");
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_deleted_tenant_refuses_everyone_until_its_administrator_restores_it(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let bob = d.person("bob").await;
	let (tenant, context) = d.tenant_of(&alice, "Team").await;
	let bob_context = d.admit(&tenant, &bob, "bob", json!(["members"])).await;
	let (status, issued) = d
		.as_deployment_administrator(
			"POST",
			&format!("/api/authorization/{tenant}/credentials"),
			json!({"subject":"bob"}),
		)
		.await;
	assert_eq!(status, 200, "{issued}");
	let token = issued["token"].as_str().unwrap().to_owned();
	let (status, invitation) = d
		.as_person(
			&alice,
			"POST",
			&format!("/api/tenants/{tenant}/invitations"),
			Some(&context),
			json!({}),
		)
		.await;
	assert_eq!(status, 201, "{invitation}");

	let (status, snapshot) = d
		.as_deployment_administrator("GET", &format!("/api/authorization/{tenant}"), Value::Null)
		.await;
	assert_eq!(status, 200, "{snapshot}");

	// A member may not delete, and the Tenant name must be typed exactly.
	let (status, body) = d.delete(&bob, &bob_context, &tenant).await;
	assert_eq!(status, 403, "{body}");
	let (status, body) = d
		.as_person(
			&alice,
			"POST",
			&format!("/api/tenants/{tenant}/deletion"),
			Some(&context),
			json!({"confirmation":"Team"}),
		)
		.await;
	assert_eq!(status, 400, "{body}");
	let (status, deletion) = d.delete(&alice, &context, &tenant).await;
	assert_eq!(status, 200, "{deletion}");
	assert_eq!(deletion["deleted_by_kind"], "tenant_administrator");
	let deleted_at: chrono::DateTime<chrono::Utc> =
		serde_json::from_value(deletion["deleted_at"].clone()).unwrap();
	let purge_after: chrono::DateTime<chrono::Utc> =
		serde_json::from_value(deletion["purge_after"].clone()).unwrap();
	assert_eq!(purge_after - deleted_at, chrono::Duration::days(30));
	for (method, path) in [
		("GET", format!("/api/authorization/{tenant}/credentials")),
		(
			"POST",
			format!(
				"/api/authorization/{tenant}/credentials/{}/revoke",
				issued["credential"]["id"].as_str().unwrap()
			),
		),
	] {
		let (status, body) = d
			.as_deployment_administrator(method, &path, Value::Null)
			.await;
		assert_eq!(status, 410, "{method} {path}: {body}");
	}

	// Every way in is refused: the members' browsers learn why, while a
	// Subject Credential sees what an unknown Tenant would.
	for (person, context) in [(&alice, &context), (&bob, &bob_context)] {
		let (status, body) = d
			.as_person(person, "GET", "/api/session", Some(context), Value::Null)
			.await;
		assert_eq!(status, 410, "{body}");
		assert_eq!(body["error"], "tenant_deleted");
	}
	let (status, _, _) = d
		.send("GET", "/api/session", None, None, Some(&token), Value::Null)
		.await;
	assert_eq!(status, 401);
	let (status, body) = d.delete(&alice, &context, &tenant).await;
	assert_eq!(status, 410, "{body}");
	// The policy stays as it was deleted, even for a Deployment Administrator.
	let (status, body) = d
		.as_deployment_administrator(
			"POST",
			&format!("/api/authorization/{tenant}"),
			json!({"expected_revision":snapshot["revision"],"bundle":snapshot["bundle"]}),
		)
		.await;
	assert_eq!(status, 410, "{body}");
	// The pending Invitation is revoked, and no restore revives it.
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM tenant_invitations WHERE tenant = '{tenant}' AND revoked_at IS NULL AND redeemed_at IS NULL"
		))
		.await,
		0
	);

	// The switcher no longer offers the Tenant; only Alice may restore it.
	let session = d.session(&bob).await;
	assert!(
		session["mappings"].as_array().unwrap().is_empty(),
		"{session}"
	);
	assert_eq!(deleted(&session, &tenant).unwrap()["restorable"], false);
	let session = d.session(&alice).await;
	assert_eq!(deleted(&session, &tenant).unwrap()["restorable"], true);
	let (status, body) = d.restore(&bob, &bob_context, &tenant).await;
	assert_eq!(status, 403, "{body}");
	assert_eq!(d.deletions(&tenant, "restored_at IS NULL").await, 1);

	let (status, body) = d.restore(&alice, &context, &tenant).await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(
		d.deletions(&tenant, "restored_by_kind = 'tenant_administrator'")
			.await,
		1
	);
	for (person, context) in [(&alice, &context), (&bob, &bob_context)] {
		let (status, body) = d
			.as_person(person, "GET", "/api/session", Some(context), Value::Null)
			.await;
		assert_eq!(status, 200, "{body}");
	}
	let (status, _, _) = d
		.send("GET", "/api/session", None, None, Some(&token), Value::Null)
		.await;
	assert_eq!(status, 200);
	let (status, invitation_preview) = d
		.as_person(
			&bob,
			"POST",
			"/auth/invitations/inspect",
			None,
			json!({"tenant":tenant,"secret":invitation["secret"]}),
		)
		.await;
	assert_eq!(status, 404, "{invitation_preview}");
	// A restored Tenant can be deleted again, which starts a new grace period.
	let (status, body) = d.delete(&alice, &context, &tenant).await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(d.deletions(&tenant, "true").await, 2);
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM dashboard_administration_history WHERE tenant = '{tenant}' AND action IN ('tenant.delete','tenant.restore')"
		))
		.await,
		3
	);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn one_tenants_administrator_neither_deletes_nor_restores_another(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let carol = d.person("carol").await;
	let (team, alice_context) = d.tenant_of(&alice, "Team").await;
	let (other, carol_context) = d.tenant_of(&carol, "Other").await;
	let alice_mapping = alice_context.rsplit(':').next().unwrap().to_owned();
	let carol_mapping = carol_context.rsplit(':').next().unwrap().to_owned();

	// Alice's Mapping acts only in her own Tenant, however the request names it.
	let (status, body) = d.delete(&alice, &alice_context, &other).await;
	assert_eq!(status, 403, "{body}");
	let forged = format!("mapping:{other}:{alice_mapping}");
	let (status, body) = d.delete(&alice, &forged, &other).await;
	assert_eq!(status, 403, "{body}");
	assert_eq!(d.deletions(&other, "true").await, 0);

	let (status, body) = d.delete(&carol, &carol_context, &other).await;
	assert_eq!(status, 200, "{body}");
	for context in [
		alice_context.clone(),
		forged,
		// Carol's own Mapping, presented by Alice's session.
		format!("mapping:{other}:{carol_mapping}"),
	] {
		let (status, body) = d.restore(&alice, &context, &other).await;
		assert_eq!(status, 403, "{context}: {body}");
	}
	assert_eq!(d.deletions(&other, "restored_at IS NULL").await, 1);
	assert!(deleted(&d.session(&alice).await, &other).is_none());
	// Alice's own Tenant is untouched.
	assert_eq!(d.deletions(&team, "true").await, 0);
	let (status, body) = d
		.as_person(
			&alice,
			"GET",
			"/api/session",
			Some(&alice_context),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{body}");
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn only_a_deployment_administrator_restores_a_tenant_one_deleted(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let (tenant, context) = d.tenant_of(&alice, "Team").await;
	let (status, body) = d
		.as_deployment_administrator(
			"POST",
			&format!("/api/tenants/{tenant}/deletion"),
			json!({"confirmation":tenant}),
		)
		.await;
	assert_eq!(status, 200, "{body}");
	assert_eq!(body["deleted_by_kind"], "deployment_administrator");
	assert_eq!(
		deleted(&d.session(&alice).await, &tenant).unwrap()["restorable"],
		false
	);
	let (status, body) = d.restore(&alice, &context, &tenant).await;
	assert_eq!(status, 409, "{body}");
	let (status, listed) = d
		.as_deployment_administrator("GET", "/api/tenants", Value::Null)
		.await;
	assert_eq!(status, 200, "{listed}");
	let row = listed
		.as_array()
		.unwrap()
		.iter()
		.find(|row| row["name"] == tenant.as_str())
		.unwrap();
	assert_eq!(
		row["deletion"]["deleted_by_kind"],
		"deployment_administrator"
	);
	let (status, body) = d
		.as_deployment_administrator(
			"POST",
			&format!("/api/tenants/{tenant}/restore"),
			Value::Null,
		)
		.await;
	assert_eq!(status, 200, "{body}");
	let (status, body) = d
		.as_person(&alice, "GET", "/api/session", Some(&context), Value::Null)
		.await;
	assert_eq!(status, 200, "{body}");
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn leaving_keeps_an_administrator_and_a_returning_member_gets_only_the_invitations_groups(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let bob = d.person("bob").await;
	let (tenant, context) = d.tenant_of(&alice, "Team").await;
	let bob_context = d
		.admit(&tenant, &bob, "bob", json!(["administrators"]))
		.await;
	let leave = format!("/api/tenants/{tenant}/leave");

	// Bob leaves; his administrator group stays on the subject but no
	// enabled Mapping reaches it.
	let (status, body) = d
		.as_person(&bob, "POST", &leave, Some(&bob_context), Value::Null)
		.await;
	assert_eq!(status, 204, "{body}");
	assert!(d.context_or_none(&bob, &tenant).await.is_none());
	// Alice is now the last administrator and may not leave.
	let (status, body) = d
		.as_person(&alice, "POST", &leave, Some(&context), Value::Null)
		.await;
	assert_eq!(status, 409, "{body}");

	// Bob returns through an Invitation for members only.
	let (status, invitation) = d
		.as_person(
			&alice,
			"POST",
			&format!("/api/tenants/{tenant}/invitations"),
			Some(&context),
			json!({"groups":["members"]}),
		)
		.await;
	assert_eq!(status, 201, "{invitation}");
	let (status, request) = d
		.as_person(
			&bob,
			"POST",
			"/auth/invitations/redeem",
			None,
			json!({"tenant":tenant,"secret":invitation["secret"]}),
		)
		.await;
	assert_eq!(status, 200, "{request}");
	let (status, approved) = d
		.as_person(
			&alice,
			"POST",
			&format!(
				"/api/tenants/{tenant}/registrations/{}/approve",
				request["id"].as_str().unwrap()
			),
			Some(&context),
			json!({"subject":"bob","groups":["members"]}),
		)
		.await;
	assert_eq!(status, 200, "{approved}");
	let (_, snapshot) = d
		.as_deployment_administrator("GET", &format!("/api/authorization/{tenant}"), Value::Null)
		.await;
	assert_eq!(
		snapshot["bundle"]["subjects"]["bob"]["groups"],
		json!(["members"])
	);
	let bob_context = d.context(&bob, &tenant).await;
	let (status, body) = d
		.as_person(
			&bob,
			"PUT",
			&format!("/api/tenants/{tenant}/label"),
			Some(&bob_context),
			json!({"label":"Mine"}),
		)
		.await;
	assert_eq!(status, 403, "{body}");
	assert_eq!(
		d.count(&format!(
			"SELECT count(*) FROM dashboard_administration_history WHERE tenant = '{tenant}' AND action = 'tenant.leave'"
		))
		.await,
		1
	);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn a_deleted_personal_tenant_is_replaced_only_on_request(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(
		runtime,
		TenancyRules {
			personal_tenants: true,
			..TenancyRules::of(Offering::SelfHosted)
		},
	)
	.await;
	let alice = d.person("alice").await;
	let session = d.session(&alice).await;
	let old = session["mappings"][0]["tenant"]
		.as_str()
		.unwrap()
		.to_owned();
	let context = d.context(&alice, &old).await;
	let (status, body) = d.delete(&alice, &context, &old).await;
	assert_eq!(status, 200, "{body}");

	// Signing in again creates nothing; Alice may ask for a new one.
	let session = d.session(&alice).await;
	assert!(
		session["mappings"].as_array().unwrap().is_empty(),
		"{session}"
	);
	assert_eq!(session["personal_tenant_deleted"], true);
	assert_eq!(deleted(&session, &old).unwrap()["personal"], true);
	assert_eq!(
		d.count("SELECT count(*) FROM tenants WHERE personal").await,
		1
	);
	let (status, created) = d
		.as_person(&alice, "POST", "/api/tenants/personal", None, Value::Null)
		.await;
	assert_eq!(status, 201, "{created}");
	let new = created["name"].as_str().unwrap().to_owned();
	assert_ne!(new, old);
	let session = d.session(&alice).await;
	assert_eq!(session["mappings"][0]["tenant"], new.as_str());
	assert_eq!(session["mappings"][0]["personal"], true);
	assert_eq!(session["personal_tenant_deleted"], false);
	let (status, body) = d
		.as_person(&alice, "POST", "/api/tenants/personal", None, Value::Null)
		.await;
	assert_eq!(status, 409, "{body}");

	// The old one can still be restored, but it is no longer her Personal Tenant.
	let (status, body) = d.restore(&alice, &context, &old).await;
	assert_eq!(status, 200, "{body}");
	let session = d.session(&alice).await;
	let restored = session["mappings"]
		.as_array()
		.unwrap()
		.iter()
		.find(|mapping| mapping["tenant"] == old.as_str())
		.unwrap();
	assert_eq!(restored["personal"], false);
	d.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn deletion_stops_the_tenants_runs_and_a_restore_cancels_them(
	#[from(common::runtime)] runtime: common::RuntimeFuture,
) {
	let d = Deployment::new(runtime, open_rules()).await;
	let alice = d.person("alice").await;
	let (tenant, context) = d.tenant_of(&alice, "Team").await;
	let (status, workspace) = d
		.as_person(
			&alice,
			"POST",
			"/api/workspaces",
			Some(&context),
			json!({"title":"Work","goal":"Run something"}),
		)
		.await;
	assert_eq!(status, 200, "{workspace}");
	let workspace: Uuid = workspace["id"].as_str().unwrap().parse().unwrap();
	let (status, task) = d
		.as_person(
			&alice,
			"POST",
			&format!("/api/workspaces/{workspace}/tasks"),
			Some(&context),
			json!({"title":"work","description":"keep working"}),
		)
		.await;
	assert_eq!(status, 200, "{task}");
	let task: Uuid = task["id"].as_str().unwrap().parse().unwrap();
	// A Run of the Tenant as a worker would hold it, recorded directly.
	let pool = d.federation.store.pool.fixture_driver();
	let run = Uuid::new_v4();
	let mut tx = pool.begin().await.unwrap();
	sqlx::query("SELECT set_config('aidash.input_ledger_worker', 'true', true)")
		.execute(&mut *tx)
		.await
		.unwrap();
	sqlx::query(
		"INSERT INTO runs (id, task_id, workspace_id, home_node, node, agent_id, agent_version, phase, control, context, pending, tenant) \
		 VALUES ($1, $2, $3, $4 || '/' || (SELECT node FROM workspaces WHERE id = $3), (SELECT node FROM workspaces WHERE id = $3), 'agent', '1.0.0', 'THINKING', 'ACTIVE', '{}'::jsonb, '{}'::jsonb, (SELECT tenant FROM workspaces WHERE id = $3))",
	)
	.bind(run)
	.bind(task)
	.bind(workspace)
	.bind(d.federation.config.deployment.as_str())
	.execute(&mut *tx)
	.await
	.unwrap();
	sqlx::query(
		"INSERT INTO authorization_execution (run_id, task_id, workspace_id, tenant, credential_id, root_subject, subject_chain) \
		 SELECT $1, $2, $3, $4, credential_id, subject, ARRAY[subject, 'agent'] FROM dashboard_mappings WHERE tenant = $4",
	)
	.bind(run)
	.bind(task)
	.bind(workspace)
	.bind(&tenant)
	.execute(&mut *tx)
	.await
	.unwrap();
	tx.commit().await.unwrap();
	let scope = aidash_application::tenancy::TenancyScope::tenant(&tenant).unwrap();
	// A blocked mutation must keep deletion's policy fence until its effect commits.
	for (table, id, endpoint, input) in [
		(
			"runs",
			run,
			format!("/api/runs/{run}/control"),
			json!({"action":"pause"}),
		),
		(
			"tasks",
			task,
			format!("/api/tasks/{task}/abandon"),
			json!({"revision":0,"reason":"Deployment Administrator cleanup"}),
		),
	] {
		if table == "tasks" {
			let query = Query::update()
				.table(Alias::new("tasks"))
				.value(Alias::new("status"), "FAILED")
				.and_where(Expr::col("id").eq(Expr::value(task)))
				.to_string(PostgresQueryBuilder);
			d.federation
				.store
				.pool
				.atomic(&scope, async |tx| {
					aidash_server::database::native::query(&query)
						.execute(tx)
						.await?;
					Ok(())
				})
				.await
				.unwrap();
		}
		let mut row_lock = d.federation.store.pool.begin(&scope).await.unwrap();
		let pid: i32 = aidash_server::database::native::query_scalar(
			&Query::select()
				.expr(Expr::cust("pg_backend_pid()"))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&mut *row_lock)
		.await
		.unwrap();
		let query = Query::select()
			.column(Alias::new("id"))
			.from(Alias::new(table))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder);
		let _: Uuid = aidash_server::database::native::query_scalar(&query)
			.scalar_one(&mut *row_lock)
			.await
			.unwrap();
		let operation = d.as_deployment_administrator("POST", &endpoint, input);
		tokio::pin!(operation);
		tokio::select! {
			result = &mut operation => panic!("mutation did not wait for its row: {result:?}"),
			() = common::wait_for_blocked(&d.federation.store.pool, pid, 1) => {},
		}
		let mut deletion_fence = d.federation.store.pool.begin(&scope).await.unwrap();
		{
			let query = Query::select()
				.column(Alias::new("tenant"))
				.from(Alias::new("authorization_bundles"))
				.and_where(Expr::col("tenant").eq(Expr::value(&tenant)))
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder);
			let acquire = async {
				aidash_server::database::native::query_scalar::<String>(&query)
					.scalar_one(&mut *deletion_fence)
					.await
					.unwrap()
			};
			tokio::pin!(acquire);
			assert!(
				tokio::time::timeout(std::time::Duration::from_millis(250), &mut acquire)
					.await
					.is_err(),
				"{endpoint} released its policy fence before mutation"
			);
			row_lock.rollback().await.unwrap();
			let (status, body) = operation.await;
			assert_eq!(status, 200, "{endpoint}: {body}");
			acquire.await;
		}
		deletion_fence.rollback().await.unwrap();
	}
	let state = || async {
		sqlx::query_as::<_, (String, Option<String>)>(
			"SELECT control, error FROM runs WHERE id = $1",
		)
		.bind(run)
		.fetch_one(pool)
		.await
		.unwrap()
	};

	let (status, body) = d.delete(&alice, &context, &tenant).await;
	assert_eq!(status, 200, "{body}");
	// Paused, so no worker reclaims a Run whose authority is now refused.
	assert_eq!(
		state().await,
		("PAUSED".into(), Some("tenant deleted".into()))
	);
	let (status, body) = d.restore(&alice, &context, &tenant).await;
	assert_eq!(status, 200, "{body}");
	// Restoring brings the data back, never the work.
	assert_eq!(
		state().await,
		("CANCELLED".into(), Some("tenant deleted".into()))
	);
	d.finish().await;
}

#[path = "deletion/purge.rs"]
mod purge;
