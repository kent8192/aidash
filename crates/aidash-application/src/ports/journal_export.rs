//! Authorized access to the resolved Context Journal.
use crate::{Error, Result, tenancy::TenancyScope};
use aidash_domain::context::ContextEvent;
use aidash_domain::context::journal_export::JournalLine;
use async_trait::async_trait;
use uuid::Uuid;

/// The generation reflects withdrawn Journal content across the entire Run,
/// even when the returned lines are restricted to a smaller range.
#[derive(Debug, Clone)]
pub struct JournalSnapshot {
	pub lines: Vec<JournalLine>,
	pub generation: String,
	pub dependencies: aidash_domain::context::research::JournalDependencies,
}

/// Build a snapshot from the complete set of lines the reader is authorized to inspect.
pub fn snapshot_from_lines(lines: Vec<JournalLine>, range: Option<(u64, u64)>) -> JournalSnapshot {
	let withdrawn: Vec<_> = lines
		.iter()
		.filter_map(|line| {
			let result = line.result.as_ref()?;
			let unavailable = result["contract"] == "aidash.research.unavailable/1";
			(ContextEvent::is_revoked_result(result)
				|| ContextEvent::is_expired_result(result)
				|| unavailable)
				.then(|| (line.seq, result["contract"].clone()))
		})
		.collect();
	let generation = aidash_domain::registry::rules::digest(&serde_json::json!(withdrawn));
	let lines = lines
		.into_iter()
		.filter(|line| range.is_none_or(|(from, through)| (from..=through).contains(&line.seq)))
		.collect::<Vec<_>>();
	let dependencies = aidash_domain::context::research::JournalDependencies::from_lines(&lines);
	JournalSnapshot {
		lines,
		generation,
		dependencies,
	}
}

#[async_trait]
pub trait JournalSource: Send + Sync {
	/// Export an inclusive sequence range under the reader Run's authority.
	async fn export(
		&self,
		scope: &TenancyScope,
		reader_run: Uuid,
		target_run: Uuid,
		range: Option<(u64, u64)>,
	) -> Result<Vec<JournalLine>>;

	async fn snapshot(
		&self,
		scope: &TenancyScope,
		reader_run: Uuid,
		target_run: Uuid,
		range: Option<(u64, u64)>,
	) -> Result<JournalSnapshot> {
		if range.is_some_and(|(from, through)| from > through || through > i64::MAX as u64) {
			return Err(Error::Invalid("invalid journal range".into()));
		}
		let lines = self.export(scope, reader_run, target_run, None).await?;
		Ok(snapshot_from_lines(lines, range))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use aidash_domain::context::{ContextEvent, HistoryEntry};
	use aidash_domain::provider::ToolCall;
	use serde_json::json;

	struct Source(Vec<JournalLine>);
	#[async_trait]
	impl JournalSource for Source {
		async fn export(
			&self,
			_: &TenancyScope,
			_: Uuid,
			_: Uuid,
			_: Option<(u64, u64)>,
		) -> Result<Vec<JournalLine>> {
			Ok(self.0.clone())
		}
	}
	fn line(seq: u64, result: serde_json::Value) -> JournalLine {
		JournalLine::from(&HistoryEntry::new(
			seq,
			ContextEvent::tool(
				ToolCall {
					id: format!("call-{seq}"),
					name: "research_fetch".into(),
					arguments: json!({}),
				},
				result,
			),
		))
	}
	#[tokio::test]
	async fn snapshot_generation_changes_for_withdrawal_outside_the_requested_range() {
		let run = Uuid::new_v4();
		let scope = &TenancyScope::tenant("acme").unwrap();
		let ordinary = line(1, json!({"text":"retained"}));
		let original = Source(vec![ordinary.clone()])
			.snapshot(scope, run, run, Some((2, 2)))
			.await
			.unwrap();
		let appended = Source(vec![ordinary, line(2, json!({"text":"new"}))])
			.snapshot(scope, run, run, Some((2, 2)))
			.await
			.unwrap();
		assert_eq!(original.generation, appended.generation);
		assert_eq!(appended.lines.len(), 1);
		let expired = Source(vec![
			line(1, ContextEvent::expired_result("call-1")),
			line(2, json!({"text":"new"})),
		])
		.snapshot(scope, run, run, Some((2, 2)))
		.await
		.unwrap();
		assert_ne!(appended.generation, expired.generation);
		let revoked = Source(vec![
			line(1, ContextEvent::revoked_result("call-1")),
			line(2, json!({"text":"new"})),
		])
		.snapshot(scope, run, run, Some((2, 2)))
		.await
		.unwrap();
		assert_ne!(expired.generation, revoked.generation);
		assert_eq!(revoked.lines[0].seq, 2);
		let document = Uuid::new_v4();
		let source = Source(vec![line(
			1,
			json!({"contract":aidash_domain::research::RESULT_CONTRACT,
			"operation":"fetch","data":{"document_id":document}}),
		)]);
		let bounded = source
			.snapshot(scope, run, run, Some((2, 2)))
			.await
			.unwrap();
		assert!(bounded.lines.is_empty());
		assert!(bounded.dependencies.is_empty());
	}
}
