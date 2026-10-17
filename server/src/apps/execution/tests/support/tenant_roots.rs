//! Fixtures create Tenants under fixed names, which the Tenant creation API
//! never does: it assigns them (ADR 0050). These write the root record a
//! Tenant needs before its first policy, as the API's creation steps would.
use aidash_server::federation::Federation;

#[allow(dead_code)]
pub async fn tenant_root(f: &Federation, name: &str) {
	tenant_root_in(f.store.pool.driver(), name).await;
	// Tenant creation writes every built-in into the new Registry (ADR 0033).
	f.registry
		.seed(&aidash_domain::tenancy::TenantName::new(name).unwrap())
		.await
		.expect("the built-in definitions are seeded");
}

#[allow(dead_code)]
pub async fn tenant_root_in(pool: &sqlx::PgPool, name: &str) {
	use reinhardt::query::{
		Alias, Expr, OnConflict, PostgresQueryBuilder, Query, QueryStatementBuilder,
	};
	let sql = Query::insert()
		.into_table(Alias::new("tenants"))
		.columns(["name", "label", "personal", "created_at", "created_by_kind"].map(Alias::new))
		.from_subquery(
			Query::select()
				.expr(Expr::cust("$1"))
				.expr(Expr::cust("$1"))
				.expr(Expr::cust("false"))
				.expr(Expr::cust("clock_timestamp()"))
				.expr(Expr::cust("'deployment_administrator'"))
				.to_owned(),
		)
		.on_conflict(
			OnConflict::column(Alias::new("name"))
				.do_nothing()
				.to_owned(),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&sql)
		.bind(name)
		.execute(pool)
		.await
		.expect("the Tenant root record is written");
}

/// Writes a Tenant whose policy allows its subjects every action, seeds its
/// built-ins and returns its scope; a Tenant whose policy exists is reused. Every workspace belongs to a Tenant
/// (ADR 0043), so Store fixtures own their workspaces in one.
#[allow(dead_code)]
pub async fn tenant_scope(
	store: &aidash_server::store::Store,
	name: &str,
) -> aidash_application::tenancy::TenancyScope {
	tenant_root_in(store.pool.driver(), name).await;
	let tenant = aidash_domain::tenancy::TenantName::new(name).unwrap();
	let scope = aidash_application::tenancy::TenancyScope::Tenant(tenant.clone());
	// Suites may share one fixture Tenant between a fixture and its tests.
	let written: bool =
		sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM authorization_bundles WHERE tenant = $1)")
			.bind(name)
			.fetch_one(store.pool.driver())
			.await
			.unwrap();
	if written {
		return scope;
	}
	let bundle = serde_json::json!({"tenant":name,"subjects":{"alice":{"kind":"user"}},
		"policies":[{"id":"fixture","effect":"allow","subjects":{"any":true},"actions":["*"],"resources":{"kinds":["*"]}}]});
	aidash_server::authorization::Authorization {
		pool: store.pool.clone(),
	}
	.replace(
		name,
		0,
		serde_json::from_value(bundle).unwrap(),
		"deployment_administrator",
	)
	.await
	.expect("the Tenant policy is written");
	aidash_server::registry::Registry::new(store.pool.clone(), &store.node_id)
		.unwrap()
		.seed(&tenant)
		.await
		.expect("the built-in definitions are seeded");
	scope
}

/// Queues a local Run of `agent` for `task` through the claim primitive, the
/// persistence step of a subject's admission. No Run is accepted without one
/// (ADR 0043).
#[allow(dead_code)]
pub async fn local_run(
	store: &aidash_server::store::Store,
	scope: &aidash_application::tenancy::TenancyScope,
	task: &aidash_server::domain::Task,
	agent: &aidash_server::registry::Entry,
) -> aidash_server::domain::Run {
	let owner = aidash_server::domain::qualified_agent(&store.node_id, &agent.id, &agent.version);
	// A task the agent already claimed has its Run.
	if task.owner.as_deref() != Some(owner.as_str()) {
		store
			.claim(scope, task.id, task.revision, &owner, agent)
			.await
			.expect("the local Run is queued");
	}
	store
		.runs(scope)
		.await
		.unwrap()
		.into_iter()
		.find(|run| run.task_id == task.id)
		.expect("the claim queues a Run")
}

/// Writes the Run a scoped receiver admission records for `task` from `home`:
/// the workspace and task live at the home Node. Legacy acceptance of remote
/// Runs is gone (ADR 0043), so fixtures write the row the admission would.
#[allow(dead_code)]
pub async fn remote_run(
	store: &aidash_server::store::Store,
	task: &aidash_server::domain::Task,
	home: &str,
	agent_id: &str,
	agent_version: &str,
) -> aidash_server::domain::Run {
	use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query, QueryStatementBuilder};
	let id = uuid::Uuid::new_v4();
	let sql = Query::insert()
		.into_table(Alias::new("runs"))
		.columns(
			[
				"id",
				"task_id",
				"workspace_id",
				"home_node",
				"agent_id",
				"agent_version",
				"context",
			]
			.map(Alias::new),
		)
		.from_subquery(
			Query::select()
				.expr(Expr::cust("$1"))
				.expr(Expr::cust("$2"))
				.expr(Expr::cust("$3"))
				.expr(Expr::cust("$4"))
				.expr(Expr::cust("$5"))
				.expr(Expr::cust("$6"))
				.expr(Expr::cust("$7"))
				.to_owned(),
		)
		.to_string(PostgresQueryBuilder);
	sqlx::query(&sql)
		.bind(id)
		.bind(task.id)
		.bind(task.workspace_id)
		.bind(home)
		.bind(agent_id)
		.bind(agent_version)
		.bind(serde_json::json!(aidash_server::context::Context::default()))
		.execute(store.pool.driver())
		.await
		.expect("the admitted remote Run is written");
	store
		.run(&aidash_application::tenancy::TEST_FIXTURE, id)
		.await
		.expect("the remote Run is readable")
}

/// Writes the Tenant as [`tenant_scope`] does and returns a Subject Credential
/// for its `alice`, for suites that call Tenant routes and do work, which a
/// Deployment Administrator no longer does (ADR 0043).
#[allow(dead_code)]
pub async fn tenant_token(store: &aidash_server::store::Store, tenant: &str) -> String {
	tenant_scope(store, tenant).await;
	aidash_server::authorization::Authorization {
		pool: store.pool.clone(),
	}
	.issue_credential(tenant, "alice", 3600, "deployment_administrator")
	.await
	.expect("the Subject Credential is issued")
	.token
}

/// Makes `agent` runnable in `tenant`, as its Tenant Administrator would: the
/// agent is a subject of the Tenant's policy, and it and every definition it
/// names are approved in the catalog (built-ins need no approval).
#[allow(dead_code)]
pub async fn approve_agent(f: &Federation, tenant: &str, agent: &aidash_server::registry::Entry) {
	let authorization = aidash_server::authorization::Authorization {
		pool: f.store.pool.clone(),
	};
	let snapshot = authorization.snapshot(tenant).await.unwrap();
	let subject =
		aidash_server::domain::qualified_agent(&f.config.node_id, &agent.id, &agent.version);
	let mut bundle = serde_json::to_value(&snapshot.bundle).unwrap();
	if bundle["subjects"].get(&subject).is_none() {
		bundle["subjects"][&subject] = serde_json::json!({"kind":"agent"});
		authorization
			.replace(
				tenant,
				snapshot.revision,
				serde_json::from_value(bundle).unwrap(),
				"deployment_administrator",
			)
			.await
			.expect("the agent becomes a subject of the Tenant");
	}
	let name = aidash_domain::tenancy::TenantName::new(tenant).unwrap();
	let mut pending = vec![aidash_domain::registry::EntityRef {
		id: agent.id.clone(),
		version: agent.version.clone(),
	}];
	let mut order = Vec::new();
	while let Some(reference) = pending.pop() {
		if aidash_application::registry::system::builtin_reference(&reference)
			|| order.contains(&reference)
		{
			continue;
		}
		let entry = f
			.registry
			.get(&name, &reference.id, &reference.version)
			.await
			.expect("an approved graph is registered in its Tenant");
		pending.extend(aidash_application::authorization::catalog::approvals::dependencies(&entry));
		order.push(reference);
	}
	// Dependencies before the definitions that name them.
	for reference in order.into_iter().rev() {
		let current = authorization
			.catalog(tenant)
			.await
			.unwrap()
			.into_iter()
			.find(|binding| {
				binding.entry_id == reference.id && binding.entry_version == reference.version
			});
		if current.as_ref().is_some_and(|binding| binding.enabled) {
			continue;
		}
		authorization
			.set_catalog(
				tenant,
				&reference,
				current.map_or(0, |binding| binding.revision),
				true,
				"deployment_administrator",
			)
			.await
			.expect("the definition is approved");
	}
}

/// Claims `task` for `agent` as the Tenant's `alice`, the scoped admission that
/// gives a local Run its execution grant (ADR 0043), and returns the Run.
#[allow(dead_code)]
pub async fn admitted_run(
	f: &Federation,
	tenant: &str,
	task: &aidash_server::domain::Task,
	agent: &aidash_server::registry::Entry,
) -> aidash_server::domain::Run {
	approve_agent(f, tenant, agent).await;
	let authorization = aidash_server::authorization::Authorization {
		pool: f.store.pool.clone(),
	};
	let token = authorization
		.issue_credential(tenant, "alice", 3600, "deployment_administrator")
		.await
		.unwrap()
		.token;
	let aidash_server::authorization::identity::Actor::Subject(identity) =
		authorization.authenticate(&token).await.unwrap()
	else {
		panic!("a Subject Credential authenticates a subject")
	};
	aidash_server::authorization::services::execution::claim(
		f,
		&identity,
		task.id,
		task.revision,
		&aidash_domain::registry::EntityRef {
			id: agent.id.clone(),
			version: agent.version.clone(),
		},
	)
	.await
	.expect("the subject admits the Run");
	f.store
		.runs(&aidash_application::tenancy::TenancyScope::tenant(tenant).unwrap())
		.await
		.unwrap()
		.into_iter()
		.find(|run| run.task_id == task.id)
		.expect("admission queues a Run")
}
