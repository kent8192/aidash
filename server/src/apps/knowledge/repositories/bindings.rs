//! Only the Home's admission path issues or binds a logical participant to a Run.
use super::{access::Lease, native_memory as repository, units};
use crate::{Error, Result, database::native, store::Store};
use aidash_domain::{
	RunMetadata,
	memory::*,
	registry::{AgentConfig, EntityRef, Entry},
};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, LockType, OnConflict, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
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
			.and_where(crate::database::scope::local_home(
				"runs",
				store.deployment.as_str(),
			))
			.and_where(Expr::col("task_id").eq(Expr::value(task)))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?;
	let Some(run) = run else {
		return Ok(None);
	};
	// Built-in memory is optional at Run activation. Its first tool or
	// background use admits it in that operation's own transaction.
	let implicit_without_snapshot = run.context.binding_snapshot.is_none()
		&& serde_json::from_value::<aidash_domain::registry::bindings::AgentBindings>(
			agent.config.clone(),
		)
		.is_ok_and(|settings| {
			!settings.bindings.iter().any(|binding| {
				binding.kind == aidash_domain::registry::bindings::BindingKind::Memory
			})
		});
	if uses_builtin(&run) || implicit_without_snapshot {
		return Ok(None);
	}
	admit(store, lease, &run.metadata(), agent).await
}

fn uses_builtin(run: &aidash_domain::Run) -> bool {
	run.context
		.binding_snapshot
		.as_ref()
		.is_some_and(|snapshot| {
			snapshot.bindings.iter().any(|binding| {
				binding.excluded_reason.is_none()
					&& binding.identity.id
						== aidash_domain::registry::bindings::memory_defaults::MEMORY_ID
					&& binding.identity.version
						== aidash_domain::registry::bindings::memory_defaults::VERSION
			})
		})
}

pub(crate) async fn for_run(
	store: &Store,
	lease: &mut Lease<'_>,
	run: &aidash_domain::Run,
) -> Result<Option<Binding>> {
	if let Some(binding) = load(&mut **lease.tx(), &run.metadata()).await? {
		return Ok(Some(binding));
	}
	if !uses_builtin(run) {
		return Ok(None);
	}
	let snapshot = run
		.context
		.binding_snapshot
		.as_ref()
		.ok_or(Error::Forbidden)?;
	let agent = snapshot
		.definitions
		.iter()
		.find(|definition| definition.identity == snapshot.agent)
		.ok_or(Error::Forbidden)?;
	admit(store, lease, &run.metadata(), &agent.definition).await
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
	let provider = EntityRef {
		id: row.try_get("provider_id")?,
		version: row.try_get("provider_version")?,
	};
	let builtin = provider.id == aidash_domain::registry::bindings::memory_defaults::MEMORY_ID
		&& provider.version == aidash_domain::registry::bindings::memory_defaults::VERSION;
	if p.try_get::<bool>("deleted")?
		|| p.try_get::<i64>("revision")? != revision
		|| p.try_get::<Uuid>("workspace_id")? != run.workspace_id
		|| p.try_get::<String>("home")? != run.home_node
		|| (!builtin
			&& (p.try_get::<String>("agent_id")? != run.agent_id
				|| p.try_get::<String>("agent_version")? != run.agent_version))
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
		provider,
	}))
}
pub(crate) async fn admit(
	store: &Store,
	lease: &mut Lease<'_>,
	run: &RunMetadata,
	agent: &Entry,
) -> Result<Option<Binding>> {
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
	let config = configuration(lease, &tenant, agent, store.deployment.as_str()).await?;
	let Some(provider) = config.memory else {
		return Ok(None);
	};
	let builtin = provider.id == aidash_domain::registry::bindings::memory_defaults::MEMORY_ID
		&& provider.version == aidash_domain::registry::bindings::memory_defaults::VERSION;
	if !run.is_local(store.deployment.as_str())
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
	crate::semantic::native_memory::policy(lease, &tenant, &provider).await?;
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
	let (participant, revision, reauthorize) = if let Some(assignment) = assigned {
		let id: Uuid = assignment.try_get("participant_id")?;
		let observed: i64 = assignment.try_get("participant_revision")?;
		let bank = Bank {
			home: run.home_node.clone(),
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
		(id, observed, false)
	} else {
		let id = if builtin {
			crate::semantic::native_memory::request_id(
				run.workspace_id,
				"builtin-memory-workspace",
			)?
		} else {
			Uuid::now_v7()
		};
		let principal = lease.saved()?["subject"]
			.as_str()
			.ok_or(Error::Forbidden)?
			.to_owned();
		let mut insert = Query::insert();
		insert
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
					.expr(Expr::value(run.home_node.as_str()))
					.expr(Expr::value(&tenant))
					.expr(Expr::value(run.workspace_id))
					.expr(Expr::value(principal))
					.expr(Expr::value(&agent.id))
					.expr(Expr::value(&agent.version))
					.expr(Expr::value(1_i64))
					.expr(Expr::value(false))
					.to_owned(),
			);
		if builtin {
			insert.on_conflict(OnConflict::column(Alias::new("id")).do_nothing().to_owned());
		}
		let created = native::query(&insert.to_string(PostgresQueryBuilder))
			.execute(&mut **lease.tx())
			.await?
			.rows_affected()
			!= 0;
		if !created {
			let prior = native::query(
				&Query::select()
					.column(ColumnRef::Asterisk)
					.from(Alias::new("memory_participants"))
					.and_where(Expr::col("id").eq(Expr::value(id)))
					.lock(LockType::Share)
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut **lease.tx())
			.await?;
			if prior.try_get::<bool>("deleted")?
				|| prior.try_get::<String>("home")? != run.home_node
				|| prior.try_get::<String>("tenant")? != tenant
				|| prior.try_get::<Uuid>("workspace_id")? != run.workspace_id
			{
				return Err(Error::Conflict(
					"built-in memory participant changed".into(),
				));
			}
			(id, prior.try_get("revision")?, true)
		} else {
			(id, 1, false)
		}
	};
	let binding = Binding {
		bank: Bank {
			home: run.home_node.clone(),
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
	if reauthorize {
		units::authorize(lease, &binding.bank, "memory.participant.use").await?;
	}
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
					"tenant",
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
					.expr(crate::database::scope::tenant_of("runs", "id", run.id))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	Ok(Some(binding))
}

/// Resolve only the explicitly bound immutable memory roles for participant APIs.
/// Execution admission independently pins and validates the complete Tool closure.
pub(crate) async fn configuration(
	lease: &mut Lease<'_>,
	tenant: &str,
	agent: &Entry,
	node: &str,
) -> Result<AgentConfig> {
	use aidash_domain::registry::bindings::BindingKind;
	let mut settings: AgentConfig = serde_json::from_value(agent.config.clone())?;
	settings.memory = None;
	for binding in &settings.bindings {
		let kind = match binding.kind {
			BindingKind::Memory => "memory",
			BindingKind::Source => "source",
			_ => continue,
		};
		if binding.target.registry_deployment != node {
			return Err(Error::Forbidden);
		}
		let entry = crate::semantic::native_memory::definition(
			lease,
			tenant,
			&binding.target.local(),
			kind,
		)
		.await?;
		if entry.config.get("schema_version").is_some() {
			continue;
		}
		if kind == "memory" {
			if settings.memory.replace(binding.target.local()).is_some() {
				return Err(Error::Invalid(
					"Agent requires one primary memory provider".into(),
				));
			}
		} else {
			settings.sources.push(binding.target.local());
		}
	}
	if settings.memory.is_none() {
		settings.memory =
			Some(aidash_domain::registry::bindings::memory_defaults::reference(node).local());
	}
	Ok(settings)
}
