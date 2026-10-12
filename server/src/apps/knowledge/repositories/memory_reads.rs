//! Canonical source revisions are independent of disposable search generations.
use super::{access::Lease, bindings, units};
use crate::{Error, Result, database::native};
use aidash_domain::{
	RunMetadata,
	memory::{Evidence, Unit},
	semantic::Failure,
};
use reinhardt::query::{
	Alias, ColumnRef, Condition, Expr, ExprTrait, LockType, OnConflict, Order,
	PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use uuid::Uuid;

pub(crate) async fn record(lease: &mut Lease<'_>, run: Uuid, selected: &[Unit]) -> Result<()> {
	// The delivery transaction owns durability. A savepoint also prevents a
	// rejected operation leaking staged roots when an inherited caller recovers.
	let savepoint = format!("memory_reads_{}", Uuid::new_v4().simple());
	lease.tx().savepoint(&savepoint).await?;
	let result = record_in(lease, run, selected).await;
	if result.is_err() {
		lease.tx().rollback_to_savepoint(&savepoint).await?;
	}
	lease.tx().release_savepoint(&savepoint).await?;
	result
}

async fn record_in(lease: &mut Lease<'_>, run: Uuid, selected: &[Unit]) -> Result<()> {
	let metadata = crate::database::query_as::<RunMetadata>(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("runs"))
			.and_where(Expr::col("id").eq(Expr::value(run)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut **lease.tx())
	.await?;
	let binding = bindings::load(&mut **lease.tx(), &metadata)
		.await?
		.ok_or(Error::Forbidden)?;
	let policy = crate::semantic::native_memory::policy(lease, &binding.provider).await?;
	// Separate gate avoids upgrading the binding lock held by the calling scope.
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_run_read_gates"))
			.columns([Alias::new("run_id")])
			.from_subquery(Query::select().expr(Expr::value(run)).to_owned())
			.on_conflict(
				OnConflict::column(Alias::new("run_id"))
					.do_nothing()
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	native::query(
		&Query::select()
			.column(Alias::new("run_id"))
			.from(Alias::new("memory_run_read_gates"))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut **lease.tx())
	.await?;
	let mut ordered: Vec<_> = selected.iter().collect();
	ordered.sort_by_key(|unit| unit.id);
	for unit in ordered {
		let prior: Option<Uuid> = native::query_scalar(
			&Query::select()
				.column(Alias::new("unit_id"))
				.from(Alias::new("memory_run_reads"))
				.and_where(Expr::col("run_id").eq(Expr::value(run)))
				.and_where(Expr::col("unit_id").eq(Expr::value(unit.id)))
				.limit(1)
				.to_string(PostgresQueryBuilder),
		)
		.scalar_optional(&mut **lease.tx())
		.await?;
		let inserted = native::query(
			&Query::insert()
				.into_table(Alias::new("memory_run_reads"))
				.columns(["run_id", "unit_id", "revision"].map(Alias::new))
				.from_subquery(
					Query::select()
						.expr(Expr::value(run))
						.expr(Expr::value(unit.id))
						.expr(Expr::value(unit.revision))
						.to_owned(),
				)
				.on_conflict(
					OnConflict::columns([
						Alias::new("run_id"),
						Alias::new("unit_id"),
						Alias::new("revision"),
					])
					.do_nothing()
					.to_owned(),
				)
				.to_string(PostgresQueryBuilder),
		)
		.execute(&mut **lease.tx())
		.await?;
		if metadata.home_node == unit.bank.home {
			super::memory_decay::delivery(
				lease,
				unit,
				inserted.rows_affected() > 0 && prior.is_none(),
			)
			.await?;
		}
	}
	// A future Run proof also visits the Run itself. Automatic learning validates
	// that proof together with all canonical input/output evidence, whose finite
	// envelope includes the Run root. Reserve the entire admitted envelope so a
	// full journal cannot make an otherwise bounded learning input unprovable.
	let reserved = if policy.learn_from_runs {
		policy
			.bounds
			.max_evidence
			.min(policy.bounds.max_graph_visits)
	} else {
		1
	};
	let journal_visits = policy.bounds.max_graph_visits - reserved;
	let reads = native::query(
		&Query::select()
			.columns(["unit_id", "revision"].map(Alias::new))
			.from(Alias::new("memory_run_reads"))
			.and_where(Expr::col("run_id").eq(Expr::value(run)))
			.order_by(Alias::new("unit_id"), Order::Asc)
			.order_by(Alias::new("revision"), Order::Asc)
			.limit(journal_visits as u64 + 1)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut **lease.tx())
	.await?;
	if reads.len() > journal_visits {
		return Err(Error::Conflict(
			"Run memory read journal exceeds its provenance bound".into(),
		));
	}
	let mut evidence = Vec::with_capacity(reads.len());
	for read in reads {
		let unit = units::load(lease, read.try_get("unit_id")?, false)
			.await?
			.ok_or(Error::Forbidden)?;
		evidence.push(Evidence::Unit {
			bank: unit.bank,
			id: unit.id,
			revision: read.try_get("revision")?,
		});
	}
	match units::current(lease, metadata.workspace_id, &evidence, journal_visits).await {
		Err(Error::Invalid(message))
			if message == "memory evidence traversal exceeds its bound" =>
		{
			return Err(Error::Conflict(
				"Run memory read journal exceeds its provenance bound".into(),
			));
		}
		result => result?,
	}
	Ok(())
}

pub(crate) async fn visible(lease: &mut Lease<'_>, run_id: Uuid) -> Result<bool> {
	Ok(invalidity(lease, run_id).await?.is_none())
}

/// Why a Run's memory read journal no longer holds, or `None` while it does.
/// A changed, withdrawn, deleted or expired source is `Invalidated`; a changed
/// participant or provider is `Configuration`; lost access is `Authority`.
pub(crate) async fn invalidity(lease: &mut Lease<'_>, run_id: Uuid) -> Result<Option<Failure>> {
	// Error kinds of the shared helpers: `Conflict` reports a changed or removed
	// source, `Forbidden` lost authority.
	let source = |error: Error| match error {
		Error::Conflict(_) => Ok(Some(Failure::Invalidated)),
		Error::Forbidden => Ok(Some(Failure::Authority)),
		error => Err(error),
	};
	let binding_change = |error: Error| match error {
		Error::Conflict(_) => Ok(Some(Failure::Configuration)),
		Error::Forbidden => Ok(Some(Failure::Authority)),
		error => Err(error),
	};
	let Some(run) = crate::database::query_as::<RunMetadata>(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("runs"))
			.and_where(Expr::col("id").eq(Expr::value(run_id)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?
	else {
		return Ok(Some(Failure::Authority));
	};
	let binding = match bindings::load(&mut **lease.tx(), &run).await {
		Ok(binding) => binding,
		Err(error) => return binding_change(error),
	};
	let Some(binding) = binding else {
		let read = native::query(
			&Query::select()
				.column(Alias::new("unit_id"))
				.from(Alias::new("memory_run_reads"))
				.and_where(Expr::col("run_id").eq(Expr::value(run_id)))
				.limit(1)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **lease.tx())
		.await?;
		return Ok(read.map(|_| Failure::Authority));
	};
	if let Err(error) = units::authorize(lease, &binding.bank, "memory.read").await {
		return binding_change(error);
	}
	let policy =
		match crate::apps::knowledge::services::native_memory::policy(lease, &binding.provider)
			.await
		{
			Ok(policy) => policy,
			Err(error) => return binding_change(error),
		};
	// The journal spans the whole Run. Each bounded page still validates every
	// delivered revision; accumulated dependencies must never be sampled away.
	let page_size = policy.bounds.max_graph_visits.min(256) as u64;
	let mut cursor: Option<(Uuid, i64)> = None;
	loop {
		let mut query = Query::select();
		query
			.columns(["unit_id", "revision"].map(Alias::new))
			.from(Alias::new("memory_run_reads"))
			.and_where(Expr::col("run_id").eq(Expr::value(run_id)))
			.order_by(Alias::new("unit_id"), Order::Asc)
			.order_by(Alias::new("revision"), Order::Asc)
			.limit(page_size);
		if let Some((unit, revision)) = cursor {
			query.cond_where(
				Condition::any()
					.add(Expr::col("unit_id").gt(Expr::value(unit)))
					.add(
						Condition::all()
							.add(Expr::col("unit_id").eq(Expr::value(unit)))
							.add(Expr::col("revision").gt(Expr::value(revision))),
					),
			);
		}
		let reads = native::query(&query.to_string(PostgresQueryBuilder))
			.fetch_all(&mut **lease.tx())
			.await?;
		if reads.is_empty() {
			return Ok(None);
		}
		for read in reads {
			let id: Uuid = read.try_get("unit_id")?;
			let revision: i64 = read.try_get("revision")?;
			cursor = Some((id, revision));
			let Some(unit) = units::load(lease, id, false).await? else {
				return Ok(Some(Failure::Invalidated));
			};
			if unit.bank.workspace != run.workspace_id {
				return Ok(Some(Failure::Authority));
			}
			if !unit.visible() || unit.revision != revision {
				return Ok(Some(Failure::Invalidated));
			}
			let policy = match units::unexpired(lease, &unit).await {
				Ok(policy) => policy,
				Err(error) => return source(error),
			};
			if let Err(error) = units::current(
				lease,
				run.workspace_id,
				&[Evidence::Unit {
					bank: unit.bank.clone(),
					id: unit.id,
					revision: unit.revision,
				}],
				policy.bounds.max_graph_visits,
			)
			.await
			{
				return source(error);
			}
		}
	}
}

#[cfg(test)]
#[path = "memory_reads/tests.rs"]
mod tests;
