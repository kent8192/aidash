//! Home-owned bounded engine jobs. Claims, attempts and outcomes survive process cuts.
//! Inputs contain exact evidence identities, never a second copy of memory bodies.
use super::{access::Lease, native_memory as repository, units};
use crate::{Error, Result, database::native, store::Store};
use aidash_domain::{memory::*, registry::EntityRef};
use chrono::{Duration, Utc};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, LockBehavior, LockType, OnConflict, Order,
	PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
mod completion;
mod consolidation;
mod discovery;
pub(crate) use completion::completion_ready;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Input {
	Learn {
		run: Uuid,
		revision: i64,
	},
	Observation {
		source: Evidence,
		origin_run: Option<Uuid>,
	},
	MentalModel {
		target: Uuid,
		revision: i64,
		sources: Vec<Evidence>,
		origin_run: Option<Uuid>,
	},
}
impl Input {
	fn origin_run(&self) -> Option<Uuid> {
		match self {
			Self::Learn { run, .. } => Some(*run),
			Self::Observation { origin_run, .. } | Self::MentalModel { origin_run, .. } => {
				*origin_run
			}
		}
	}
	fn kind(&self) -> &'static str {
		match self {
			Self::Learn { .. } => "learn",
			Self::Observation { .. } => "observation",
			Self::MentalModel { .. } => "mental_model",
		}
	}
}

#[allow(clippy::too_many_arguments)] // Persist independent scope, policy, receipt, input and outcome fields.
async fn enqueue(
	lease: &mut Lease<'_>,
	bank: &Bank,
	provider: &EntityRef,
	policy: &Policy,
	id: Uuid,
	input: Input,
	state: &str,
	error: Option<&str>,
) -> Result<()> {
	let bank_id = repository::bank_id(lease, bank, false)
		.await?
		.ok_or(Error::Forbidden)?;
	if let Some(existing) = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_engine_jobs"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?
	{
		if existing.try_get::<Uuid>("bank_id")? != bank_id
			|| existing.try_get::<String>("provider_id")? != provider.id
			|| existing.try_get::<String>("provider_version")? != provider.version
		{
			return Err(Error::Conflict("engine job scope changed".into()));
		}
		let previous: Input = serde_json::from_value(existing.try_get("input")?)?;
		if matches!(input, Input::MentalModel { .. })
			&& previous != input
			&& existing.try_get::<String>("state")? != "complete"
		{
			// Revoke any obsolete claim. A source revision changes the work identity
			// even when the already-stale question's revision stays the same.
			native::query(
				&Query::update()
					.table(Alias::new("memory_engine_jobs"))
					.value(Alias::new("input"), serde_json::to_value(&input)?)
					.value(Alias::new("authority"), lease.saved()?)
					.value(Alias::new("state"), state)
					.value(Alias::new("claim"), None::<Uuid>)
					.value(Alias::new("attempts"), 0_i32)
					.value(Alias::new("next_attempt"), Utc::now())
					.value(Alias::new("last_error"), error)
					.value(Alias::new("updated_at"), Utc::now())
					.and_where(Expr::col("id").eq(Expr::value(id)))
					.to_string(PostgresQueryBuilder),
			)
			.execute(&mut **lease.tx())
			.await?;
		}
		return Ok(());
	}
	repository::record_capacity(
		lease,
		bank_id,
		"memory_engine_jobs",
		policy.retention.max_model_operations,
	)
	.await?;
	let now = Utc::now();
	let authority = lease.saved()?;
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_engine_jobs"))
			.columns(
				[
					"id",
					"bank_id",
					"provider_id",
					"provider_version",
					"kind",
					"input",
					"authority",
					"state",
					"attempts",
					"claim",
					"next_attempt",
					"last_error",
					"created_at",
					"updated_at",
				]
				.map(Alias::new),
			)
			.from_subquery(
				Query::select()
					.expr(Expr::value(id))
					.expr(Expr::value(bank_id))
					.expr(Expr::value(&provider.id))
					.expr(Expr::value(&provider.version))
					.expr(Expr::value(input.kind()))
					.expr(Expr::value(serde_json::to_value(input)?))
					.expr(Expr::value(authority))
					.expr(Expr::value(state))
					.expr(Expr::value(0_i32))
					.expr(Expr::value(None::<Uuid>))
					.expr(Expr::value(now))
					.expr(Expr::value(error))
					.expr(Expr::value(now))
					.expr(Expr::value(now))
					.to_owned(),
			)
			.on_conflict(OnConflict::column(Alias::new("id")).do_nothing().to_owned())
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	Ok(())
}

/// Admission/correction schedules only maintenance over already-admitted source units.
/// Derived results may refresh dependent questions, but never propose new Run knowledge.
pub(crate) async fn changed(
	lease: &mut Lease<'_>,
	bank: &Bank,
	provider: &EntityRef,
	policy: &Policy,
	changed: &[Unit],
	origin_run: Option<Uuid>,
) -> Result<()> {
	for unit in changed {
		if policy.maintain_observations
			&& !unit.deleted
			&& !unit.stale
			&& !unit.content.kind.derived()
		{
			let id = crate::semantic::native_memory::request_id(
				unit.id,
				&format!("observation:{}", unit.revision),
			)?;
			enqueue(
				lease,
				bank,
				provider,
				policy,
				id,
				Input::Observation {
					source: unit.evidence(),
					origin_run,
				},
				"pending",
				None,
			)
			.await?;
		}
	}
	consolidation::repairs(lease, bank, provider, policy, changed, origin_run).await?;
	if !policy.refresh_mental_models {
		return Ok(());
	}
	let bank_id = repository::bank_id(lease, bank, false)
		.await?
		.ok_or(Error::Forbidden)?;
	let rows = native::query(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("memory_units"))
			.and_where(Expr::col("bank_id").eq(Expr::value(bank_id)))
			.and_where(Expr::col("kind").eq("mental_model"))
			.and_where(Expr::col("deleted").eq(false))
			.and_where(Expr::col("stale").eq(true))
			.order_by(Alias::new("id"), Order::Asc)
			.limit(policy.bounds.max_units as u64 + 1)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut **lease.tx())
	.await?;
	if rows.len() > policy.bounds.max_units {
		return Err(Error::Conflict(
			"mental model queue exceeds bank capacity".into(),
		));
	}
	for row in rows {
		let model = units::load(lease, row.try_get("id")?, false)
			.await?
			.ok_or(Error::Forbidden)?;
		if model
			.content
			.mental_model
			.as_ref()
			.is_none_or(|m| !m.automatic_refresh)
		{
			continue;
		}
		let mut sources = Vec::new();
		let mut eligible = true;
		for evidence in &model.content.evidence {
			let Evidence::Unit { id, .. } = evidence else {
				eligible = false;
				break;
			};
			let Some(source) = units::load(lease, *id, false).await? else {
				eligible = false;
				break;
			};
			if source.bank != *bank || !source.visible() {
				eligible = false;
				break;
			}
			sources.push(source.evidence());
		}
		if eligible && !sources.is_empty() {
			let id = crate::semantic::native_memory::request_id(
				model.id,
				&format!("mental-model:{}", model.revision),
			)?;
			enqueue(
				lease,
				bank,
				provider,
				policy,
				id,
				Input::MentalModel {
					target: model.id,
					revision: model.revision,
					sources,
					origin_run,
				},
				"pending",
				None,
			)
			.await?;
		}
	}
	Ok(())
}

async fn schedule_learning(store: &Store) -> Result<()> {
	let rows = discovery::page(store).await?;
	for run_id in rows {
		schedule_run(store, run_id).await?;
	}
	Ok(())
}

async fn schedule_run(store: &Store, run_id: Uuid) -> Result<Option<Uuid>> {
	let run: aidash_domain::Run = crate::database::query_as(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("runs"))
			.and_where(Expr::col("id").eq(Expr::value(run_id)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&store.pool)
	.await?;
	let access =
		crate::authorization::execution::access_for_run(store, &run.metadata(), true).await;
	let mut lease = match access {
		Ok(Some(access)) => Lease::Scoped(Box::new(access)),
		Ok(None) => Lease::begin(store, &crate::authorization::identity::Actor::Operator).await?,
		Err(Error::Forbidden | Error::Conflict(_)) => return Ok(None),
		Err(e) => return Err(e),
	};
	let result = async {
		repository::lock_workspace(&mut lease, run.workspace_id, true).await?;
		let binding = super::bindings::load(&mut **lease.tx(), &run.metadata())
			.await?
			.ok_or(Error::Forbidden)?;
		let policy = crate::semantic::native_memory::policy(&mut lease, &binding.provider).await?;
		let enabled = policy.learn_from_runs;
		enqueue(
			&mut lease,
			&binding.bank,
			&binding.provider,
			&policy,
			run.id,
			Input::Learn {
				run: run.id,
				revision: run.revision,
			},
			if enabled { "pending" } else { "blocked" },
			(!enabled).then_some("learning_disabled_at_completion"),
		)
		.await?;
		if enabled {
			repository::bank_id(&mut lease, &binding.bank, false).await
		} else {
			Ok(None)
		}
	}
	.await;
	match lease.finish(result).await {
		Ok(bank) => Ok(bank),
		Err(Error::Forbidden | Error::Conflict(_)) => Ok(None),
		Err(e) => Err(e),
	}
}

/// Safe to run from both the janitor and deterministic recovery tests.
pub(crate) async fn sweep(store: &Store) -> Result<usize> {
	schedule_learning(store).await?;
	let rows = native::query(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("memory_engine_jobs"))
			.and_where(Expr::col("state").is_in(["pending", "running"]))
			.and_where(Expr::col("next_attempt").lte(Expr::value(Utc::now())))
			.order_by(Alias::new("next_attempt"), Order::Asc)
			.limit(32)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&store.pool)
	.await?;
	let mut count = 0;
	for row in rows {
		if process(store, row.try_get("id")?).await? {
			count += 1;
		}
	}
	Ok(count)
}

async fn process(store: &Store, id: Uuid) -> Result<bool> {
	let mut claim_tx = native::begin(&store.pool).await?;
	let Some(row) = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_engine_jobs"))
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.and_where(Expr::col("state").is_in(["pending", "running"]))
			.and_where(Expr::col("next_attempt").lte(Expr::value(Utc::now())))
			.lock(LockType::Update)
			.lock_behavior(LockBehavior::SkipLocked)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut *claim_tx)
	.await?
	else {
		return Ok(false);
	};
	let input: Input = row.try_get("input")?;
	let provider = EntityRef {
		id: row.try_get("provider_id")?,
		version: row.try_get("provider_version")?,
	};
	let bank_row = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_banks"))
			.and_where(Expr::col("id").eq(Expr::value(row.try_get::<Uuid>("bank_id")?)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut *claim_tx)
	.await?;
	let bank = Bank {
		home: bank_row.try_get("home")?,
		tenant: bank_row.try_get("tenant")?,
		workspace: bank_row.try_get("workspace_id")?,
		participant: bank_row.try_get("participant_id")?,
	};
	let entry = crate::apps::registry::models::Definition::read_in(
		&mut *claim_tx,
		&provider.id,
		&provider.version,
	)
	.await?;
	let policy: ProviderConfig = serde_json::from_value(entry.config)?;
	policy.policy.validate()?;
	let limit = policy.policy.bounds.max_retries;
	let attempts = row.try_get::<i32>("attempts")? as usize;
	if attempts >= limit {
		native::query(
			&Query::update()
				.table(Alias::new("memory_engine_jobs"))
				.value(Alias::new("state"), "failed")
				.value(Alias::new("last_error"), "retry_limit_exhausted")
				.and_where(Expr::col("id").eq(Expr::value(id)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut *claim_tx)
		.await?;
		claim_tx.commit().await?;
		return Ok(false);
	}
	let claim = Uuid::now_v7();
	// Several bounded model calls may be needed; an abandoned claim becomes due again.
	let timeout = i64::try_from(policy.policy.bounds.max_model_calls)
		.unwrap()
		.saturating_mul(i64::from(policy.policy.bounds.max_call_seconds))
		.saturating_add(30);
	native::query(
		&Query::update()
			.table(Alias::new("memory_engine_jobs"))
			.value(Alias::new("state"), "running")
			.value(Alias::new("claim"), claim)
			.value(Alias::new("attempts"), (attempts + 1) as i32)
			.value(
				Alias::new("next_attempt"),
				Utc::now() + Duration::seconds(timeout),
			)
			.value(Alias::new("updated_at"), Utc::now())
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *claim_tx)
	.await?;
	claim_tx.commit().await?;
	let outcome = execute(
		store,
		&bank,
		&provider,
		id,
		claim,
		&input,
		row.try_get("authority")?,
	)
	.await;
	let backpressure = matches!(&outcome, Err(Error::Conflict(message)) if message == super::candidates::QUEUE_FULL);
	let (state, error) = match &outcome {
		Ok(()) => ("complete", None),
		Err(Error::Conflict(_)) if backpressure => ("pending", Some("candidate_queue_full")),
		Err(Error::Forbidden | Error::Conflict(_)) => {
			("blocked", Some("authority_or_source_changed"))
		}
		Err(Error::Invalid(_)) => ("failed", Some("invalid_or_exhausted_work")),
		Err(_) => (
			if attempts + 1 >= limit {
				"failed"
			} else {
				"pending"
			},
			Some("engine_unavailable"),
		),
	};
	let mut tx = native::begin(&store.pool).await?;
	native::query(
		&Query::update()
			.table(Alias::new("memory_engine_jobs"))
			.value(Alias::new("state"), state)
			.value(Alias::new("last_error"), error)
			// Waiting for human review does not consume a model failure attempt.
			.value(Alias::new("attempts"), (attempts + usize::from(!backpressure)) as i32)
			.value(
				Alias::new("next_attempt"),
				Utc::now() + Duration::seconds(30),
			)
			.value(Alias::new("updated_at"), Utc::now())
			.and_where(Expr::col("id").eq(Expr::value(id)))
			.and_where(Expr::col("claim").eq(Expr::value(claim)))
			.and_where(Expr::col("state").eq("running"))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	tx.commit().await?;
	Ok(outcome.is_ok())
}

async fn execute(
	store: &Store,
	bank: &Bank,
	provider: &EntityRef,
	id: Uuid,
	claim: Uuid,
	input: &Input,
	authority: serde_json::Value,
) -> Result<()> {
	let mut lease = Lease::restore(store, authority).await?;
	let result = async {
		repository::lock_workspace(&mut lease, bank.workspace, true).await?;
		// Hold the exact claim through admission, so an expired worker cannot publish.
		let active = native::query(
			&Query::select()
				.column(Alias::new("id"))
				.from(Alias::new("memory_engine_jobs"))
				.and_where(Expr::col("id").eq(Expr::value(id)))
				.and_where(Expr::col("claim").eq(Expr::value(claim)))
				.and_where(Expr::col("state").eq("running"))
				.lock(LockType::Update)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **lease.tx())
		.await?;
		if active.is_none() {
			return Err(Error::Conflict("engine claim was replaced".into()));
		}
		crate::semantic::native_memory::bank_provider(&mut lease, bank, provider).await?;
		let policy = crate::semantic::native_memory::policy(&mut lease, provider).await?;
		let digest =
			aidash_domain::semantic::indexing::content_digest(&serde_json::to_string(input)?);
		let operation = crate::semantic::native_memory::request_id(
			id,
			&if matches!(input, Input::MentalModel { .. }) {
				format!("engine-operation:{digest}")
			} else {
				"engine-operation".into()
			},
		)?;
		let run_id = input.origin_run();
		if let Some(run) = run_id {
			let record: aidash_domain::Run = crate::database::query_as(
				&Query::select()
					.column(ColumnRef::Asterisk)
					.from(Alias::new("runs"))
					.and_where(Expr::col("id").eq(Expr::value(run)))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(&mut **lease.tx())
			.await?;
			if let Some(access) = lease.access() {
				crate::authorization::execution::inherit_run_authority(access, &record).await?;
			}
		}
		// Run learning and maintenance triggered by Run writes retain the origin's
		// authority and model allowance in addition to the finite bank policy.
		let models = if run_id.is_some() {
			crate::semantic::services::memory_models::Models::resolve(
				store,
				&mut lease,
				provider.clone(),
				bank.clone(),
				policy.clone(),
				operation,
				digest,
				run_id,
			)
			.await?
		} else {
			crate::semantic::services::memory_models::Models::resolve_index(
				store,
				&mut lease,
				provider.clone(),
				bank.clone(),
				policy.clone(),
				operation,
				digest,
			)
			.await?
		};
		let engine = aidash_application::memory::Engine {
			provider,
			policy: &policy,
			models: &models,
		};
		let mut scope = super::memory_scope::Scope {
			store,
			lease: &mut lease,
			models: &models,
			delivered: &mut Vec::new(),
		};
		match input {
			Input::Learn { run, revision } => {
				if !policy.learn_from_runs {
					return Err(Error::Forbidden);
				}
				let record: aidash_domain::Run = crate::database::query_as(
					&Query::select()
						.column(ColumnRef::Asterisk)
						.from(Alias::new("runs"))
						.and_where(Expr::col("id").eq(Expr::value(*run)))
						.lock(LockType::Share)
						.to_string(PostgresQueryBuilder),
				)
				.fetch_one(&mut **scope.lease.tx())
				.await?;
				if record.home_node != store.node_id
					|| record.workspace_id != bank.workspace
					|| record.revision != *revision
					|| !matches!(
						record.state,
						aidash_domain::run_state::RunState::Completed(_)
					) || record.error.is_some()
				{
					return Err(Error::Conflict("Run is not eligible for learning".into()));
				}
				let proof = Evidence::Run {
					id: *run,
					revision: *revision,
					digest: aidash_domain::semantic::indexing::content_digest(
						&serde_json::to_string(&record)?,
					),
				};
				let (text, evidence) =
					super::learning::input(scope.lease, bank, &proof, &policy.bounds).await?;
				engine
					.learn(&mut scope, bank, &proof, &text, &evidence)
					.await?;
			}
			Input::Observation { source, .. } => {
				if !policy.maintain_observations {
					return Err(Error::Forbidden);
				}
				let Evidence::Unit {
					id: source_id,
					revision,
					..
				} = source
				else {
					return Err(Error::Invalid("observation source".into()));
				};
				let source_unit = units::load(scope.lease, *source_id, false)
					.await?
					.ok_or(Error::Forbidden)?;
				if source_unit.bank != *bank
					|| source_unit.revision != *revision
					|| !source_unit.visible()
					|| source_unit.content.kind.derived()
				{
					return Err(Error::Conflict("observation source changed".into()));
				}
				let snapshot = repository::list(
					scope.lease,
					bank,
					policy.bounds.max_units,
					policy.bounds.max_graph_visits,
				)
				.await?;
				let consolidated = engine
					.consolidate(&mut scope, &source_unit, &snapshot)
					.await?;
				let sources = &consolidated.units;
				let target =
					crate::semantic::native_memory::request_id(sources[0].id, "observation-unit")?;
				let existing = units::load(scope.lease, target, false).await?;
				if existing.as_ref().is_some_and(|unit| {
					unit.visible()
						&& unit.content.kind == Kind::Observation
						&& unit
							.content
							.evidence
							.iter()
							.collect::<std::collections::BTreeSet<_>>()
							== sources
								.iter()
								.map(Unit::evidence)
								.collect::<Vec<_>>()
								.iter()
								.collect()
				}) {
					// Another bounded job already consolidated this exact source group.
					// A crash/replay or duplicate trigger cannot synthesize it again.
					consolidation::retire(
						scope.lease,
						bank,
						provider,
						&policy,
						id,
						target,
						sources,
					)
					.await?;
					return Ok(());
				}
				let mut placeholder = source_unit.content.clone();
				placeholder.kind = Kind::Observation;
				placeholder.verification = Verification::Unverified;
				placeholder.evidence = sources.iter().map(Unit::evidence).collect();
				placeholder.links.clear();
				let change = match existing {
					Some(unit) if !unit.deleted && unit.content.kind == Kind::Observation => {
						Change::Correct {
							id: target,
							expected_revision: unit.revision,
							content: placeholder,
						}
					}
					None => Change::Add {
						id: target,
						content: placeholder,
					},
					_ => return Err(Error::Conflict("observation was deleted".into())),
				};
				engine
					.maintain_consolidated(
						&mut scope,
						&Mutation {
							operation_id: operation,
							provider: provider.clone(),
							bank: bank.clone(),
							changes: vec![change],
						},
						&consolidated,
					)
					.await?;
				consolidation::retire(scope.lease, bank, provider, &policy, id, target, sources)
					.await?;
			}
			Input::MentalModel {
				target,
				revision,
				sources,
				..
			} => {
				if !policy.refresh_mental_models {
					return Err(Error::Forbidden);
				}
				let target_unit = units::load(scope.lease, *target, false)
					.await?
					.ok_or(Error::Forbidden)?;
				if target_unit.bank != *bank
					|| target_unit.revision != *revision
					|| target_unit.deleted
					|| target_unit
						.content
						.mental_model
						.as_ref()
						.is_none_or(|m| !m.automatic_refresh)
				{
					return Err(Error::Conflict("recurring question changed".into()));
				}
				let mut current = Vec::new();
				for source in sources {
					let Evidence::Unit { id, revision, .. } = source else {
						return Err(Error::Invalid("recurring question source".into()));
					};
					let unit = units::load(scope.lease, *id, false)
						.await?
						.ok_or(Error::Forbidden)?;
					if unit.bank != *bank || unit.revision != *revision {
						return Err(Error::Conflict("question source changed".into()));
					}
					current.push(unit);
				}
				engine
					.maintain(
						&mut scope,
						&Mutation {
							operation_id: operation,
							provider: provider.clone(),
							bank: bank.clone(),
							changes: vec![Change::Correct {
								id: *target,
								expected_revision: *revision,
								content: target_unit.content,
							}],
						},
						Kind::MentalModel,
						&current,
					)
					.await?;
			}
		}
		// Claim outcome and publication commit together; a crash after commit is idempotent.
		native::query(
			&Query::update()
				.table(Alias::new("memory_engine_jobs"))
				.value(Alias::new("state"), "complete")
				.value(Alias::new("updated_at"), Utc::now())
				.and_where(Expr::col("id").eq(Expr::value(id)))
				.and_where(Expr::col("claim").eq(Expr::value(claim)))
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **scope.lease.tx())
		.await?;
		Ok(())
	}
	.await;
	lease.finish(result).await
}
