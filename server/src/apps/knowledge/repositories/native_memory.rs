//! Unit changes, exact-request receipts and dependent fences share one authority transaction.
use super::{access::Lease, units};
use crate::{Error, Result, database::native};
use aidash_domain::memory::*;
use chrono::{SubsecRound, Utc};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, Func, LockType, OnConflict, Order, PostgresQueryBuilder,
	Query, QueryStatementBuilder, SimpleExpr,
};
use std::collections::{BTreeSet, VecDeque};

mod dependencies;
pub(crate) use dependencies::validate_impact;
use uuid::Uuid;

pub(crate) async fn lock_workspace(
	lease: &mut Lease<'_>,
	workspace: Uuid,
	exclusive: bool,
) -> Result<()> {
	let found = native::query(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("workspaces"))
			.and_where(Expr::col("id").eq(Expr::value(workspace)))
			.lock(if exclusive {
				LockType::Update
			} else {
				LockType::Share
			})
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?;
	if found.is_none() {
		return Err(Error::Forbidden);
	}
	Ok(())
}

pub(crate) async fn bank_id(
	lease: &mut Lease<'_>,
	bank: &Bank,
	create: bool,
) -> Result<Option<Uuid>> {
	bank.validate()?;
	let mut select = Query::select();
	select
		.column(Alias::new("id"))
		.from(Alias::new("memory_banks"))
		.and_where(Expr::col("home").eq(bank.home.as_str()))
		.and_where(Expr::col("tenant").eq(bank.tenant.as_str()))
		.and_where(Expr::col("workspace_id").eq(Expr::value(bank.workspace)));
	match bank.participant {
		Some(id) => {
			select.and_where(Expr::col("participant_id").eq(Expr::value(id)));
		}
		None => {
			select.and_where(Expr::col("participant_id").is_null());
		}
	}
	let found: Option<Uuid> = native::query_scalar(&select.to_string(PostgresQueryBuilder))
		.scalar_optional(&mut **lease.tx())
		.await?;
	if found.is_some() || !create {
		return Ok(found);
	}
	let id = Uuid::now_v7();
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_banks"))
			.columns(
				[
					"id",
					"home",
					"tenant",
					"workspace_id",
					"participant_id",
					"revision",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::value(id))
					.expr(Expr::value(&bank.home))
					.expr(Expr::value(&bank.tenant))
					.expr(Expr::value(bank.workspace))
					.expr(Expr::value(bank.participant))
					.expr(Expr::value(1_i64))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	Ok(Some(id))
}

/// Exact receipts contain identities/revisions, not a second copy of memory bodies.
pub(crate) async fn mutate(
	lease: &mut Lease<'_>,
	mutation: &Mutation,
	bounds: &Bounds,
) -> Result<Vec<Unit>> {
	let origin = lease.access().and_then(|access| access.read_run);
	mutate_origin(lease, mutation, bounds, origin).await
}
pub(crate) async fn mutate_origin(
	lease: &mut Lease<'_>,
	mutation: &Mutation,
	bounds: &Bounds,
	origin: Option<Uuid>,
) -> Result<Vec<Unit>> {
	mutation.validate_bounds(bounds)?;
	lock_workspace(lease, mutation.bank.workspace, true).await?;
	units::authorize(lease, &mutation.bank, "memory.write").await?;
	let bank_id = bank_id(lease, &mutation.bank, true)
		.await?
		.ok_or(Error::Forbidden)?;
	super::bank_settings::ensure(lease, &mutation.bank, &mutation.provider).await?;
	let digest =
		aidash_domain::semantic::indexing::content_digest(&serde_json::to_string(mutation)?);
	let receipt = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_receipts"))
			.and_where(Expr::col("operation_id").eq(Expr::value(mutation.operation_id)))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?;
	if let Some(receipt) = receipt {
		if receipt.try_get::<Uuid>("bank_id")? != bank_id
			|| receipt.try_get::<String>("digest")? != digest
		{
			return Err(Error::Conflict(
				"memory operation ID was reused with a different request".into(),
			));
		}
		let outcome: Vec<Evidence> = receipt.try_get("outcome")?;
		let mut result = Vec::new();
		for evidence in outcome {
			let Evidence::Unit { bank, id, revision } = evidence else {
				return Err(Error::Invalid("corrupt memory receipt".into()));
			};
			let unit = units::load(lease, id, false)
				.await?
				.ok_or(Error::Forbidden)?;
			// A receipt proves the operation; it cannot authorize delivery of an obsolete body.
			if unit.bank != bank || unit.revision != revision || unit.stale {
				return Err(Error::Conflict(
					"memory operation completed; its result has since changed".into(),
				));
			}
			if !unit.deleted {
				units::unexpired(lease, &unit).await?;
				units::current(
					lease,
					bank.workspace,
					&unit.content.evidence,
					bounds.max_graph_visits,
				)
				.await?;
			}
			result.push(unit);
		}
		return Ok(result);
	}
	let policy = crate::semantic::native_memory::policy(lease, &mutation.provider).await?;
	let record_count: i64 = native::query_scalar(
		&Query::select()
			.expr(Func::count(Expr::col(ColumnRef::Asterisk).into()))
			.from(Alias::new("memory_units"))
			.and_where(Expr::col("bank_id").eq(Expr::value(bank_id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut **lease.tx())
	.await?;
	let additions = mutation
		.changes
		.iter()
		.filter(|change| matches!(change, Change::Add { .. }))
		.count();
	if usize::try_from(record_count)
		.ok()
		.and_then(|n| n.checked_add(additions))
		.is_none_or(|n| n > policy.retention.max_unit_records)
	{
		return Err(Error::Conflict(
			"memory unit and deletion identity capacity reached".into(),
		));
	}
	let operations: i64 = native::query_scalar(
		&Query::select()
			.expr(Func::count(Expr::col(ColumnRef::Asterisk).into()))
			.from(Alias::new("memory_receipts"))
			.and_where(Expr::col("bank_id").eq(Expr::value(bank_id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut **lease.tx())
	.await?;
	if operations >= policy.retention.max_model_operations as i64 {
		return Err(Error::Conflict(
			"memory mutation record capacity reached".into(),
		));
	}
	let actor = lease.saved()?["subject"]
		.as_str()
		.ok_or(Error::Forbidden)?
		.to_owned();
	let count: i64 = native::query_scalar(
		&Query::select()
			.expr(Func::count(Expr::col(ColumnRef::Asterisk).into()))
			.from(Alias::new("memory_units"))
			.and_where(Expr::col("bank_id").eq(Expr::value(bank_id)))
			.and_where(Expr::col("deleted").eq(false))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut **lease.tx())
	.await?;
	let mut retained =
		usize::try_from(count).map_err(|_| Error::Invalid("corrupt memory count".into()))?;
	let mut result = Vec::new();
	// PostgreSQL's timestamp input rounds nanoseconds. Canonicalize before
	// admission so the pending fence, returned unit and persisted row agree.
	let now = Utc::now().trunc_subsecs(6);
	for change in &mutation.changes {
		let id = change.id();
		let existing = units::load(lease, id, true).await?;
		let mut unit = match change {
			Change::Add { content, .. } => {
				if existing.is_some() {
					return Err(Error::Conflict("memory unit ID already exists".into()));
				}
				retained = retained
					.checked_add(1)
					.ok_or_else(|| Error::Invalid("memory storage count overflow".into()))?;
				Unit {
					id,
					bank: mutation.bank.clone(),
					revision: 1,
					content: content.clone(),
					learned_at: now,
					updated_at: now,
					deleted: false,
					stale: false,
				}
			}
			Change::Correct {
				expected_revision, ..
			}
			| Change::Delete {
				expected_revision, ..
			} => {
				let mut unit =
					existing.ok_or_else(|| Error::Conflict("memory unit does not exist".into()))?;
				if unit.bank != mutation.bank || unit.revision != *expected_revision || unit.deleted
				{
					return Err(Error::Conflict(
						"memory unit revision changed or was deleted".into(),
					));
				}
				unit.revision = unit
					.revision
					.checked_add(1)
					.filter(|v| *v < i64::MAX)
					.ok_or_else(|| Error::Invalid("memory unit revision exhausted".into()))?;
				unit.updated_at = now;
				unit.stale = false;
				match change {
					Change::Correct { content, .. } => unit.content = content.clone(),
					Change::Delete { .. } => {
						unit.deleted = true;
						unit.content.text.clear();
						unit.content.mental_model = None;
						unit.content.entities.clear();
						unit.content.links.clear();
						retained = retained
							.checked_sub(1)
							.ok_or_else(|| Error::Invalid("corrupt memory count".into()))?;
					}
					_ => unreachable!(),
				}
				unit
			}
		};
		if let Some(range) = unit.content.occurred.as_mut() {
			range.start = range.start.trunc_subsecs(6);
			range.end = range.end.trunc_subsecs(6);
		}
		if !unit.deleted {
			units::unexpired(lease, &unit).await?;
			units::current(
				lease,
				unit.bank.workspace,
				&unit.content.evidence,
				bounds.max_graph_visits,
			)
			.await?;
			for link in &unit.content.links {
				if mutation.changes.iter().any(|change| {
					matches!(change,
					Change::Add { id, content } if *id == link.target && *id != unit.id
						&& link.revision == 1 && content.verification != Verification::Contradicted)
				}) {
					// Batch-local causal targets are admitted by this same atomic
					// request. A failed target CAS rolls back every source and edge.
					units::authorize(lease, &unit.bank, "memory.read").await?;
					continue;
				}
				let target = units::load(lease, link.target, false)
					.await?
					.ok_or_else(|| Error::Conflict("memory graph target removed".into()))?;
				if target.bank != unit.bank
					|| target.id == unit.id
					|| target.revision != link.revision
					|| !target.visible()
				{
					return Err(Error::Conflict(
						"memory graph target changed or is outside this bank".into(),
					));
				}
				units::authorize(lease, &target.bank, "memory.read").await?;
				units::unexpired(lease, &target).await?;
			}
			for evidence in &unit.content.evidence {
				if matches!(evidence, Evidence::Unit { id: source, .. } if *source == id) {
					return Err(Error::Invalid("a memory unit cannot support itself".into()));
				}
				if let Evidence::Unit { bank: source, .. } = evidence
					&& source != &unit.bank
				{
					return Err(Error::Forbidden);
				}
			}
		}
		save_origin(
			lease,
			bank_id,
			&unit,
			mutation.operation_id,
			&actor,
			Some(&mutation.provider),
			origin,
		)
		.await?;
		if matches!(change, Change::Correct { .. }) {
			super::memory_decay::reactivate(lease, unit.id).await?;
		}
		if unit.content.kind.derived() && !unit.deleted {
			for source in &unit.content.evidence {
				if let Evidence::Unit { id, .. } = source {
					super::memory_decay::reactivate(lease, *id).await?;
				}
			}
		}
		if unit.deleted {
			super::purge::schedule(lease, bank_id, &unit, &policy.retention).await?;
		}
		super::purge::history(lease, &unit, &policy.retention).await?;
		result.push(unit);
	}
	// Capacity applies to the atomic final state, including batched replacements.
	if retained > bounds.max_units {
		return Err(Error::Conflict("memory bank storage limit reached".into()));
	}
	// Admission must leave every source correctable within its pinned impact cap.
	for unit in result.iter().filter(|unit| !unit.deleted) {
		dependencies::validate_admission(lease, unit, bounds).await?;
	}
	// Impact discovery is bounded and complete before any result may leave the transaction.
	fence_dependents(
		lease,
		mutation.bank.workspace,
		&result,
		mutation.operation_id,
		&actor,
		bounds,
	)
	.await?;
	for unit in &result {
		if !unit.deleted {
			units::current(
				lease,
				unit.bank.workspace,
				&unit.content.evidence,
				bounds.max_graph_visits,
			)
			.await?;
		}
	}
	bump_bank(lease, bank_id).await?;
	super::engine_jobs::changed(
		lease,
		&mutation.bank,
		&mutation.provider,
		&policy,
		&result,
		origin,
	)
	.await?;
	let outcome: Vec<_> = result.iter().map(Unit::evidence).collect();
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_receipts"))
			.columns(["operation_id", "bank_id", "digest", "outcome", "created_at"].map(Alias::new))
			.from_subquery(
				Query::select()
					.expr(Expr::value(mutation.operation_id))
					.expr(Expr::value(bank_id))
					.expr(Expr::value(digest))
					.expr(Expr::value(serde_json::to_value(outcome)?))
					.expr(Expr::value(now))
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	Ok(result)
}

pub(crate) fn content_columns() -> Vec<Alias> {
	[
		"mental_model",
		"text",
		"kind",
		"learning",
		"verification",
		"occurred_start",
		"occurred_end",
		"entities",
		"evidence",
		"links",
	]
	.map(Alias::new)
	.to_vec()
}
fn label<T: serde::Serialize>(value: T) -> Result<String> {
	serde_json::to_value(value)?
		.as_str()
		.map(str::to_owned)
		.ok_or_else(|| Error::Invalid("invalid memory state".into()))
}
pub(crate) fn content_values(content: &Content) -> Result<Vec<SimpleExpr>> {
	Ok(vec![
		Expr::value(
			content
				.mental_model
				.as_ref()
				.map(serde_json::to_value)
				.transpose()?,
		)
		.into(),
		Expr::value(&content.text).into(),
		Expr::value(label(content.kind)?).into(),
		Expr::value(label(content.learning)?).into(),
		Expr::value(label(content.verification)?).into(),
		Expr::value(content.occurred.as_ref().map(|v| v.start.trunc_subsecs(6))).into(),
		Expr::value(content.occurred.as_ref().map(|v| v.end.trunc_subsecs(6))).into(),
		Expr::value(serde_json::to_value(&content.entities)?).into(),
		Expr::value(serde_json::to_value(&content.evidence)?).into(),
		Expr::value(serde_json::to_value(&content.links)?).into(),
	])
}
pub(crate) async fn restore_body(lease: &mut Lease<'_>, bank_id: Uuid, unit: &Unit) -> Result<()> {
	let mut columns = ["id", "bank_id", "revision"].map(Alias::new).to_vec();
	columns.extend(content_columns());
	columns.extend(["learned_at", "updated_at", "deleted", "stale"].map(Alias::new));
	let mut select = Query::select();
	select
		.expr(Expr::value(unit.id))
		.expr(Expr::value(bank_id))
		.expr(Expr::value(unit.revision));
	for value in content_values(&unit.content)? {
		select.expr(value);
	}
	select
		.expr(Expr::value(unit.learned_at.trunc_subsecs(6)))
		.expr(Expr::value(unit.updated_at.trunc_subsecs(6)))
		.expr(Expr::value(unit.deleted))
		.expr(Expr::value(unit.stale));
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_units"))
			.columns(columns.clone())
			.from_subquery(select)
			.on_conflict(
				OnConflict::column(Alias::new("id"))
					.update_columns(columns.iter().skip(2).cloned())
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	Ok(())
}
pub(crate) async fn save(
	lease: &mut Lease<'_>,
	bank_id: Uuid,
	unit: &Unit,
	operation: Uuid,
	actor: &str,
	provider: Option<&aidash_domain::registry::EntityRef>,
) -> Result<()> {
	let origin = lease.access().and_then(|access| access.read_run);
	save_origin(lease, bank_id, unit, operation, actor, provider, origin).await
}
async fn save_origin(
	lease: &mut Lease<'_>,
	bank_id: Uuid,
	unit: &Unit,
	operation: Uuid,
	actor: &str,
	provider: Option<&aidash_domain::registry::EntityRef>,
	origin: Option<Uuid>,
) -> Result<()> {
	lease.tx().observe_memory(unit)?;
	let mut columns = ["id", "bank_id", "revision"].map(Alias::new).to_vec();
	columns.extend(content_columns());
	columns.extend(["learned_at", "updated_at", "deleted", "stale"].map(Alias::new));
	let mut values: Vec<SimpleExpr> = vec![
		Expr::value(unit.id).into(),
		Expr::value(bank_id).into(),
		Expr::value(unit.revision).into(),
	];
	values.extend(content_values(&unit.content)?);
	values.extend([
		Expr::value(unit.learned_at.trunc_subsecs(6)).into(),
		Expr::value(unit.updated_at.trunc_subsecs(6)).into(),
		Expr::value(unit.deleted).into(),
		Expr::value(unit.stale).into(),
	]);
	let mut select = Query::select();
	for value in &values {
		select.expr(value.clone());
	}
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_units"))
			.columns(columns.clone())
			.from_subquery(select.clone())
			.on_conflict(
				OnConflict::column(Alias::new("id"))
					.update_columns(columns.iter().skip(2).cloned())
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	columns[0] = Alias::new("unit_id");
	columns.extend(["operation_id", "actor"].map(Alias::new));
	select.expr(Expr::value(operation)).expr(Expr::value(actor));
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_history"))
			.columns(columns)
			.from_subquery(select)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	refresh_dependencies(lease, bank_id, unit).await?;
	let max_origins = if !unit.stale && !unit.deleted {
		Some(
			crate::semantic::native_memory::policy(lease, provider.ok_or(Error::Forbidden)?)
				.await?
				.bounds
				.max_graph_visits,
		)
	} else {
		None
	};
	super::unit_origins::record(lease, unit, origin, max_origins).await?;
	project(lease, unit, actor, provider).await?;
	Ok(())
}
pub(crate) async fn refresh_dependencies(
	lease: &mut Lease<'_>,
	bank_id: Uuid,
	unit: &Unit,
) -> Result<()> {
	native::query(
		&Query::delete()
			.from_table(Alias::new("memory_dependencies"))
			.and_where(Expr::col("unit_id").eq(Expr::value(unit.id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	if !unit.deleted {
		for evidence in unit.content.evidence.iter().collect::<BTreeSet<_>>() {
			let (kind, id, revision) = match evidence {
				Evidence::Publication { id, revision } => {
					let (source, revision) = if unit.stale {
						super::publications::retained_source(lease, *id).await?
					} else {
						super::publications::source(lease, *id, *revision).await?
					};
					("unit", source, revision)
				}
				Evidence::Unit { id, revision, .. } => ("unit", *id, *revision),
				Evidence::Message { id, revision, .. } => ("message", *id, *revision),
				Evidence::Artifact { id, revision, .. } => ("artifact", *id, *revision),
				Evidence::Run { id, revision, .. } => ("run", *id, *revision),
			};
			native::query(
				&Query::insert()
					.into_table(Alias::new("memory_dependencies"))
					.columns(
						[
							"unit_id",
							"bank_id",
							"source_kind",
							"source_id",
							"source_revision",
						]
						.map(Alias::new),
					)
					.from_subquery(
						Query::select()
							.expr(Expr::value(unit.id))
							.expr(Expr::value(bank_id))
							.expr(Expr::value(kind))
							.expr(Expr::value(id))
							.expr(Expr::value(revision))
							.to_owned(),
					)
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **lease.tx())
			.await?;
		}
	}
	Ok(())
}

/// Search is a disposable projection. Unit CAS above remains the canonical
/// mutation fence; an index-only revision never substitutes for a unit revision.
pub(crate) async fn project(
	lease: &mut Lease<'_>,
	unit: &Unit,
	actor: &str,
	provider: Option<&aidash_domain::registry::EntityRef>,
) -> Result<()> {
	use crate::apps::knowledge::services::core;
	use aidash_domain::semantic::{Source, mutations::Entry};
	let Some(index) = crate::apps::knowledge::models::SemanticIndexe::locked(
		&mut **lease.tx(),
		unit.bank.workspace,
		true,
	)
	.await?
	else {
		return Ok(());
	};
	let spec = index.configuration()?;
	let prior: Option<core::Entry> = native::query_as(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("semantic_entries"))
			.and_where(Expr::col("id").eq(Expr::value(unit.id)))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?;
	if let Some(prior) = &prior {
		if prior.workspace_id != unit.bank.workspace
			|| serde_json::from_value::<Source>(prior.source.clone())?
				!= (Source::Unit { id: unit.id })
		{
			return Err(Error::Conflict(
				"memory search projection identity is already in use".into(),
			));
		}
	} else if unit.visible() {
		let count: i64 = native::query_scalar(
			&Query::select()
				.expr(Func::count(Expr::col(ColumnRef::Asterisk).into()))
				.from(Alias::new("semantic_entries"))
				.and_where(Expr::col("workspace_id").eq(Expr::value(unit.bank.workspace)))
				.and_where(Expr::col("deleted").eq(false))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&mut **lease.tx())
		.await?;
		if count >= spec.max_sources as i64 {
			return Err(Error::Conflict(
				"Workspace search source limit reached".into(),
			));
		}
	}
	if unit.visible() {
		aidash_domain::semantic::indexing::validate_text(&unit.content.text, spec.max_input_bytes)?;
	}
	let revision = prior.as_ref().map_or(Ok(1), |old| {
		old.revision
			.checked_add(1)
			.filter(|v| *v < i64::MAX)
			.ok_or_else(|| Error::Invalid("memory projection revision exhausted".into()))
	})?;
	let state = if unit.deleted {
		"DELETED"
	} else if !unit.visible() {
		"REVOKED"
	} else {
		"PENDING"
	};
	let now = Utc::now();
	let origin = super::unit_origins::load(lease, unit.id)
		.await?
		.ok_or(Error::Forbidden)?;
	if origin.revision != unit.revision {
		return Err(Error::Forbidden);
	}
	let saved = serde_json::to_value(origin.authority)?;
	let provider = provider
		.map(serde_json::to_value)
		.transpose()?
		.or_else(|| {
			prior
				.as_ref()
				.and_then(|entry| entry.metadata.get("provider").cloned())
		})
		.ok_or_else(|| Error::Conflict("memory projection requires a pinned provider".into()))?;
	let entry = Entry {
		id: unit.id,
		workspace_id: unit.bank.workspace,
		key: format!("memory-unit:{}", unit.id),
		source: serde_json::to_value(Source::Unit { id: unit.id })?,
		agent: unit.bank.participant.map(|id| format!("participant:{id}")),
		metadata: serde_json::json!({"unit_revision":unit.revision,"bank":unit.bank,"provider":provider}),
		revision,
		point_id: Uuid::now_v7(),
		index_revision: index.revision,
		deleted: unit.deleted,
		state: state.into(),
		attempts: 0,
		last_error: None,
		created_by: prior
			.as_ref()
			.map_or(actor.to_owned(), |old| old.created_by.clone()),
		updated_at: now,
	};
	let values: Vec<(&str, SimpleExpr)> = vec![
		("id", Expr::value(entry.id).into()),
		("workspace_id", Expr::value(entry.workspace_id).into()),
		("key", Expr::value(&entry.key).into()),
		("source", Expr::value(entry.source.clone()).into()),
		("agent", Expr::value(entry.agent.clone()).into()),
		("metadata", Expr::value(entry.metadata.clone()).into()),
		("revision", Expr::value(entry.revision).into()),
		("point_id", Expr::value(entry.point_id).into()),
		("index_revision", Expr::value(entry.index_revision).into()),
		("deleted", Expr::value(entry.deleted).into()),
		("state", Expr::value(&entry.state).into()),
		("attempts", Expr::value(0_i32).into()),
		("last_error", Expr::value(Option::<String>::None).into()),
		("created_by", Expr::value(&entry.created_by).into()),
		("authority", Expr::value(saved).into()),
		("updated_at", Expr::value(now).into()),
		("next_attempt", Expr::value(now).into()),
	];
	let columns: Vec<_> = values.iter().map(|(name, _)| Alias::new(*name)).collect();
	let mut select = Query::select();
	for (_, value) in values {
		select.expr(value);
	}
	native::query(
		&Query::insert()
			.into_table(Alias::new("semantic_entries"))
			.columns(columns.clone())
			.from_subquery(select)
			.on_conflict(
				OnConflict::column(Alias::new("id"))
					.update_columns(columns.iter().skip(3).cloned())
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	let projection: core::Entry = entry.into();
	super::storage::schedule_point(lease.tx(), &projection, &index.collection).await?;
	super::storage::history(
		lease.tx(),
		unit.bank.workspace,
		Some(unit.id),
		revision,
		state,
		"native unit revision projected",
	)
	.await?;
	Ok(())
}
pub(crate) async fn bump_bank(lease: &mut Lease<'_>, id: Uuid) -> Result<()> {
	native::query(
		&Query::update()
			.table(Alias::new("memory_banks"))
			.value_expr(Alias::new("revision"), Expr::col("revision").add(1_i64))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	Ok(())
}
async fn fence_dependents(
	lease: &mut Lease<'_>,
	workspace: Uuid,
	changed: &[Unit],
	operation: Uuid,
	actor: &str,
	bounds: &Bounds,
) -> Result<()> {
	// Each changed root gets its declared traversal budget; a batch is bounded
	// by max_candidates * max_graph_visits rather than consuming one shared cap.
	let mut fenced: BTreeSet<_> = changed.iter().map(|unit| unit.id).collect();
	for root in changed {
		let mut pending = VecDeque::from([root.id]);
		let mut seen = BTreeSet::from([root.id]);
		while let Some(id) = pending.pop_front() {
			let rows = dependencies::dependents(lease, id, bounds.max_graph_visits).await?;
			let source = units::load(lease, id, false)
				.await?
				.ok_or(Error::Forbidden)?;
			for row in rows {
				if row.try_get::<i64>("source_revision")? == source.revision && source.visible() {
					continue;
				}
				let dependent: Uuid = row.try_get("unit_id")?;
				if !seen.insert(dependent) {
					continue;
				}
				if seen.len() > bounds.max_graph_visits {
					return Err(Error::Conflict(
						"memory change impact exceeds its declared bound".into(),
					));
				}
				if !fenced.insert(dependent) {
					continue;
				}
				let mut unit = units::load(lease, dependent, true)
					.await?
					.ok_or(Error::Forbidden)?;
				if unit.bank.workspace != workspace {
					return Err(Error::Forbidden);
				}
				unit.stale = true;
				unit.revision = unit
					.revision
					.checked_add(1)
					.filter(|revision| *revision < i64::MAX)
					.ok_or(Error::Forbidden)?;
				unit.updated_at = Utc::now();
				let bank = bank_id(lease, &unit.bank, false)
					.await?
					.ok_or(Error::Forbidden)?;
				save(lease, bank, &unit, operation, actor, None).await?;
				bump_bank(lease, bank).await?;
				native::query(
					&Query::update()
						.table(Alias::new("memory_bank_settings"))
						.value(Alias::new("next_maintenance"), Utc::now())
						.and_where(Expr::col("bank_id").eq(Expr::value(bank)))
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut **lease.tx())
				.await?;
				pending.push_back(unit.id);
			}
		}
	}
	Ok(())
}

pub(crate) async fn list(
	lease: &mut Lease<'_>,
	bank: &Bank,
	limit: usize,
	max_graph_visits: usize,
) -> Result<Vec<Unit>> {
	list_mode(lease, bank, limit, max_graph_visits, ListMode::All).await
}

#[derive(Clone, Copy)]
pub(crate) enum ListMode {
	All,
	Recall,
	Dormant,
}

pub(crate) async fn list_mode(
	lease: &mut Lease<'_>,
	bank: &Bank,
	limit: usize,
	max_graph_visits: usize,
	mode: ListMode,
) -> Result<Vec<Unit>> {
	lock_workspace(lease, bank.workspace, false).await?;
	units::authorize(lease, bank, "memory.read").await?;
	if matches!(mode, ListMode::Dormant) {
		units::authorize(lease, bank, "memory.read_dormant").await?;
	}
	let Some(bank_id) = bank_id(lease, bank, false).await? else {
		return Ok(Vec::new());
	};
	let mut query = Query::select();
	query
		.column(Alias::new("id"))
		.from(Alias::new("memory_units"))
		.and_where(Expr::col("bank_id").eq(Expr::value(bank_id)))
		.and_where(Expr::col("deleted").eq(false))
		.and_where(Expr::col("stale").eq(false))
		.and_where(Expr::col("verification").ne("contradicted"))
		.order_by(Alias::new("id"), Order::Asc)
		.limit(limit as u64 + 1);
	if !matches!(mode, ListMode::All) {
		let settings = super::bank_settings::get(lease, bank)
			.await?
			.ok_or(Error::Forbidden)?;
		let policy = crate::semantic::native_memory::policy(lease, &settings.provider).await?;
		if policy.decay.as_ref().is_some_and(|d| d.dormancy.is_some()) {
			let dormant = super::memory_decay::dormant_filter(bank_id, &settings.provider)?;
			if matches!(mode, ListMode::Dormant) {
				query.and_where(Expr::col("id").in_subquery(dormant));
			} else {
				query.and_where(Expr::col("id").not_in_subquery(dormant));
			}
		} else if matches!(mode, ListMode::Dormant) {
			return Ok(Vec::new());
		}
	}
	let ids: Vec<Uuid> = native::query_scalar(&query.to_string(PostgresQueryBuilder))
		.scalar_all(&mut **lease.tx())
		.await?;
	if ids.len() > limit {
		return Err(Error::Conflict(
			"memory snapshot exceeds its declared bound".into(),
		));
	}
	load_snapshot(lease, bank, &ids, max_graph_visits).await
}

async fn load_snapshot(
	lease: &mut Lease<'_>,
	bank: &Bank,
	ids: &[Uuid],
	max_graph_visits: usize,
) -> Result<Vec<Unit>> {
	let mut result = Vec::new();
	for &id in ids {
		let unit = units::load(lease, id, false)
			.await?
			.ok_or(Error::Forbidden)?;
		let current = async {
			units::unexpired(lease, &unit).await?;
			units::current(
				lease,
				bank.workspace,
				&unit.content.evidence,
				max_graph_visits,
			)
			.await
		}
		.await;
		match current {
			Err(Error::Conflict(_) | Error::Forbidden) => continue,
			result => result?,
		}
		result.push(unit);
	}
	Ok(result)
}

/// Classify both recall partitions in one PostgreSQL statement snapshot. Source
/// hydration may overlap retention changes, but cannot change captured membership.
pub(crate) async fn recall_including_dormant(
	lease: &mut Lease<'_>,
	bank: &Bank,
	limit: usize,
	max_graph_visits: usize,
) -> Result<(Vec<Unit>, Vec<Unit>)> {
	lock_workspace(lease, bank.workspace, false).await?;
	units::authorize(lease, bank, "memory.read").await?;
	units::authorize(lease, bank, "memory.read_dormant").await?;
	let Some(bank_id) = bank_id(lease, bank, false).await? else {
		return Ok((Vec::new(), Vec::new()));
	};
	let settings = super::bank_settings::get(lease, bank)
		.await?
		.ok_or(Error::Forbidden)?;
	let policy = crate::semantic::native_memory::policy(lease, &settings.provider).await?;
	let dormant = if policy
		.decay
		.as_ref()
		.is_some_and(|decay| decay.dormancy.is_some())
	{
		Expr::col("id").in_subquery(super::memory_decay::dormant_filter(
			bank_id,
			&settings.provider,
		)?)
	} else {
		Expr::value(false)
	};
	let total_limit = limit
		.checked_mul(2)
		.and_then(|value| value.checked_add(1))
		.ok_or_else(|| Error::Invalid("memory snapshot bound overflow".into()))?;
	let rows = native::query(
		&Query::select()
			.column(Alias::new("id"))
			.expr_as(dormant, Alias::new("dormant"))
			.from(Alias::new("memory_units"))
			.and_where(Expr::col("bank_id").eq(Expr::value(bank_id)))
			.and_where(Expr::col("deleted").eq(false))
			.and_where(Expr::col("stale").eq(false))
			.and_where(Expr::col("verification").ne("contradicted"))
			.order_by(Alias::new("id"), Order::Asc)
			.limit(total_limit as u64)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut **lease.tx())
	.await?;
	let mut active = Vec::new();
	let mut dormant = Vec::new();
	for row in rows {
		let partition = if row.try_get::<bool>("dormant")? {
			&mut dormant
		} else {
			&mut active
		};
		partition.push(row.try_get::<Uuid>("id")?);
		if partition.len() > limit {
			return Err(Error::Conflict(
				"memory snapshot exceeds its declared bound".into(),
			));
		}
	}
	Ok((
		load_snapshot(lease, bank, &active, max_graph_visits).await?,
		load_snapshot(lease, bank, &dormant, max_graph_visits).await?,
	))
}

pub(crate) async fn keyword(
	lease: &mut Lease<'_>,
	bank: &Bank,
	text: &str,
	allowed: &[Uuid],
	limit: usize,
) -> Result<Vec<Uuid>> {
	if allowed.is_empty() {
		return Ok(Vec::new());
	}
	let bank_id = bank_id(lease, bank, false).await?.ok_or(Error::Forbidden)?;
	let expression = SimpleExpr::CustomWithExpr(
		"? &@~ ?".into(),
		vec![Expr::col("text").into(), Expr::value(text).into()],
	);
	native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("memory_units"))
			.and_where(Expr::col("bank_id").eq(Expr::value(bank_id)))
			.and_where(Expr::col("id").is_in(allowed.iter().copied().map(Expr::value)))
			.and_where(expression)
			.and_where(Expr::col("deleted").eq(false))
			.and_where(Expr::col("stale").eq(false))
			.order_by_expr(Expr::cust("pgroonga_score(tableoid, ctid)"), Order::Desc)
			.order_by(Alias::new("id"), Order::Asc)
			.limit(limit as u64)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_all(&mut **lease.tx())
	.await
}

/// Negative identities and audit receipts have an explicit finite bank capacity.
pub(crate) async fn record_capacity(
	lease: &mut Lease<'_>,
	bank: uuid::Uuid,
	table: &str,
	limit: usize,
) -> Result<()> {
	let count: i64 = native::query_scalar(
		&Query::select()
			.expr(Func::count(Expr::col("bank_id").into()))
			.from(Alias::new(table))
			.and_where(Expr::col("bank_id").eq(Expr::value(bank)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut **lease.tx())
	.await?;
	if count >= limit as i64 {
		return Err(Error::Conflict(
			"memory audit record capacity reached".into(),
		));
	}
	Ok(())
}
