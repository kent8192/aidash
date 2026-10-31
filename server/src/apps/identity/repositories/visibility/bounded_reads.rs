//! Keyset candidate queries and their live authorization transaction.
use crate::{apps::identity::services::access::Access, federation::Federation};
use aidash_application::{
	Error, Result,
	authorization::bounded_reads::{HumanState, PAGE_SIZE},
	ports::authorization::bounded_reads::Scope,
};
use aidash_domain::{HumanRequest, RawRun, RunInspection, policy::Resource};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use reinhardt::query::{
	Alias, ColumnRef::Asterisk, Condition, Expr, ExprTrait as _, Order, PostgresQueryBuilder,
	Query, QueryStatementBuilder as _, SimpleExpr,
};
use uuid::Uuid;

pub(crate) struct Reads<'a> {
	pub federation: &'a Federation,
	pub access: &'a mut Access,
	pub task_workspace: Option<Uuid>,
	pub selected_run: Option<RunInspection>,
}

impl Reads<'_> {
	async fn cursor(
		&mut self,
		table: &str,
		id: Uuid,
		owner: Option<(&str, Uuid)>,
		workspaces: &[Uuid],
	) -> Result<DateTime<Utc>> {
		let mut query = Query::select();
		query
			.column(Alias::new("created_at"))
			.from(Alias::new(table))
			.and_where(Expr::col(Alias::new("id")).eq(Expr::value(id)));
		if let Some((column, owner)) = owner {
			query.and_where(Expr::col(Alias::new(column)).eq(Expr::value(owner)));
		}
		query.and_where(workspace_filter(workspaces));
		let query = query.to_string(PostgresQueryBuilder);
		crate::database::native::query_scalar(&query)
			.scalar_optional(&mut **self.access.tx)
			.await
			.map_err(Error::from)?
			.ok_or_else(|| Error::Invalid("invalid cursor".into()))
	}
}

#[async_trait]
impl Scope for Reads<'_> {
	async fn require_task(&mut self, id: Uuid) -> Result<()> {
		let task = self.access.task_read(id).await.map_err(Error::from)?;
		self.access
			.require_workspace(task.workspace_id, "workspace.read")
			.await
			.map_err(Error::from)?;
		self.task_workspace = Some(task.workspace_id);
		Ok(())
	}
	async fn require_run(&mut self, id: Uuid) -> Result<RunInspection> {
		let run = crate::apps::identity::repositories::execution::details::visible_within(
			self.federation,
			self.access,
			id,
		)
		.await?;
		if !run.is_local(self.federation.config.deployment.as_str())
			&& !self
				.access
				.allowed(run.workspace_id, "workspace.read")
				.await
				.map_err(Error::from)?
		{
			return Err(Error::Forbidden);
		}
		self.selected_run = Some(run.clone());
		Ok(run)
	}
	async fn visible_run(&mut self, id: Uuid) -> Result<Option<RunInspection>> {
		match self.require_run(id).await {
			Ok(run) => Ok(Some(run)),
			Err(Error::Forbidden) => Ok(None),
			Err(error) => Err(error),
		}
	}
	async fn runs(&mut self, task: Uuid, cursor: Option<Uuid>) -> Result<Vec<RawRun>> {
		let workspaces = [self.task_workspace.ok_or(Error::Forbidden)?];
		let position = match cursor {
			Some(id) => Some((
				self.cursor("runs", id, Some(("task_id", task)), &workspaces)
					.await?,
				id,
			)),
			None => None,
		};
		let mut query = Query::select();
		query
			.column(Asterisk)
			.from(Alias::new("runs"))
			.and_where(Expr::col(Alias::new("task_id")).eq(Expr::value(task)))
			.and_where(workspace_filter(&workspaces))
			.order_by(Alias::new("created_at"), Order::Desc)
			.order_by(Alias::new("id"), Order::Desc)
			.limit(PAGE_SIZE as u64);
		if let Some((at, id)) = position {
			query.cond_where(before(at, id));
		}
		aidash_server::database::query_as(&query.to_string(PostgresQueryBuilder))
			.fetch_all(&mut **self.access.tx)
			.await
			.map_err(Into::into)
	}
	async fn humans(
		&mut self,
		run: Option<Uuid>,
		state: HumanState,
		cursor: Option<Uuid>,
	) -> Result<Vec<HumanRequest>> {
		let workspaces = if run.is_some() {
			vec![
				self.selected_run
					.as_ref()
					.ok_or(Error::Forbidden)?
					.workspace_id,
			]
		} else {
			self.access
				.visible("workspace.read")
				.await
				.map_err(Error::from)?
		};
		let position = match cursor {
			Some(id) => Some((
				self.cursor(
					"human_requests",
					id,
					run.map(|run| ("run_id", run)),
					&workspaces,
				)
				.await?,
				id,
			)),
			None => None,
		};
		let mut query = Query::select();
		query
			.column(Asterisk)
			.from(Alias::new("human_requests"))
			.and_where(workspace_filter(&workspaces))
			.order_by(Alias::new("created_at"), Order::Desc)
			.order_by(Alias::new("id"), Order::Desc)
			.limit(PAGE_SIZE as u64);
		if let Some(id) = run {
			query.and_where(Expr::col(Alias::new("run_id")).eq(Expr::value(id)));
		}
		match state {
			HumanState::Pending => {
				query.and_where(Expr::col(Alias::new("response")).is_null());
			}
			HumanState::Answered => {
				query.and_where(Expr::col(Alias::new("response")).is_not_null());
			}
			HumanState::All => {}
		}
		if let Some((at, id)) = position {
			query.cond_where(before(at, id));
		}
		aidash_server::database::query_as(&query.to_string(PostgresQueryBuilder))
			.fetch_all(&mut **self.access.tx)
			.await
			.map_err(Into::into)
	}
	async fn human_resource(&mut self, request: &HumanRequest) -> Result<Resource> {
		self.access
			.human_resource(request)
			.await
			.map_err(Into::into)
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		self.access
			.decide(resource, action)
			.await
			.map_err(Into::into)
	}
}

fn workspace_filter(workspaces: &[Uuid]) -> SimpleExpr {
	Expr::col(Alias::new("workspace_id")).is_in(workspaces.iter().copied().map(Expr::value))
}

fn before(at: DateTime<Utc>, id: Uuid) -> Condition {
	Condition::any()
		.add(Expr::col(Alias::new("created_at")).lt(Expr::value(at)))
		.add(
			Condition::all()
				.add(Expr::col(Alias::new("created_at")).eq(Expr::value(at)))
				.add(Expr::col(Alias::new("id")).lt(Expr::value(id))),
		)
}
