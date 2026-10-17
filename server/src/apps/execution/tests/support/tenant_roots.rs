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
/// built-ins and returns its scope. Every workspace belongs to a Tenant
/// (ADR 0043), so Store fixtures own their workspaces in one.
#[allow(dead_code)]
pub async fn tenant_scope(
	store: &aidash_server::store::Store,
	name: &str,
) -> aidash_application::tenancy::TenancyScope {
	tenant_root_in(store.pool.driver(), name).await;
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
	let tenant = aidash_domain::tenancy::TenantName::new(name).unwrap();
	aidash_server::registry::Registry::new(store.pool.clone(), &store.node_id)
		.unwrap()
		.seed(&tenant)
		.await
		.expect("the built-in definitions are seeded");
	aidash_application::tenancy::TenancyScope::Tenant(tenant)
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
	store
		.claim(scope, task.id, task.revision, &owner, agent)
		.await
		.expect("the local Run is queued");
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
				.expr(Expr::cust("'{}'::jsonb"))
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
		.execute(store.pool.driver())
		.await
		.expect("the admitted remote Run is written");
	store
		.run(&aidash_application::tenancy::TEST_FIXTURE, id)
		.await
		.expect("the remote Run is readable")
}
