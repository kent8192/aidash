//! The links that make a flow (#1012, ADR 0143), read on a caller-owned Tenant
//! transaction: Task parents, the Tasks a Run created or delegated
//! (`authorization_task_origins`), the Duties a Run set up and the Tasks a
//! Duty started (`tasks.duty_id`).
use crate::database::native;
use crate::{Error, Result};
use aidash_application::flows::FlowGraph;
use reinhardt::db::backends::TransactionExecutor;
use reinhardt::query::{
	Alias, Expr, ExprTrait, JoinType, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder as _, TableRef,
};
use std::collections::BTreeMap;
use uuid::Uuid;

pub(crate) struct NativeFlowGraph<'a> {
	pub tx: &'a mut dyn TransactionExecutor,
	pub tenant: String,
}

fn ids(values: &[Uuid]) -> impl Iterator<Item = Expr> + '_ {
	values.iter().map(|id| Expr::value(*id))
}

impl NativeFlowGraph<'_> {
	async fn column(
		&mut self,
		table: &str,
		select: &str,
		by: &str,
		values: &[Uuid],
	) -> Result<Vec<Uuid>> {
		if values.is_empty() {
			return Ok(vec![]);
		}
		native::query_scalar(
			&Query::select()
				.column(Alias::new(select))
				.from(Alias::new(table))
				.and_where(Expr::col("tenant").eq(self.tenant.clone()))
				.and_where(Expr::col(Alias::new(by)).is_in(ids(values)))
				.order_by(Alias::new(select), Order::Asc)
				.to_string(PostgresQueryBuilder),
		)
		.scalar_all(&mut *self.tx)
		.await
	}

	/// The Root Task's Node: the Node of the Thread it works in.
	pub(crate) async fn task_node(&mut self, task: Uuid) -> Result<String> {
		native::query_scalar(
			&Query::select()
				.column((Alias::new("w"), Alias::new("node")))
				.from_as(Alias::new("tasks"), Alias::new("t"))
				.join(
					JoinType::InnerJoin,
					TableRef::table_alias(Alias::new("threads"), Alias::new("w")),
					Expr::col((Alias::new("w"), Alias::new("id")))
						.equals((Alias::new("t"), Alias::new("thread_id")))
						.and(
							Expr::col((Alias::new("w"), Alias::new("tenant")))
								.equals((Alias::new("t"), Alias::new("tenant"))),
						),
				)
				.and_where(
					Expr::col((Alias::new("t"), Alias::new("tenant"))).eq(self.tenant.clone()),
				)
				.and_where(Expr::col((Alias::new("t"), Alias::new("id"))).eq(Expr::value(task)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut *self.tx)
		.await?
		.ok_or_else(|| Error::NotFound("task not found".into()))
	}

	/// The flow's Runs that have not ended.
	pub(crate) async fn open_runs(&mut self, runs: &[Uuid]) -> Result<Vec<Uuid>> {
		if runs.is_empty() {
			return Ok(vec![]);
		}
		native::query_scalar(
			&Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("task_executions"))
				.and_where(Expr::col("tenant").eq(self.tenant.clone()))
				.and_where(Expr::col("id").is_in(ids(runs)))
				.and_where(Expr::col("phase").is_not_in(["COMPLETED", "FAILED", "CANCELLED"]))
				.and_where(Expr::col("control").ne("CANCELLED"))
				.order_by(Alias::new("id"), Order::Asc)
				.to_string(PostgresQueryBuilder),
		)
		.scalar_all(&mut *self.tx)
		.await
	}

	/// The Remote Grants that hand the flow's Tasks to another Deployment,
	/// each with whether it is revoked and whether its Task has ended
	/// (`home::ending_statuses`).
	pub(crate) async fn remote_grants(
		&mut self,
		tasks: &[Uuid],
	) -> Result<Vec<(Uuid, bool, bool)>> {
		if tasks.is_empty() {
			return Ok(vec![]);
		}
		let (g, t) = (Alias::new("g"), Alias::new("t"));
		native::query_as(
			&Query::select()
				.expr_as(Expr::col((g.clone(), Alias::new("id"))), Alias::new("id"))
				.expr_as(
					Expr::col((g.clone(), Alias::new("revoked"))),
					Alias::new("revoked"),
				)
				.expr_as(
					Expr::col((t.clone(), Alias::new("status"))).is_in(
						aidash_domain::federation::execution::home::ending_statuses()
							.map(|status| status.as_str()),
					),
					Alias::new("ended"),
				)
				.from_as(Alias::new("authorization_remote_grants"), g.clone())
				.join(
					JoinType::InnerJoin,
					TableRef::table_alias(Alias::new("tasks"), t.clone()),
					Expr::col((t.clone(), Alias::new("id")))
						.equals((g.clone(), Alias::new("task_id")))
						.and(
							Expr::col((t.clone(), Alias::new("tenant")))
								.equals((g.clone(), Alias::new("tenant"))),
						),
				)
				.and_where(Expr::col((g.clone(), Alias::new("tenant"))).eq(self.tenant.clone()))
				.and_where(Expr::col((g.clone(), Alias::new("task_id"))).is_in(ids(tasks)))
				.order_by((g, Alias::new("id")), Order::Asc)
				.to_string(PostgresQueryBuilder),
		)
		.columns(&["id", "revoked", "ended"])
		.fetch_all(&mut *self.tx)
		.await
	}

	/// Active Subnodes a Run of the flow's Tasks created, with their creating Run.
	pub(crate) async fn active_subnodes(&mut self, tasks: &[Uuid]) -> Result<Vec<(Uuid, Uuid)>> {
		if tasks.is_empty() {
			return Ok(vec![]);
		}
		native::query_as(
			&Query::select()
				.columns([Alias::new("id"), Alias::new("creator_run_id")])
				.from(Alias::new("subnodes"))
				.and_where(Expr::col("tenant").eq(self.tenant.clone()))
				.and_where(Expr::col("creator_task_id").is_in(ids(tasks)))
				.and_where(Expr::col("state").eq("active"))
				.order_by(Alias::new("id"), Order::Asc)
				.to_string(PostgresQueryBuilder),
		)
		.columns(&["id", "creator_run_id"])
		.fetch_all(&mut *self.tx)
		.await
	}
}

async fn pairs(tx: &mut dyn TransactionExecutor, sql: &str) -> Result<Vec<(Uuid, Uuid)>> {
	native::query_as(sql)
		.columns(&["task", "parent"])
		.fetch_all(&mut *tx)
		.await
}

#[async_trait::async_trait]
impl FlowGraph for NativeFlowGraph<'_> {
	async fn children(&mut self, tasks: &[Uuid]) -> aidash_application::Result<Vec<Uuid>> {
		Ok(self.column("tasks", "id", "parent_id", tasks).await?)
	}
	async fn runs(&mut self, tasks: &[Uuid]) -> aidash_application::Result<Vec<Uuid>> {
		Ok(self
			.column("task_executions", "id", "task_id", tasks)
			.await?)
	}
	async fn created_by_runs(&mut self, runs: &[Uuid]) -> aidash_application::Result<Vec<Uuid>> {
		Ok(self
			.column(
				"authorization_task_origins",
				"task_id",
				"source_run_id",
				runs,
			)
			.await?)
	}
	async fn duties(
		&mut self,
		tasks: &[Uuid],
		runs: &[Uuid],
	) -> aidash_application::Result<Vec<Uuid>> {
		Ok(
			crate::apps::workspaces::repositories::duties::created_by_flow(
				&mut *self.tx,
				&self.tenant,
				tasks,
				runs,
			)
			.await?,
		)
	}
	async fn duty_tasks(&mut self, duties: &[Uuid]) -> aidash_application::Result<Vec<Uuid>> {
		Ok(self.column("tasks", "id", "duty_id", duties).await?)
	}
	async fn parents(
		&mut self,
		tasks: &[Uuid],
	) -> aidash_application::Result<BTreeMap<Uuid, Uuid>> {
		if tasks.is_empty() {
			return Ok(BTreeMap::new());
		}
		let tenant = self.tenant.clone();
		let t = Alias::new("t");
		let tree = Query::select()
			.expr_as(Expr::col((t.clone(), Alias::new("id"))), Alias::new("task"))
			.expr_as(
				Expr::col((t.clone(), Alias::new("parent_id"))),
				Alias::new("parent"),
			)
			.from_as(Alias::new("tasks"), t.clone())
			.and_where(Expr::col((t.clone(), Alias::new("tenant"))).eq(tenant.clone()))
			.and_where(Expr::col((t.clone(), Alias::new("id"))).is_in(ids(tasks)))
			.and_where(Expr::col((t.clone(), Alias::new("parent_id"))).is_not_null())
			.to_string(PostgresQueryBuilder);
		let (o, r) = (Alias::new("o"), Alias::new("r"));
		let origins = Query::select()
			.expr_as(
				Expr::col((o.clone(), Alias::new("task_id"))),
				Alias::new("task"),
			)
			.expr_as(
				Expr::col((r.clone(), Alias::new("task_id"))),
				Alias::new("parent"),
			)
			.from_as(Alias::new("authorization_task_origins"), o.clone())
			.join(
				JoinType::InnerJoin,
				TableRef::table_alias(Alias::new("task_executions"), r.clone()),
				Expr::col((r.clone(), Alias::new("id")))
					.equals((o.clone(), Alias::new("source_run_id")))
					.and(
						Expr::col((r.clone(), Alias::new("tenant")))
							.equals((o.clone(), Alias::new("tenant"))),
					),
			)
			.and_where(Expr::col((o.clone(), Alias::new("tenant"))).eq(tenant.clone()))
			.and_where(Expr::col((o.clone(), Alias::new("task_id"))).is_in(ids(tasks)))
			.to_string(PostgresQueryBuilder);
		let d = Alias::new("d");
		let duties = Query::select()
			.expr_as(Expr::col((t.clone(), Alias::new("id"))), Alias::new("task"))
			.expr_as(
				Expr::col((d.clone(), Alias::new("created_by_task_id"))),
				Alias::new("parent"),
			)
			.from_as(Alias::new("tasks"), t.clone())
			.join(
				JoinType::InnerJoin,
				TableRef::table_alias(Alias::new("duties"), d.clone()),
				Expr::col((d.clone(), Alias::new("id")))
					.equals((t.clone(), Alias::new("duty_id")))
					.and(
						Expr::col((d.clone(), Alias::new("tenant")))
							.equals((t.clone(), Alias::new("tenant"))),
					),
			)
			.and_where(Expr::col((t.clone(), Alias::new("tenant"))).eq(tenant))
			.and_where(Expr::col((t.clone(), Alias::new("id"))).is_in(ids(tasks)))
			.and_where(Expr::col((d.clone(), Alias::new("created_by_task_id"))).is_not_null())
			.to_string(PostgresQueryBuilder);
		// The parent Task first, then the creating Run's Task, then the Task
		// that set up the Duty that started it.
		let mut parents = BTreeMap::new();
		for sql in [duties, origins, tree] {
			for (task, parent) in pairs(&mut *self.tx, &sql).await? {
				if task != parent {
					parents.insert(task, parent);
				}
			}
		}
		Ok(parents)
	}
}
