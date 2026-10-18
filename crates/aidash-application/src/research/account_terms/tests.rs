use super::*;
use crate::ports::research::ProviderResultRetention;
use aidash_domain::{
	context::HistoryEntry,
	research::{Operation, envelope},
};
use async_trait::async_trait;
use chrono::TimeZone;
use serde_json::{Value, json};
use std::sync::Mutex;

fn day(day: i64) -> DateTime<Utc> {
	Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap() + Duration::days(day)
}

fn terms(account: &str, retention_days: u16, terminated_at: Option<DateTime<Utc>>) -> AccountTerms {
	let mut terms: AccountTerms = serde_json::from_value(json!({
		"account_id": account,
		"agreement_reference": "agreement",
		"verified_by": "deployment_administrator",
		"verified_at": "2026-09-01T00:00:00Z",
		"review_after": "2027-09-01T00:00:00Z",
		"storage_permitted": true,
		"query_training_prohibited": true,
		"evaluation_permitted": true,
		"retention_days": retention_days,
		"deletion_after_termination_days": 7,
		"price_effective_at": "2026-09-01T00:00:00Z",
		"price_review_after": "2027-09-01T00:00:00Z",
		"search_price_micro_usd": 5000,
		"monthly_base_fee_micro_usd": 0,
		"monthly_limit_micro_usd": 20_000_000
	}))
	.unwrap();
	terms.terminated_at = terminated_at;
	terms
}

#[derive(Default)]
struct Results {
	all: Vec<RetainedResult>,
	terminations: Mutex<Vec<Termination>>,
	purged: Mutex<Vec<Uuid>>,
	asked: Mutex<Vec<(DateTime<Utc>, Vec<String>)>>,
}

impl Results {
	fn new(mut all: Vec<RetainedResult>) -> Self {
		all.sort_by_key(|result| result.attempt_id);
		Self {
			all,
			..Self::default()
		}
	}
	fn purged(&self) -> Vec<Uuid> {
		let mut purged = self.purged.lock().unwrap().clone();
		purged.sort();
		purged
	}
}

#[async_trait]
impl ProviderResultRetention for Results {
	async fn record_terminations(&self, terminations: &[Termination]) -> crate::Result<()> {
		let mut recorded = self.terminations.lock().unwrap();
		for termination in terminations {
			if !recorded
				.iter()
				.any(|known| known.account_id == termination.account_id)
			{
				recorded.push(termination.clone());
			}
		}
		Ok(())
	}
	async fn terminations(&self) -> crate::Result<Vec<Termination>> {
		Ok(self.terminations.lock().unwrap().clone())
	}
	async fn candidates(
		&self,
		after: Option<Uuid>,
		retrieved_before: DateTime<Utc>,
		accounts: &[String],
		limit: u32,
	) -> crate::Result<Vec<RetainedResult>> {
		self.asked
			.lock()
			.unwrap()
			.push((retrieved_before, accounts.to_vec()));
		let purged = self.purged.lock().unwrap();
		Ok(self
			.all
			.iter()
			.filter(|result| after.is_none_or(|after| result.attempt_id > after))
			.filter(|result| !purged.contains(&result.attempt_id))
			.filter(|result| {
				result.retrieved_at <= retrieved_before || accounts.contains(&result.account_id)
			})
			.take(limit as usize)
			.cloned()
			.collect())
	}
	async fn purge(&self, attempts: &[Uuid], _: DateTime<Utc>) -> crate::Result<u64> {
		self.purged.lock().unwrap().extend(attempts);
		Ok(attempts.len() as u64)
	}
}

fn retained(account: &str, retrieved_at: DateTime<Utc>, retention_days: u16) -> RetainedResult {
	RetainedResult {
		attempt_id: Uuid::now_v7(),
		account_id: account.into(),
		retrieved_at,
		retention_days,
	}
}

#[tokio::test]
async fn results_expire_under_the_shorter_of_recorded_and_current_terms() {
	let old = retained("brave", day(0), 30);
	let young = retained("brave", day(25), 30);
	// Recorded under longer terms; the current ten days apply.
	let shortened = retained("brave", day(15), 90);
	// The declaration is gone; the recorded period still applies.
	let removed = retained("gone", day(0), 20);
	let results = Results::new(vec![old.clone(), young, shortened.clone(), removed.clone()]);
	let purged = sweep(&results, &[terms("brave", 10, None)], day(31), 2)
		.await
		.unwrap();
	assert_eq!(purged, 3);
	let mut expected = vec![old.attempt_id, shortened.attempt_id, removed.attempt_id];
	expected.sort();
	assert_eq!(results.purged(), expected);
	// Only results at least a day old are examined.
	assert!(
		results
			.asked
			.lock()
			.unwrap()
			.iter()
			.all(|(before, accounts)| *before == day(30) && accounts.is_empty())
	);
}

#[tokio::test]
async fn a_recorded_termination_deletes_results_after_its_deadline_even_once_undeclared() {
	let recent = retained("ended", day(9), 90);
	let other = retained("brave", day(9), 90);
	let results = Results::new(vec![recent.clone(), other]);
	// Within the deletion period nothing goes, but the end is recorded.
	let ended = terms("ended", 90, Some(day(10)));
	assert_eq!(sweep(&results, &[ended], day(16), 8).await.unwrap(), 0);
	assert_eq!(results.terminations.lock().unwrap().len(), 1);
	// The declaration is removed; the recorded end still applies.
	assert_eq!(
		sweep(&results, &[terms("brave", 90, None)], day(17), 8)
			.await
			.unwrap(),
		1
	);
	assert_eq!(results.purged(), vec![recent.attempt_id]);
	assert_eq!(
		results.asked.lock().unwrap().last().unwrap().1,
		vec!["ended".to_owned()]
	);
}

struct Journal(Vec<HistoryEntry>);

#[async_trait]
impl super::super::revocation::Journal for Journal {
	async fn entries(&self, from: u64, through: u64) -> crate::Result<Vec<HistoryEntry>> {
		Ok(self
			.0
			.iter()
			.filter(|entry| (from..=through).contains(&entry.seq))
			.cloned()
			.collect())
	}
}

const SOURCE: &str = "0190f1d2-0000-7000-8000-000000000001";

fn search(source: &str) -> Value {
	envelope(
		Operation::Search,
		"ok",
		json!({"sources": [{"source_id": source, "title": "t", "snippet": "provider text"}]}),
		false,
	)
}

fn call(id: &str, result: Value) -> ContextEvent {
	ContextEvent::Tool {
		call: aidash_domain::provider::ToolCall {
			id: id.into(),
			name: "web_search".into(),
			arguments: json!({"query": "sent"}),
		},
		result,
	}
}

fn expired(through: DateTime<Utc>) -> ExpiredResults {
	ExpiredResults {
		sources: [SOURCE.parse().unwrap()].into(),
		documents: BTreeSet::new(),
		through: Some(through),
	}
}

fn summarized(context: &mut Context) {
	use aidash_domain::context::summary::*;
	let summary = ExecutionSummary::merge(
		SummaryContent {
			goal: "goal".into(),
			constraints: vec![],
			decisions: vec![],
			unresolved: vec![],
			resolved: vec![],
			artifacts: vec![],
			verification: vec![],
		},
		None,
		&context.history[..1],
		Default::default(),
		"context-recovery/1",
		SummaryProvider {
			model: aidash_domain::registry::EntityRef {
				id: "summarizer".into(),
				version: "1.0.0".into(),
			},
			definition_digest: "digest".into(),
		},
	)
	.unwrap();
	context.execution_summary = Some(Box::new(summary));
	context.history.remove(0);
}

#[tokio::test]
async fn a_summary_that_absorbed_an_expired_result_is_restored_with_the_placeholder() {
	let mut context = Context::default();
	context.push(call("searched", search(SOURCE)));
	context.push(call("later", json!({"status": "ok"})));
	let journal = Journal(context.history.clone());
	summarized(&mut context);
	assert!(
		expire_projection(&mut context, &expired(day(1)), &journal)
			.await
			.unwrap()
	);
	assert!(context.execution_summary.is_none());
	assert_eq!(context.research_expired_through, Some(day(1)));
	assert!(matches!(
		&context.history[0].event,
		ContextEvent::Tool { result, .. } if *result == ContextEvent::expired_result("searched")
	));
	// The same purges again change nothing.
	assert!(
		!expire_projection(&mut context, &expired(day(1)), &journal)
			.await
			.unwrap()
	);
}

#[tokio::test]
async fn a_summary_without_expired_results_is_kept() {
	let mut context = Context::default();
	context.push(call("kept", search("0190f1d2-0000-7000-8000-00000000000f")));
	context.push(call("searched", search(SOURCE)));
	let journal = Journal(context.history.clone());
	summarized(&mut context);
	assert!(
		expire_projection(&mut context, &expired(day(1)), &journal)
			.await
			.unwrap()
	);
	assert!(context.execution_summary.is_some());
	assert!(matches!(
		&context.history[0].event,
		ContextEvent::Tool { result, .. } if *result == ContextEvent::expired_result("searched")
	));
}

#[tokio::test]
async fn no_expired_results_leave_the_projection_unchanged() {
	let mut context = Context::default();
	context.push(call("searched", search(SOURCE)));
	let before = serde_json::to_value(&context).unwrap();
	assert!(
		!expire_projection(&mut context, &ExpiredResults::default(), &Journal(vec![]))
			.await
			.unwrap()
	);
	assert_eq!(serde_json::to_value(&context).unwrap(), before);
}
