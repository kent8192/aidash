//! Queue surviving admitted support and retire superseded automatic observations.
use super::{Input, Lease, enqueue, repository, units};
use crate::{Error, Result, database::native};
use aidash_domain::{memory::*, registry::EntityRef};
use reinhardt::query::{
	Alias, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use std::collections::BTreeSet;
use uuid::Uuid;

pub(super) async fn repairs(
	lease: &mut Lease<'_>,
	bank: &Bank,
	provider: &EntityRef,
	policy: &Policy,
	changed: &[Unit],
	origin_run: Option<Uuid>,
) -> Result<()> {
	let affected: BTreeSet<_> = changed
		.iter()
		.filter(|unit| !unit.content.kind.derived())
		.map(|unit| unit.id)
		.collect();
	if !policy.maintain_observations || affected.is_empty() {
		return Ok(());
	}
	let bank_id = repository::bank_id(lease, bank, false)
		.await?
		.ok_or(Error::Forbidden)?;
	let rows: Vec<Uuid> = native::query_scalar(
		&Query::select()
			.column(Alias::new("id"))
			.from(Alias::new("memory_units"))
			.and_where(Expr::col("bank_id").eq(Expr::value(bank_id)))
			.and_where(Expr::col("kind").eq("observation"))
			.and_where(Expr::col("stale").eq(true))
			.and_where(Expr::col("deleted").eq(false))
			.limit(policy.bounds.max_units as u64 + 1)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_all(&mut **lease.tx())
	.await?;
	if rows.len() > policy.bounds.max_units {
		return Err(Error::Conflict(
			"observation repair exceeds its allowance".into(),
		));
	}
	let mut sources = BTreeSet::new();
	for id in rows {
		let observation = units::load(lease, id, false)
			.await?
			.ok_or(Error::Forbidden)?;
		let evidence: Vec<Uuid> = observation
			.content
			.evidence
			.iter()
			.filter_map(|proof| match proof {
				Evidence::Unit {
					id,
					bank: source_bank,
					..
				} if source_bank == bank => Some(*id),
				_ => None,
			})
			.collect();
		let automatic = evidence.iter().any(|source| {
			crate::semantic::native_memory::request_id(*source, "observation-unit")
				.is_ok_and(|target| target == id)
		});
		if automatic && evidence.iter().any(|id| affected.contains(id)) {
			sources.extend(evidence);
		}
	}
	if sources.len() > policy.bounds.max_graph_visits {
		return Err(Error::Conflict(
			"complete observation repair exceeds its allowance".into(),
		));
	}
	let trigger = serde_json::to_string(&changed.iter().map(Unit::evidence).collect::<Vec<_>>())?;
	for id in sources {
		let Some(source) = units::load(lease, id, false).await? else {
			continue;
		};
		if source.bank != *bank || !source.visible() || source.content.kind.derived() {
			continue;
		}
		let id = crate::semantic::native_memory::request_id(
			source.id,
			&format!("observation-repair:{}:{trigger}", source.revision),
		)?;
		enqueue(
			lease,
			bank,
			provider,
			policy,
			id,
			Input::Observation {
				source: source.evidence(),
				origin_run,
			},
			"pending",
			None,
		)
		.await?;
	}
	Ok(())
}

pub(super) async fn retire(
	lease: &mut Lease<'_>,
	bank: &Bank,
	provider: &EntityRef,
	policy: &Policy,
	job: Uuid,
	target: Uuid,
	sources: &[Unit],
) -> Result<()> {
	let ids: BTreeSet<_> = sources.iter().map(|source| source.id).collect();
	let mut changes = vec![];
	for source in sources {
		let id = crate::semantic::native_memory::request_id(source.id, "observation-unit")?;
		if id == target {
			continue;
		}
		if let Some(unit) = units::load(lease, id, false).await?
			&& unit.bank == *bank && !unit.deleted && unit.content.kind == Kind::Observation && !unit.content.evidence.is_empty()
				&& unit.content.evidence.iter().all(|proof| matches!(proof, Evidence::Unit { bank: source_bank, id, .. } if source_bank == bank && ids.contains(id))) {
				changes.push(Change::Delete { id, expected_revision: unit.revision });
			}
	}
	if !changes.is_empty() {
		repository::mutate(
			lease,
			&Mutation {
				operation_id: crate::semantic::native_memory::request_id(
					job,
					"consolidation-retirement",
				)?,
				provider: provider.clone(),
				bank: bank.clone(),
				changes,
			},
			&policy.bounds,
		)
		.await?;
	}
	Ok(())
}
