//! Only the Home's admission path issues or binds a logical participant to a Run.
use super::{access::Lease, native_memory as repository, units};
use crate::{Error, Result, database::native, store::Store};
use aidash_domain::{
	RunMetadata,
	memory::*,
	registry::{AgentConfig, EntityRef, Entry},
};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, LockType, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use uuid::Uuid;

pub(crate) async fn claimed(
	store: &Store,
	lease: &mut Lease<'_>,
	task: Uuid,
	agent: &Entry,
) -> Result<Option<Binding>> {
	let run: Option<aidash_domain::Run> = crate::database::query_as(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("runs"))
			.and_where(Expr::col("home_node").eq(store.node_id.as_str()))
			.and_where(Expr::col("task_id").eq(Expr::value(task)))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?;
	let Some(run) = run else {
		return Ok(None);
	};
	admit(store, lease, &run.metadata(), agent).await
}

pub(crate) async fn load(
	tx: &mut dyn reinhardt::db::backends::TransactionExecutor,
	run: &RunMetadata,
) -> Result<Option<Binding>> {
	let row = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_run_bindings"))
			.and_where(Expr::col("run_id").eq(Expr::value(run.id)))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut *tx)
	.await?;
	let Some(row) = row else {
		return Ok(None);
	};
	let participant: Uuid = row.try_get("participant_id")?;
	let p = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_participants"))
			.and_where(Expr::col("id").eq(Expr::value(participant)))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut *tx)
	.await?
	.ok_or(Error::Forbidden)?;
	let revision: i64 = row.try_get("participant_revision")?;
	if p.try_get::<bool>("deleted")?
		|| p.try_get::<i64>("revision")? != revision
		|| p.try_get::<Uuid>("workspace_id")? != run.workspace_id
		|| p.try_get::<String>("home")? != run.home_node
		|| p.try_get::<String>("agent_id")? != run.agent_id
		|| p.try_get::<String>("agent_version")? != run.agent_version
	{
		return Err(Error::Conflict(
			"Run logical participant changed; memory context requires recovery".into(),
		));
	}
	Ok(Some(Binding {
		bank: Bank {
			home: p.try_get("home")?,
			tenant: p.try_get("tenant")?,
			workspace: run.workspace_id,
			participant: Some(participant),
		},
		participant_revision: revision,
		agent: EntityRef {
			id: run.agent_id.clone(),
			version: run.agent_version.clone(),
		},
		provider: EntityRef {
			id: row.try_get("provider_id")?,
			version: row.try_get("provider_version")?,
		},
	}))
}
pub(crate) async fn admit(
	store: &Store,
	lease: &mut Lease<'_>,
	run: &RunMetadata,
	agent: &Entry,
) -> Result<Option<Binding>> {
	let config: AgentConfig = serde_json::from_value(agent.config.clone())?;
	let Some(provider) = config.memory else {
		return Ok(None);
	};
	if run.home_node != store.node_id
		|| agent.id != run.agent_id
		|| agent.version != run.agent_version
	{
		return Err(Error::Forbidden);
	}
	if let Some(binding) = load(&mut **lease.tx(), run).await? {
		if binding.provider != provider {
			return Err(Error::Conflict("Run memory provider changed".into()));
		}
		return Ok(Some(binding));
	}
	crate::semantic::native_memory::policy(lease, &provider).await?;
	let tenant: String = native::query_scalar(
		&Query::select()
			.column(Alias::new("tenant"))
			.from(Alias::new("authorization_workspaces"))
			.and_where(Expr::col("workspace_id").eq(Expr::value(run.workspace_id)))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut **lease.tx())
	.await?
	.ok_or(Error::Forbidden)?;
	let assigned = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_task_participants"))
			.and_where(Expr::col("task_id").eq(Expr::value(run.task_id)))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?;
	let (participant, revision) = if let Some(assignment) = assigned {
		let id: Uuid = assignment.try_get("participant_id")?;
		let observed: i64 = assignment.try_get("participant_revision")?;
		let bank = Bank {
			home: store.node_id.clone(),
			tenant: tenant.clone(),
			workspace: run.workspace_id,
			participant: Some(id),
		};
		units::authorize(lease, &bank, "memory.participant.use").await?;
		let p = native::query(
			&Query::select()
				.columns(["revision", "agent_id", "agent_version"].map(Alias::new))
				.from(Alias::new("memory_participants"))
				.and_where(Expr::col("id").eq(Expr::value(id)))
				.lock(LockType::Share)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(&mut **lease.tx())
		.await?;
		if p.try_get::<i64>("revision")? != observed
			|| p.try_get::<String>("agent_id")? != agent.id
			|| p.try_get::<String>("agent_version")? != agent.version
		{
			return Err(Error::Conflict(
				"assigned logical participant revision or Agent version changed".into(),
			));
		}
		(id, observed)
	} else {
		let id = Uuid::now_v7();
		let principal = lease.saved()?["subject"]
			.as_str()
			.ok_or(Error::Forbidden)?
			.to_owned();
		native::query(
			&Query::insert()
				.into_table(Alias::new("memory_participants"))
				.columns(
					[
						"id",
						"home",
						"tenant",
						"workspace_id",
						"principal",
						"agent_id",
						"agent_version",
						"revision",
						"deleted",
					]
					.map(Alias::new),
				)
				.from_subquery(
					Query::select()
						.expr(Expr::value(id))
						.expr(Expr::value(&store.node_id))
						.expr(Expr::value(&tenant))
						.expr(Expr::value(run.workspace_id))
						.expr(Expr::value(principal))
						.expr(Expr::value(&agent.id))
						.expr(Expr::value(&agent.version))
						.expr(Expr::value(1_i64))
						.expr(Expr::value(false))
						.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **lease.tx())
		.await?;
		(id, 1)
	};
	let binding = Binding {
		bank: Bank {
			home: store.node_id.clone(),
			tenant,
			workspace: run.workspace_id,
			participant: Some(participant),
		},
		participant_revision: revision,
		agent: EntityRef {
			id: agent.id.clone(),
			version: agent.version.clone(),
		},
		provider,
	};
	repository::bank_id(lease, &binding.bank, true).await?;
	super::bank_settings::ensure(lease, &binding.bank, &binding.provider).await?;
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_run_bindings"))
			.columns(
				[
					"run_id",
					"participant_id",
					"provider_id",
					"provider_version",
					"participant_revision",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::value(run.id))
					.expr(Expr::value(participant))
					.expr(Expr::value(&binding.provider.id))
					.expr(Expr::value(&binding.provider.version))
					.expr(Expr::value(revision))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	Ok(Some(binding))
}
