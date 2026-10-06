//! Remote dependency journals use canonical Unit revisions, never disposable index generations.
use super::{access::Lease, units};
use crate::{Error, Result, authorization::access::Access, database::native, store::Store};
use aidash_domain::{
	memory::{Evidence, Recall},
	semantic::{
		Failure,
		remote::{Binding, NativeBinding, NativeContext},
	},
};
use reinhardt::query::{
	Alias, Expr, ExprTrait, LockType, OnConflict, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
use uuid::Uuid;
const MAX_READS: usize = 1024;

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
	store: &Store,
	grant: Uuid,
	binding: &NativeBinding,
	context: &NativeContext,
) -> Result<()> {
	let mut tx = native::begin(&store.control_pool).await?;
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
	.execute(&mut *tx)
	.await?;
	native::query(
		&Query::select()
			.column(Alias::new("grant_id"))
			.from(Alias::new("memory_remote_read_gates"))
			.and_where(Expr::col("grant_id").eq(Expr::value(grant)))
			.lock(LockType::Update)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut *tx)
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
				.execute(&mut *tx)
				.await?;
			}
		}
	}
	let count = native::query(
		&Query::select()
			.column(Alias::new("unit_id"))
			.from(Alias::new("memory_remote_reads"))
			.and_where(Expr::col("grant_id").eq(Expr::value(grant)))
			.limit(MAX_READS as u64 + 1)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut *tx)
	.await?
	.len();
	if count > MAX_READS {
		return Err(Error::Conflict(
			"remote memory dependency limit reached".into(),
		));
	}
	tx.commit().await
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
	let reads = native::query(
		&Query::select()
			.columns(["unit_id", "revision"].map(Alias::new))
			.from(Alias::new("memory_remote_reads"))
			.and_where(Expr::col("grant_id").eq(Expr::value(grant)))
			.order_by(Alias::new("unit_id"), Order::Asc)
			.order_by(Alias::new("revision"), Order::Asc)
			.limit(MAX_READS as u64 + 1)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut **lease.tx())
	.await?;
	let Some(binding) = semantic.native() else {
		if reads.is_empty() {
			return Ok(());
		}
		return Err(Error::RemoteSemantic(Failure::Invalidated));
	};

	if crate::apps::knowledge::services::remote_memory::bind(
		&home,
		&mut lease,
		binding.participant.bank.workspace,
		&binding.selection,
		binding.generation.as_ref(),
	)
	.await? != *binding
		|| reads.len() > MAX_READS
	{
		return Err(Error::RemoteSemantic(Failure::Invalidated));
	}
	for read in reads {
		let unit = units::load(&mut lease, read.try_get("unit_id")?, false)
			.await?
			.ok_or(Error::RemoteSemantic(Failure::Invalidated))?;
		if !unit.visible()
			|| unit.revision != read.try_get::<i64>("revision")?
			|| !binding
				.banks
				.iter()
				.any(|declared| declared.bank == unit.bank)
		{
			return Err(Error::RemoteSemantic(Failure::Invalidated));
		}
		units::current(
			&mut lease,
			unit.bank.workspace,
			&[Evidence::Unit {
				bank: unit.bank.clone(),
				id: unit.id,
				revision: unit.revision,
			}],
			MAX_READS,
		)
		.await?;
	}
	Ok(())
}
