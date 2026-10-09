//! Explicit index maintenance preserves canonical unit revisions and read dependencies.
use super::native_memory::{Operation, definition};
use crate::apps::knowledge::repositories::{
	access::Lease, candidates, memory_receipts, native_memory as repository, units,
};
use crate::{Error, Result, database::native};
use aidash_domain::{memory::*, semantic::EmbeddingConfig};
use chrono::Utc;
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, PostgresQueryBuilder, Query, QueryStatementBuilder,
};

pub(crate) async fn reindex(
	lease: &mut Lease<'_>,
	input: &Operation,
	policy: &Policy,
	expected: i64,
) -> Result<Vec<Unit>> {
	candidates::human(lease)?;
	units::authorize(lease, &input.bank, "memory.read").await?;
	let index = crate::apps::knowledge::models::SemanticIndexe::locked(
		&mut **lease.tx(),
		input.bank.workspace,
		true,
	)
	.await?
	.ok_or(Error::SemanticUnavailable)?;
	let configuration = index.configuration()?;
	if expected < 1 || index.revision != expected {
		return Err(Error::Conflict(
			"observed memory index revision changed".into(),
		));
	}
	let embedding: EmbeddingConfig = serde_json::from_value(
		definition(lease, &policy.embedding, "embedding")
			.await?
			.config,
	)?;
	if !configuration.enabled
		|| serde_json::to_value(embedding)? != serde_json::to_value(configuration.embedding)?
	{
		return Err(Error::Conflict(
			"configure the approved embedding generation before rebuilding".into(),
		));
	}
	let bank_id = repository::bank_id(lease, &input.bank, false)
		.await?
		.ok_or(Error::Forbidden)?;
	let digest = aidash_domain::semantic::indexing::content_digest(&serde_json::to_string(input)?);
	if let Some(receipt) = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_receipts"))
			.and_where(Expr::col("operation_id").eq(Expr::value(input.operation_id)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_optional(&mut **lease.tx())
	.await?
	{
		if receipt.try_get::<String>("digest")? != digest
			|| receipt.try_get::<uuid::Uuid>("bank_id")? != bank_id
		{
			return Err(Error::Conflict("memory operation ID was reused".into()));
		}
		let evidence: Vec<Evidence> = receipt.try_get("outcome")?;
		units::current(
			lease,
			input.bank.workspace,
			&evidence,
			policy.bounds.max_graph_visits,
		)
		.await?;
		let mut result = vec![];
		for source in evidence {
			if let Evidence::Unit { id, .. } = source {
				result.push(
					units::load(lease, id, false)
						.await?
						.ok_or(Error::Forbidden)?,
				);
			}
		}
		return Ok(result);
	}
	repository::record_capacity(
		lease,
		bank_id,
		"memory_receipts",
		policy.retention.max_model_operations,
	)
	.await?;
	let selected = repository::list(
		lease,
		&input.bank,
		policy.bounds.max_units,
		policy.bounds.max_graph_visits,
	)
	.await?;
	let actor = lease.saved()?["subject"]
		.as_str()
		.ok_or(Error::Forbidden)?
		.to_owned();
	for unit in &selected {
		repository::project(lease, unit, &actor, Some(&input.provider)).await?;
	}
	memory_receipts::reserve(
		lease,
		input.operation_id,
		bank_id,
		&digest,
		&selected.iter().map(Unit::evidence).collect::<Vec<_>>(),
		Utc::now(),
	)
	.await?;
	Ok(selected)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct Inspection {
	pub items: Vec<UnitStatus>,
	pub next: Option<uuid::Uuid>,
	pub purge_jobs: Vec<PurgeStatus>,
	pub model_operations: i64,
	pub pending_candidates: i64,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct UnitStatus {
	pub id: uuid::Uuid,
	pub revision: i64,
	pub deleted: bool,
	pub stale: bool,
	pub disclosure_current: bool,
	pub dependent_units: i64,
	pub dependent_runs: i64,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct PurgeStatus {
	pub unit_id: uuid::Uuid,
	pub revision: i64,
	pub state: String,
	pub purge_after: chrono::DateTime<Utc>,
	pub backup_until: chrono::DateTime<Utc>,
	pub attempts: i32,
	pub next_attempt: chrono::DateTime<Utc>,
	pub last_error: Option<String>,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct History {
	pub unit_id: uuid::Uuid,
	pub revision: i64,
	pub operation_id: uuid::Uuid,
	pub actor: String,
	pub updated_at: chrono::DateTime<Utc>,
	pub deleted: bool,
	pub stale: bool,
	/// Body is withheld after deletion or when its exact evidence is no longer disclosable.
	pub content: Option<Content>,
}
async fn count(lease: &mut Lease<'_>, table: &str, column: &str, id: uuid::Uuid) -> Result<i64> {
	native::query_scalar(
		&Query::select()
			.expr(reinhardt::query::Func::count(Expr::col("id").into()))
			.from(Alias::new(table))
			.and_where(Expr::col(Alias::new(column)).eq(Expr::value(id)))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut **lease.tx())
	.await
}
pub(crate) async fn inspect(
	lease: &mut Lease<'_>,
	bank: &Bank,
	after: Option<uuid::Uuid>,
	policy: &Policy,
) -> Result<Inspection> {
	units::authorize(lease, bank, "memory.history.read").await?;
	let Some(id) = repository::bank_id(lease, bank, false).await? else {
		return Ok(Inspection {
			items: vec![],
			next: None,
			purge_jobs: vec![],
			model_operations: 0,
			pending_candidates: 0,
		});
	};
	let mut query = Query::select();
	query
		.column(ColumnRef::Asterisk)
		.from(Alias::new("memory_units"))
		.and_where(Expr::col("bank_id").eq(Expr::value(id)));
	if let Some(after) = after {
		query.and_where(Expr::col("id").gt(Expr::value(after)));
	}
	query
		.order_by(Alias::new("id"), reinhardt::query::Order::Asc)
		.limit(129);
	let mut rows = native::query(&query.to_string(PostgresQueryBuilder))
		.fetch_all(&mut **lease.tx())
		.await?;
	let more = rows.len() > 128;
	rows.truncate(128);
	let mut items = vec![];
	for row in rows {
		let unit = units::load(lease, row.try_get("id")?, false)
			.await?
			.ok_or(Error::Forbidden)?;
		let current = async {
			units::unexpired(lease, &unit).await?;
			units::current(
				lease,
				bank.workspace,
				&unit.content.evidence,
				policy.bounds.max_graph_visits,
			)
			.await
		}
		.await;
		let disclosure_current = unit.visible()
			&& match current {
				Ok(()) => true,
				Err(Error::Conflict(_) | Error::Forbidden) => false,
				Err(error) => return Err(error),
			};
		let dependent_units: i64 = native::query_scalar(
			&Query::select()
				.expr(reinhardt::query::Func::count(Expr::col("unit_id").into()))
				.from(Alias::new("memory_dependencies"))
				.and_where(Expr::col("source_kind").eq("unit"))
				.and_where(Expr::col("source_id").eq(Expr::value(unit.id)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&mut **lease.tx())
		.await?;
		let dependent_runs: i64 = native::query_scalar(
			&Query::select()
				.expr(reinhardt::query::Func::count(Expr::col("run_id").into()))
				.from(Alias::new("memory_run_reads"))
				.and_where(Expr::col("unit_id").eq(Expr::value(unit.id)))
				.to_string(PostgresQueryBuilder),
		)
		.scalar_one(&mut **lease.tx())
		.await?;
		items.push(UnitStatus {
			id: unit.id,
			revision: unit.revision,
			deleted: unit.deleted,
			stale: unit.stale,
			disclosure_current,
			dependent_units,
			dependent_runs,
		});
	}
	let jobs = native::query(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("memory_purge_jobs"))
			.and_where(Expr::col("bank_id").eq(Expr::value(id)))
			.order_by(Alias::new("updated_at"), reinhardt::query::Order::Desc)
			.limit(128)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut **lease.tx())
	.await?;
	let purge_jobs = jobs
		.into_iter()
		.map(|row| -> Result<_> {
			Ok(PurgeStatus {
				unit_id: row.try_get("unit_id")?,
				revision: row.try_get("revision")?,
				state: row.try_get("state")?,
				purge_after: row.try_get("purge_after")?,
				attempts: row.try_get("attempts")?,
				next_attempt: row.try_get("next_attempt")?,
				last_error: row.try_get("last_error")?,
				backup_until: row.try_get("backup_until")?,
			})
		})
		.collect::<Result<Vec<_>>>()?;
	let pending_candidates = native::query_scalar(
		&Query::select()
			.expr(reinhardt::query::Func::count(Expr::col("id").into()))
			.from(Alias::new("memory_candidates"))
			.and_where(Expr::col("bank_id").eq(Expr::value(id)))
			.and_where(Expr::col("state").eq("pending"))
			.to_string(PostgresQueryBuilder),
	)
	.scalar_one(&mut **lease.tx())
	.await?;
	Ok(Inspection {
		next: more.then(|| items.last().expect("nonempty page").id),
		items,
		purge_jobs,
		model_operations: count(lease, "memory_model_operations", "bank_id", id).await?,
		pending_candidates,
	})
}
pub(crate) async fn history(
	lease: &mut Lease<'_>,
	bank: &Bank,
	id: uuid::Uuid,
	revision: i64,
	before: Option<i64>,
	policy: &Policy,
) -> Result<Vec<History>> {
	units::authorize(lease, bank, "memory.history.read").await?;
	let unit = units::load(lease, id, false)
		.await?
		.ok_or(Error::Forbidden)?;
	if unit.bank != *bank {
		return Err(Error::Forbidden);
	}
	if unit.revision != revision {
		return Err(Error::Conflict(
			"observed memory history revision changed".into(),
		));
	}
	let mut query = Query::select();
	query
		.column(ColumnRef::Asterisk)
		.from(Alias::new("memory_history"))
		.and_where(Expr::col("unit_id").eq(Expr::value(id)));
	if let Some(before) = before {
		query.and_where(Expr::col("revision").lt(before));
	}
	query
		.order_by(Alias::new("revision"), reinhardt::query::Order::Desc)
		.limit(32);
	let rows = native::query(&query.to_string(PostgresQueryBuilder))
		.fetch_all(&mut **lease.tx())
		.await?;
	let expired = match units::unexpired(lease, &unit).await {
		Ok(_) => false,
		Err(Error::Conflict(_) | Error::Forbidden) => true,
		Err(error) => return Err(error),
	};
	let mut result = vec![];
	for row in rows {
		let content = units::content(&row)?;
		let current = if unit.deleted || expired {
			false
		} else {
			match units::current(
				lease,
				bank.workspace,
				&content.evidence,
				policy.bounds.max_graph_visits,
			)
			.await
			{
				Ok(()) => true,
				Err(Error::Forbidden | Error::Conflict(_)) => false,
				Err(error) => return Err(error),
			}
		};
		result.push(History {
			unit_id: id,
			revision: row.try_get("revision")?,
			operation_id: row.try_get("operation_id")?,
			actor: row.try_get("actor")?,
			updated_at: row.try_get("updated_at")?,
			deleted: row.try_get("deleted")?,
			stale: row.try_get("stale")?,
			content: current.then_some(content),
		});
	}
	Ok(result)
}

/// Job metadata is visible through the bank's history permission; input proofs
/// and saved credentials are never included in the operational view.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[schemars(rename = "MemoryJobPage")]
pub struct JobPage {
	pub items: Vec<JobStatus>,
	pub next: Option<uuid::Uuid>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[schemars(rename = "MemoryUsagePage")]
pub struct UsagePage {
	pub items: Vec<OperationUsage>,
	pub next: Option<uuid::Uuid>,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[schemars(rename = "MemoryOperationUsage")]
pub struct OperationUsage {
	pub id: uuid::Uuid,
	pub provider: aidash_domain::registry::EntityRef,
	pub calls: i64,
	pub tokens: i64,
	pub cost_micros: i64,
	pub created_at: chrono::DateTime<Utc>,
}
/// Includes conservatively charged unknown outcomes, not an estimate of refunds.
pub(crate) async fn usage(
	lease: &mut Lease<'_>,
	bank: &Bank,
	after: Option<uuid::Uuid>,
) -> Result<UsagePage> {
	let Some(id) = repository::bank_id(lease, bank, false).await? else {
		return Ok(UsagePage {
			items: vec![],
			next: None,
		});
	};
	let mut query = Query::select();
	query
		.column(ColumnRef::Asterisk)
		.from(Alias::new("memory_model_operations"))
		.and_where(Expr::col("bank_id").eq(Expr::value(id)))
		.order_by(Alias::new("id"), reinhardt::query::Order::Asc)
		.limit(129);
	if let Some(after) = after {
		query.and_where(Expr::col("id").gt(Expr::value(after)));
	}
	let mut rows = native::query(&query.to_string(PostgresQueryBuilder))
		.fetch_all(&mut **lease.tx())
		.await?;
	let more = rows.len() > 128;
	rows.truncate(128);
	let items = rows
		.into_iter()
		.map(|row| {
			Ok(OperationUsage {
				id: row.try_get("id")?,
				provider: aidash_domain::registry::EntityRef {
					id: row.try_get("provider_id")?,
					version: row.try_get("provider_version")?,
				},
				calls: row.try_get("calls")?,
				tokens: row.try_get("tokens")?,
				cost_micros: row.try_get("cost_micros")?,
				created_at: row.try_get("created_at")?,
			})
		})
		.collect::<Result<Vec<_>>>()?;
	Ok(UsagePage {
		next: more.then(|| items.last().unwrap().id),
		items,
	})
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[schemars(rename = "MemoryJobStatus")]
pub struct JobStatus {
	pub id: uuid::Uuid,
	pub kind: String,
	pub state: String,
	pub attempts: i32,
	pub next_attempt: chrono::DateTime<Utc>,
	pub last_error: Option<String>,
	pub updated_at: chrono::DateTime<Utc>,
}
pub(crate) async fn jobs(
	lease: &mut Lease<'_>,
	bank: &Bank,
	after: Option<uuid::Uuid>,
) -> Result<JobPage> {
	let Some(id) = repository::bank_id(lease, bank, false).await? else {
		return Ok(JobPage {
			items: vec![],
			next: None,
		});
	};
	let mut query = Query::select();
	query
		.column(ColumnRef::Asterisk)
		.from(Alias::new("memory_engine_jobs"))
		.and_where(Expr::col("bank_id").eq(Expr::value(id)))
		.order_by(Alias::new("id"), reinhardt::query::Order::Asc)
		.limit(129);
	if let Some(after) = after {
		query.and_where(Expr::col("id").gt(Expr::value(after)));
	}
	let mut rows = native::query(&query.to_string(PostgresQueryBuilder))
		.fetch_all(&mut **lease.tx())
		.await?;
	let more = rows.len() > 128;
	rows.truncate(128);
	let items = rows
		.into_iter()
		.map(|row| {
			Ok(JobStatus {
				id: row.try_get("id")?,
				kind: row.try_get("kind")?,
				state: row.try_get("state")?,
				attempts: row.try_get("attempts")?,
				next_attempt: row.try_get("next_attempt")?,
				last_error: row.try_get("last_error")?,
				updated_at: row.try_get("updated_at")?,
			})
		})
		.collect::<Result<Vec<_>>>()?;
	Ok(JobPage {
		next: more.then(|| items.last().unwrap().id),
		items,
	})
}
