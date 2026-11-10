//! Candidate extraction reads complete canonical records, never caller prose or UI previews.
use super::{access::Lease, units};
use crate::{Error, Result, database::native};
use aidash_application::ports::semantic::visibility::SemanticDisclosureScope;
use aidash_domain::{
	Run,
	memory::{Bank, Bounds, Evidence},
};
use reinhardt::query::{
	Alias, ColumnRef, Expr, ExprTrait, LockType, Order, PostgresQueryBuilder, Query,
	QueryStatementBuilder,
};
use serde_json::{Value, json};

/// Load only the offloaded Journal range under the Run's live authority.
pub(crate) async fn offloaded_range(
	lease: &mut Lease<'_>,
	bank: &Bank,
	proof: &Evidence,
	bounds: &Bounds,
) -> Result<String> {
	let Evidence::JournalRange {
		run_id,
		from_seq,
		through_seq,
	} = proof
	else {
		return Err(Error::Invalid(
			"offload extraction requires a Journal range".into(),
		));
	};
	units::current(
		lease,
		bank.workspace,
		std::slice::from_ref(proof),
		bounds.max_graph_visits,
	)
	.await?;
	let from = i64::try_from(*from_seq)
		.map_err(|_| Error::Invalid("Journal range exceeds storage bounds".into()))?;
	let through = i64::try_from(*through_seq)
		.map_err(|_| Error::Invalid("Journal range exceeds storage bounds".into()))?;
	let events = native::query(
		&Query::select()
			.columns(["seq", "event"].map(Alias::new))
			.from(Alias::new(
				crate::apps::execution::repositories::context_journal::EVENTS,
			))
			.and_where(Expr::col("run_id").eq(Expr::value(*run_id)))
			.and_where(Expr::col("seq").gte(Expr::value(from)))
			.and_where(Expr::col("seq").lte(Expr::value(through)))
			.order_by(Alias::new("seq"), Order::Asc)
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_all(&mut **lease.tx())
	.await?;
	let mut entries = Vec::with_capacity(events.len());
	for row in events {
		let seq: i64 = row.try_get("seq")?;
		let event: Value = row.try_get("event")?;
		entries.push(aidash_domain::context::HistoryEntry::new(
			u64::try_from(seq).map_err(|_| Error::Invalid("negative Journal sequence".into()))?,
			serde_json::from_value(event)?,
		));
	}
	crate::apps::execution::repositories::research::resolve_entries(
		lease.tx().as_mut(),
		*run_id,
		&mut entries,
	)
	.await?;
	let values: Vec<_> = entries.into_iter().map(|entry| entry.event).collect();
	let text = serde_json::to_string(&values)?;
	if text.len() > bounds.max_input_bytes {
		return Err(Error::Invalid(
			"offloaded Journal range exceeds memory input bound".into(),
		));
	}
	Ok(text)
}

pub(crate) async fn input(
	lease: &mut Lease<'_>,
	bank: &Bank,
	proof: &Evidence,
	bounds: &Bounds,
) -> Result<(String, Vec<Evidence>)> {
	units::current(
		lease,
		bank.workspace,
		std::slice::from_ref(proof),
		bounds.max_graph_visits,
	)
	.await?;
	let Evidence::Run { id, .. } = proof else {
		return Err(Error::Invalid(
			"learning requires complete Run evidence".into(),
		));
	};
	let run: Run = crate::database::query_as(
		&Query::select()
			.column(ColumnRef::Asterisk)
			.from(Alias::new("runs"))
			.and_where(Expr::col("id").eq(Expr::value(*id)))
			.lock(LockType::Share)
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(&mut **lease.tx())
	.await?;
	if run.home_node != bank.home {
		return Err(Error::Forbidden);
	}
	let binding = super::bindings::load(&mut **lease.tx(), &run.metadata())
		.await?
		.ok_or(Error::Forbidden)?;
	if binding.bank != *bank {
		return Err(Error::Forbidden);
	}
	let mut evidence = vec![proof.clone()];
	let mut remaining = bounds.max_graph_visits;
	let mut journal = serde_json::Map::new();
	let tasks =
		crate::apps::execution::services::task_evidence::complete(lease.tx(), *id, remaining)
			.await?;
	if tasks.len() > remaining {
		return Err(Error::Invalid(
			"complete task journal exceeds the learning record limit".into(),
		));
	}
	remaining -= tasks.len();
	journal.insert("observed_tasks".into(), serde_json::to_value(tasks)?);
	// Counts are shared across collections. No collection can silently truncate.
	for (table, order, columns) in [
		(
			"invocations",
			"created_at",
			vec![
				"idempotency_key",
				"tool",
				"input",
				"status",
				"result",
				"replay_safe",
				"created_at",
			],
		),
		(
			"run_inputs",
			"seq",
			vec!["seq", "sender", "content", "message_id", "reference_only"],
		),
		(
			"human_requests",
			"created_at",
			vec![
				"id",
				"kind",
				"prompt",
				"response",
				"answered_by",
				"created_at",
			],
		),
		(
			"authorization_run_outputs",
			"resource_id",
			vec!["resource_kind", "resource_id"],
		),
	] {
		let mut query = Query::select();
		query
			.columns(columns.iter().map(|column| Alias::new(*column)))
			.from(Alias::new(table))
			.and_where(Expr::col("run_id").eq(Expr::value(*id)))
			.order_by(Alias::new(order), Order::Asc)
			.limit(remaining as u64 + 1)
			.lock(LockType::Share);
		if table == "run_inputs" {
			query.and_where(Expr::col("seq").lte(run.observed_input_seq));
		}
		let rows = native::query(&query.to_string(PostgresQueryBuilder))
			.fetch_all(&mut **lease.tx())
			.await?;
		if rows.len() > remaining {
			return Err(Error::Invalid(
				"complete Run journal exceeds the learning record limit".into(),
			));
		}
		remaining -= rows.len();
		let mut entries = Vec::with_capacity(rows.len());
		for row in rows {
			let mut entry = serde_json::Map::new();
			for column in &columns {
				entry.insert((*column).into(), row.try_get::<Value>(column)?);
			}
			if table == "run_inputs"
				&& let Some(message) = entry["message_id"].as_str()
			{
				let message: uuid::Uuid = message
					.parse()
					.map_err(|_| Error::Invalid("Run input message identity".into()))?;
				let (source, body) = canonical_source(lease, bank, "message", message).await?;
				entry.insert("source".into(), body);
				evidence.push(source);
			}
			if table == "authorization_run_outputs" {
				let kind = entry["resource_kind"].as_str().ok_or(Error::Forbidden)?;
				let id = entry["resource_id"]
					.as_str()
					.ok_or(Error::Forbidden)?
					.parse()
					.map_err(|_| Error::Invalid("Run output identity".into()))?;
				let (source, body) = canonical_source(lease, bank, kind, id).await?;
				entry.insert("source".into(), body);
				evidence.push(source);
			}
			entries.push(Value::Object(entry));
		}
		journal.insert(table.into(), json!(entries));
	}
	evidence.sort();
	evidence.dedup();
	let text = serde_json::to_string(&json!({"run":run,"journal":journal}))?;
	if text.len() > bounds.max_input_bytes || evidence.len() > bounds.max_evidence {
		return Err(Error::Invalid(
			"complete Run learning input exceeds its finite allowance".into(),
		));
	}
	Ok((text, evidence))
}

async fn canonical_source(
	lease: &mut Lease<'_>,
	bank: &Bank,
	kind: &str,
	id: uuid::Uuid,
) -> Result<(Evidence, Value)> {
	let mut disclosure = super::disclosure::Disclosure { lease };
	match kind {
		"message" => {
			let row = disclosure
				.message(id, bank.workspace)
				.await?
				.ok_or(Error::Forbidden)?;
			if disclosure.scoped() && !disclosure.message_visible(&row).await? {
				return Err(Error::Forbidden);
			}
			let proof = Evidence::Message {
				id,
				revision: 1,
				digest: aidash_domain::semantic::indexing::content_digest(&row.content),
			};
			Ok((proof, serde_json::to_value(row)?))
		}
		"artifact" => {
			let row = disclosure
				.artifact(id, bank.workspace)
				.await?
				.ok_or(Error::Forbidden)?;
			if disclosure.scoped() && !disclosure.artifact_visible(&row).await? {
				return Err(Error::Forbidden);
			}
			let proof = Evidence::Artifact {
				id,
				revision: 1,
				digest: aidash_domain::semantic::indexing::content_digest(&serde_json::to_string(
					&row.content,
				)?),
			};
			Ok((proof, serde_json::to_value(row)?))
		}
		_ => Err(Error::Invalid(
			"unsupported canonical Run output kind".into(),
		)),
	}
}
