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
	if through_seq
		.checked_sub(*from_seq)
		.and_then(|n| n.checked_add(1))
		!= Some(events.len() as u64)
	{
		return Err(Error::Conflict("offloaded Journal range expired".into()));
	}
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
	// Larger ranges are split by `offload_parts`, never refused (#852 decision 3).
	Ok(serde_json::to_string(&values)?)
}

/// Split an offloaded range's text into parts that each fit one extraction.
///
/// Intake offloads a result because it is large, so its text is often over
/// one extraction's input. Each part is extracted by its own background job,
/// with the operation's own model allowance, and every part cites the whole
/// range `proof` as evidence: the Journal proves ranges by sequence, so the
/// range is the narrowest evidence a part of one event can name. A part keeps
/// `max_input_bytes` and leaves a quarter of `max_model_tokens` for the
/// response, counted as `Models::model` counts input. At most
/// `max_model_calls` parts are extracted per range; the rest stays readable
/// from the Journal. Parts that are only whitespace are skipped, since
/// extraction refuses them.
pub(crate) fn offload_parts<'a>(
	text: &'a str,
	proof: &Evidence,
	bounds: &Bounds,
) -> Result<Vec<&'a str>> {
	let overhead = crate::semantic::services::memory_models::extraction_overhead(
		std::slice::from_ref(proof),
		aidash_domain::memory::extraction::Mode::OffloadedRange,
		bounds,
	)?;
	let room = bounds
		.max_model_tokens
		.checked_sub(overhead)
		.and_then(|room| room.checked_sub(bounds.max_model_tokens / 4))
		.filter(|room| *room > 0)
		.ok_or_else(|| {
			Error::Invalid("memory extraction leaves no room for offloaded text".into())
		})?;
	// serde_json escapes quotes, backslashes and control characters only.
	let escaped = |c: char| match c {
		'"' | '\\' | '\u{8}' | '\u{c}' | '\n' | '\r' | '\t' => 2,
		c if (c as u32) < 0x20 => 6,
		c => c.len_utf8(),
	};
	let mut parts = Vec::new();
	let (mut start, mut raw, mut size) = (0, 0, 0);
	for (index, c) in text.char_indices() {
		if index > start
			&& (size + escaped(c) > room || raw + c.len_utf8() > bounds.max_input_bytes)
		{
			parts.push(&text[start..index]);
			(start, raw, size) = (index, 0, 0);
		}
		raw += c.len_utf8();
		size += escaped(c);
	}
	parts.push(&text[start..]);
	parts.retain(|part| !part.trim().is_empty());
	parts.truncate(bounds.max_model_calls);
	Ok(parts)
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

#[cfg(test)]
mod offload_part_tests {
	use super::*;
	use aidash_domain::registry::bindings::memory_defaults;

	fn builtin_bounds() -> Bounds {
		let entry = memory_defaults::definitions()
			.unwrap()
			.into_iter()
			.find(|entry| entry.id == memory_defaults::MEMORY_ID)
			.unwrap();
		serde_json::from_value::<aidash_domain::memory::ProviderConfig>(entry.config)
			.unwrap()
			.policy
			.bounds
	}

	fn proof() -> Evidence {
		Evidence::JournalRange {
			run_id: uuid::Uuid::now_v7(),
			from_seq: 12,
			through_seq: 12,
		}
	}

	/// #852 decision 3: an intake offload (a single result over a quarter of the
	/// window) used to fail extraction outright. Each part must now fit one
	/// extraction as `Models::model` counts it, under the built-in policy.
	#[rstest::rstest]
	#[case::ascii(serde_json::to_string(&serde_json::json!([{"result": "row ".repeat(12_000)}])).unwrap())]
	#[case::escaped(serde_json::to_string(&serde_json::json!([{"result": "\u{1}\"".repeat(9_000)}])).unwrap())]
	#[case::multibyte(serde_json::to_string(&serde_json::json!([{"result": "記録".repeat(8_000)}])).unwrap())]
	fn intake_offload_splits_into_parts_that_each_fit_one_extraction(#[case] text: String) {
		// Arrange
		let bounds = builtin_bounds();
		let proof = proof();
		let overhead = crate::semantic::services::memory_models::extraction_overhead(
			std::slice::from_ref(&proof),
			aidash_domain::memory::extraction::Mode::OffloadedRange,
			&bounds,
		)
		.unwrap();
		assert!(text.len() > bounds.max_input_bytes);

		// Act
		let parts = offload_parts(&text, &proof, &bounds).unwrap();

		// Assert
		assert!(parts.len() > 1, "{} parts", parts.len());
		assert!(parts.len() <= bounds.max_model_calls);
		let mut offset = 0;
		for part in &parts {
			assert!(part.len() <= bounds.max_input_bytes);
			let input = overhead + serde_json::to_string(part).unwrap().len() - 2;
			assert!(
				input + bounds.max_model_tokens / 4 <= bounds.max_model_tokens,
				"part input {input} leaves no response room"
			);
			// Parts are consecutive slices of the range text.
			assert_eq!(&text[offset..offset + part.len()], *part);
			offset += part.len();
		}
	}

	#[test]
	fn a_range_within_one_extraction_stays_whole() {
		let text = serde_json::to_string(&serde_json::json!([{"result": "short"}])).unwrap();
		assert_eq!(
			offload_parts(&text, &proof(), &builtin_bounds()).unwrap(),
			vec![text.as_str()]
		);
	}
}
