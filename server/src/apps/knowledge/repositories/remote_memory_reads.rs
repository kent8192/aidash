//! Remote dependency journals use canonical Unit revisions, never disposable index generations.
use super::{access::Lease, units};
use crate::{Error, Result, authorization::access::Access, database::native, store::Store};
use aidash_domain::{
	memory::Recall,
	semantic::{
		Failure,
		remote::{Binding, NativeBinding, NativeContext},
	},
};
use reinhardt::query::{
	Alias, ColumnRef, Condition, Expr, ExprTrait, Func, LockType, OnConflict, Order,
	PostgresQueryBuilder, Query, QueryStatementBuilder,
};
use uuid::Uuid;
/// A grant can span Runs. Every admitted recall contributes at most max_results
/// reads and consumes a durable model operation in each declared bank.
pub(crate) async fn capacity(lease: &mut Lease<'_>, binding: &NativeBinding) -> Result<usize> {
	let mut capacity = 0_usize;
	for declared in &binding.banks {
		let policy =
			crate::semantic::native_memory::policy(lease, &declared.provider.entry).await?;
		// PostgreSQL COUNT cannot exceed i64::MAX. Saturation preserves otherwise
		// valid extreme policies without overflowing the aggregate allowance.
		capacity = capacity
			.saturating_add(
				policy
					.retention
					.max_model_operations
					.saturating_mul(policy.bounds.max_results),
			)
			.min(i64::MAX as usize);
	}
	Ok(capacity)
}

/// Receiver cache bodies are disposable; permanent operation identities and
/// budget/attempt receipts remain. Primary execution journals are separate.
pub(crate) async fn erase_receiver(store: &Store, run: &aidash_domain::RunMetadata) -> Result<()> {
	// Commit a content-free intent first. A failure during physical cleanup can
	// then be retried by maintenance, even after the Run has stopped executing.
	super::receiver_caches::invalidate(store, run.id).await?;
	super::receiver_caches::purge(store, run).await
}

pub(super) async fn erase_receiver_body(
	store: &Store,
	run: &aidash_domain::RunMetadata,
) -> Result<()> {
	let mut tx = native::begin(&store.pool).await?;
	let gate = super::receiver_caches::lock(&mut tx, run.id).await?;
	if gate.as_ref().is_none_or(|gate| gate.state == "purged") {
		return tx.rollback().await;
	}
	if gate.as_ref().is_some_and(|gate| gate.state == "failed") {
		return Err(Error::Conflict(
			"receiver cache cleanup allowance exhausted".into(),
		));
	}
	let description: Option<serde_json::Value> = native::query_scalar(
		&Query::select()
			.column(Alias::new("description"))
			.from(Alias::new("authorization_remote_admissions"))
			.and_where(Expr::col("id").eq(Expr::value(run.id)))
			.and_where(Expr::col("source_node").eq(run.home_node.as_str()))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut *tx)
	.await?;
	let Some(description) = description else {
		return tx.rollback().await;
	};
	let description: crate::authorization::remote::Description =
		serde_json::from_value(description)?;
	if description.semantic.native().is_none() {
		return tx.rollback().await;
	}
	if description.source_node != run.home_node || description.target_node != store.node_id {
		return Err(Error::Forbidden);
	}
	native::query(
		&Query::update()
			.table(Alias::new("semantic_remote_operations"))
			.value(Alias::new("receipt"), None::<serde_json::Value>)
			.value(Alias::new("state"), "INVALIDATED")
			.value(Alias::new("error"), "invalidated")
			.value_expr(Alias::new("fence"), Expr::col("fence").add(1_i64))
			.value(Alias::new("attempt_id"), None::<Uuid>)
			.value(
				Alias::new("lease_until"),
				None::<chrono::DateTime<chrono::Utc>>,
			)
			.value(
				Alias::new("next_attempt"),
				None::<chrono::DateTime<chrono::Utc>>,
			)
			.and_where(Expr::col("home_node").eq(run.home_node.as_str()))
			.and_where(Expr::col("grant_id").eq(Expr::value(description.grant_id)))
			.and_where(Expr::col("admission_id").eq(Expr::value(run.id)))
			.and_where(Expr::col("state").ne("INVALIDATED"))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	native::query(
		&Query::delete()
			.from_table(Alias::new("semantic_remote_receipts"))
			.and_where(Expr::col("run_id").eq(Expr::value(run.id)))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut *tx)
	.await?;
	super::receiver_caches::complete(&mut tx, run.id).await?;
	tx.commit().await
}

pub(crate) async fn erase_receipts(
	lease: &mut Lease<'_>,
	unit: &aidash_domain::memory::Unit,
	withdrawn: bool,
) -> Result<()> {
	let mut grants = Query::select();
	grants
		.distinct()
		.column(Alias::new("grant_id"))
		.from(Alias::new("memory_remote_reads"))
		.and_where(Expr::col("unit_id").eq(Expr::value(unit.id)));
	if !withdrawn {
		grants.and_where(Expr::col("revision").lt(unit.revision));
	}
	native::query(
		&Query::update()
			.table(Alias::new("semantic_remote_operations"))
			.value(Alias::new("receipt"), None::<serde_json::Value>)
			.value(Alias::new("state"), "INVALIDATED")
			.value(Alias::new("error"), "invalidated")
			.value_expr(Alias::new("fence"), Expr::col("fence").add(1_i64))
			.value(Alias::new("attempt_id"), None::<Uuid>)
			.value(
				Alias::new("lease_until"),
				None::<chrono::DateTime<chrono::Utc>>,
			)
			.value(
				Alias::new("next_attempt"),
				None::<chrono::DateTime<chrono::Utc>>,
			)
			.and_where(Expr::col("grant_id").in_subquery(grants))
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **lease.tx())
	.await?;
	Ok(())
}

pub(crate) async fn record(
	tx: &mut native::Transaction,
	grant: Uuid,
	binding: &NativeBinding,
	context: &NativeContext,
	capacity: usize,
) -> Result<()> {
	native::query(
		&Query::insert()
			.into_table(Alias::new("memory_remote_read_gates"))
			.columns([Alias::new("grant_id")])
			.from_subquery(Query::select().expr(Expr::value(grant)).to_owned())
			.on_conflict(
				OnConflict::column(Alias::new("grant_id"))
					.do_nothing()
					.to_owned(),
			)
			.to_string(PostgresQueryBuilder),
	)
	.execute(&mut **tx)
	.await?;
	native::query(
		&Query::select()
			.column(Alias::new("grant_id"))
			.from(Alias::new("memory_remote_read_gates"))
			.and_where(Expr::col("grant_id").eq(Expr::value(grant)))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut **tx)
	.await?;
	for item in &context.banks {
		if !binding
			.banks
			.iter()
			.any(|declared| declared.bank == item.bank && declared.provider.entry == item.provider)
		{
			return Err(Error::Forbidden);
		}
		if let Recall::Ready { units } = &item.recall {
			for unit in units {
				if unit.bank != item.bank || !unit.visible() {
					return Err(Error::Forbidden);
				}
				native::query(
					&Query::insert()
						.into_table(Alias::new("memory_remote_reads"))
						.columns(["grant_id", "unit_id", "revision"].map(Alias::new))
						.from_subquery(
							Query::select()
								.expr(Expr::value(grant))
								.expr(Expr::value(unit.id))
								.expr(Expr::value(unit.revision))
								.to_owned(),
						)
						.on_conflict(
							OnConflict::columns(
								["grant_id", "unit_id", "revision"].map(Alias::new),
							)
							.do_nothing()
							.to_owned(),
						)
						.to_string(PostgresQueryBuilder),
				)
				.execute(&mut **tx)
				.await?;
			}
		}
	}
	let count: i64 = native::query_scalar(
		&Query::select()
			.expr(Func::count(Expr::col(ColumnRef::Asterisk).into()))
			.from(Alias::new("memory_remote_reads"))
			.and_where(Expr::col("grant_id").eq(Expr::value(grant)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut **tx)
	.await?;
	if usize::try_from(count)
		.ok()
		.is_none_or(|count| count > capacity)
	{
		return Err(Error::Conflict(
			"remote memory dependency limit reached".into(),
		));
	}
	Ok(())
}

pub(crate) async fn visible(access: &mut Access, grant: Uuid) -> Result<()> {
	let Some(_visit) = access.authority_read_visit("native-memory-grant", grant) else {
		return Err(Error::RemoteSemantic(Failure::Invalidated));
	};
	let semantic: Option<serde_json::Value> = native::query_scalar(
		&Query::select()
			.column(Alias::new("semantic"))
			.from(Alias::new("authorization_remote_grants"))
			.and_where(Expr::col("id").eq(Expr::value(grant)))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.scalar_optional(&mut **access.tx)
	.await?;
	let semantic: Binding = serde_json::from_value(semantic.ok_or(Error::Forbidden)?)?;
	let home = access.node_id.clone();
	let mut lease = Lease::Inherited(access);
	let Some(binding) = semantic.native() else {
		let read = native::query(
			&Query::select()
				.column(Alias::new("unit_id"))
				.from(Alias::new("memory_remote_reads"))
				.and_where(Expr::col("grant_id").eq(Expr::value(grant)))
				.limit(1)
				.to_string(PostgresQueryBuilder),
		)
		.fetch_optional(&mut **lease.tx())
		.await?;
		return if read.is_none() {
			Ok(())
		} else {
			Err(Error::RemoteSemantic(Failure::Invalidated))
		};
	};
	let capacity = capacity(&mut lease, binding).await?;

	if crate::apps::knowledge::services::remote_memory::bind(
		&home,
		&mut lease,
		binding.participant.bank.workspace,
		&binding.selection,
		binding.generation.as_ref(),
	)
	.await? != *binding
	{
		return Err(Error::RemoteSemantic(Failure::Invalidated));
	}
	// Binding validation holds the current bank/provider/catalog locks for this
	// lease. Read each pinned policy once instead of repeating role admission for
	// every lifetime read; per-unit authority, TTL and support checks remain live.
	let mut policies = Vec::new();
	for declared in &binding.banks {
		// Unconfigured empty banks are valid declarations, but they cannot
		// supply a Unit dependency without their canonical policy settings.
		let Some(settings) = super::bank_settings::get(&mut lease, &declared.bank).await? else {
			continue;
		};
		if settings.provider != declared.provider.entry {
			return Err(Error::RemoteSemantic(Failure::Invalidated));
		}
		policies.push((
			declared.bank.clone(),
			crate::semantic::native_memory::policy(&mut lease, &declared.provider.entry).await?,
		));
	}
	// Check every accumulated revision in bounded pages, including later Runs.
	let mut cursor: Option<(Uuid, i64)> = None;
	let mut count = 0_usize;
	loop {
		let mut query = Query::select();
		query
			.columns(["unit_id", "revision"].map(Alias::new))
			.from(Alias::new("memory_remote_reads"))
			.and_where(Expr::col("grant_id").eq(Expr::value(grant)))
			.order_by(Alias::new("unit_id"), Order::Asc)
			.order_by(Alias::new("revision"), Order::Asc)
			.limit(256);
		if let Some((id, revision)) = cursor {
			query.cond_where(
				Condition::any()
					.add(Expr::col("unit_id").gt(Expr::value(id)))
					.add(
						Condition::all()
							.add(Expr::col("unit_id").eq(Expr::value(id)))
							.add(Expr::col("revision").gt(Expr::value(revision))),
					),
			);
		}
		let reads = native::query(&query.to_string(PostgresQueryBuilder))
			.fetch_all(&mut **lease.tx())
			.await?;
		if reads.is_empty() {
			return Ok(());
		}
		for read in reads {
			let id: Uuid = read.try_get("unit_id")?;
			let revision: i64 = read.try_get("revision")?;
			cursor = Some((id, revision));
			count += 1;
			if count > capacity {
				return Err(Error::RemoteSemantic(Failure::Invalidated));
			}
			let unit = units::load(&mut lease, id, false)
				.await?
				.ok_or(Error::RemoteSemantic(Failure::Invalidated))?;
			if !unit.visible() || unit.revision != revision {
				return Err(Error::RemoteSemantic(Failure::Invalidated));
			}
			let (_, policy) = policies
				.iter()
				.find(|(bank, _)| bank == &unit.bank)
				.ok_or(Error::RemoteSemantic(Failure::Invalidated))?;
			units::authorize(&mut lease, &unit.bank, "memory.read").await?;
			if policy
				.retention
				.unit_expired(unit.learned_at, chrono::Utc::now())
			{
				return Err(Error::Conflict("memory unit retention expired".into()));
			}
			// Match admission: the checked root is outside its content traversal.
			units::current(
				&mut lease,
				unit.bank.workspace,
				&unit.content.evidence,
				policy.bounds.max_graph_visits,
			)
			.await?;
		}
	}
}
