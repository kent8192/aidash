//! Inference Attempts: one outcome per attempt, lease fencing and the
//! replayable Run stream.
#[path = "support/legacy.rs"]
mod common;
#[path = "inference_progress/stream.rs"]
mod stream;
#[path = "inference_progress/worker.rs"]
mod worker;

use aidash_domain::provider::progress::{
	InferenceAttemptId, InferenceProgress, InterruptionReason, ProgressOutcome,
};
use aidash_server::{
	domain::{Run, RunState, TerminalState},
	error::Error,
	federation::Federation,
};
use common::{TestEnvironment, bootstrap, cleanup, request, setup, test_environment};
use reinhardt::query::{
	Alias, Expr, ExprTrait as _, PostgresQueryBuilder, Query, QueryStatementBuilder as _,
};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

struct Conversation {
	f: Federation,
	url: String,
	schema: String,
	app: common::TestApplication,
	token: String,
	run: Run,
}

impl Conversation {
	async fn new(
		environment: &TestEnvironment,
		service: Option<aidash_server::sse::Service>,
	) -> Self {
		let (f, url, schema) = setup(environment).await;
		let app = match service {
			Some(service) => {
				common::application_with_event_streams(
					f.clone(),
					aidash_server::http::Settings::default(),
					service,
				)
				.await
			}
			None => common::application(f.clone()).await,
		};
		let (_, token, _) = bootstrap(&f, &app, "http://127.0.0.1:9").await;
		let (status, created) = request(
			&app,
			&token,
			"POST",
			"/api/conversations",
			json!({"title":"Inference progress","goal":"Reply","target":{"id":"research","version":"1.0.0"},"target_kind":"agent"}),
		)
		.await;
		assert_eq!(status, 200, "{created}");
		let run = f.store.runs().await.unwrap().remove(0);
		Self {
			f,
			url,
			schema,
			app,
			token,
			run,
		}
	}

	async fn lease(&self, worker: Uuid) -> Run {
		let leased = self.f.store.lease_run(worker, 30).await.unwrap().unwrap();
		assert_eq!(leased.id, self.run.id);
		leased
	}

	async fn start(&self, worker: Uuid) -> InferenceAttemptId {
		let attempt = InferenceAttemptId::new();
		self.f
			.store
			.start_inference(self.run.id, worker, attempt)
			.await
			.unwrap();
		attempt
	}

	async fn text(&self, worker: Uuid, attempt: InferenceAttemptId, text: &str) {
		self.f
			.store
			.append_inference_progress(
				self.run.id,
				worker,
				attempt,
				&[InferenceProgress::Text { text: text.into() }],
			)
			.await
			.unwrap();
	}

	/// `(outcome, reason)` of one attempt.
	async fn outcome(&self, attempt: InferenceAttemptId) -> (Option<String>, Option<String>) {
		sqlx::query_as(
			&Query::select()
				.columns([Alias::new("outcome"), Alias::new("reason")])
				.from(Alias::new("inference_attempts"))
				.and_where(Expr::col(Alias::new("id")).eq(Expr::value(attempt.0)))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(self.f.store.pool.driver())
		.await
		.unwrap()
	}

	/// Stored outcome rows and journal outcome markers of one attempt.
	async fn closures(&self, attempt: InferenceAttemptId) -> (i64, i64) {
		let rows: i64 = sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("count(*)"))
				.from(Alias::new("inference_progress"))
				.and_where(Expr::col(Alias::new("attempt_id")).eq(Expr::value(attempt.0)))
				.and_where(Expr::col(Alias::new("kind")).eq("outcome"))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(self.f.store.pool.driver())
		.await
		.unwrap();
		let markers: i64 = sqlx::query_scalar(
			&Query::select()
				.expr(Expr::cust("count(*)"))
				.from(Alias::new("events"))
				.and_where(Expr::cust_with_values(
					"data->>'attempt_id' = ?",
					[attempt.0.to_string()],
				))
				.and_where(Expr::col(Alias::new("kind")).is_in([
					"inference.accepted",
					"inference.discarded",
					"inference.interrupted",
				]))
				.to_string(PostgresQueryBuilder),
		)
		.fetch_one(self.f.store.pool.driver())
		.await
		.unwrap();
		(rows, markers)
	}

	async fn finish(self) {
		cleanup(self.f, &self.url, &self.schema).await;
	}
}

fn completed(mut run: Run) -> Run {
	run.state = RunState::ToolCall(Box::new(common::tool_call(json!({
		"included_input_seq":0,
		"response":{"text":"accepted","tool_calls":[],"input_tokens":1,"output_tokens":1,"usage_complete":true},
		"cursor":0
	}))));
	run
}

#[derive(Debug, Clone, Copy)]
enum Closure {
	Accepted,
	StaleDiscard,
	OrphanOnNextStart,
	Terminal,
}

#[rstest::rstest]
#[case::accepted(Closure::Accepted, "accepted", None)]
#[case::stale_discard(Closure::StaleDiscard, "discarded", None)]
#[case::orphan_on_next_start(Closure::OrphanOnNextStart, "interrupted", Some("lease_lost"))]
#[case::terminal_closure(Closure::Terminal, "interrupted", Some("lease_lost"))]
#[tokio::test]
async fn every_attempt_records_exactly_one_outcome(
	#[case] closure: Closure,
	#[case] outcome: &str,
	#[case] reason: Option<&str>,
	#[future(awt)]
	#[from(test_environment)]
	test_environment: Arc<TestEnvironment>,
) {
	// Arrange
	let c = Conversation::new(&test_environment, None).await;
	let worker = Uuid::new_v4();
	let leased = c.lease(worker).await;
	let attempt = c.start(worker).await;
	c.text(worker, attempt, "tentative").await;

	// Act
	match closure {
		Closure::Accepted => {
			c.f.store
				.save_run(&completed(leased), worker, "model.completed")
				.await
				.unwrap();
		}
		Closure::StaleDiscard => {
			let key = format!("human:{}:{}", c.run.id, Uuid::new_v4());
			let limit = c.f.run_message_limit(&c.run).await.unwrap();
			c.f.store
				.accept_run_message(c.run.id, "human", "correction", &key, limit)
				.await
				.unwrap();
			assert!(matches!(
				c.f.store
					.save_run(&completed(leased), worker, "model.completed")
					.await,
				Err(Error::StaleInference)
			));
		}
		Closure::OrphanOnNextStart => {
			c.f.store.release_lease(c.run.id, worker).await.unwrap();
			let next = Uuid::new_v4();
			c.lease(next).await;
			let replacement = c.start(next).await;
			assert_eq!(c.outcome(replacement).await, (None, None));
		}
		Closure::Terminal => {
			let mut failed = leased;
			failed.state = RunState::Failed(TerminalState {});
			c.f.store
				.save_run(&failed, worker, "run.failed")
				.await
				.unwrap();
		}
	}
	// A late harness closure never replaces the outcome already written.
	let late =
		c.f.store
			.finish_inference(
				c.run.id,
				worker,
				attempt,
				ProgressOutcome::Interrupted(InterruptionReason::StreamError),
			)
			.await;

	// Assert
	assert!(matches!(late, Ok(()) | Err(Error::Conflict(_))), "{late:?}");
	assert_eq!(
		c.outcome(attempt).await,
		(Some(outcome.to_owned()), reason.map(str::to_owned))
	);
	assert_eq!(c.closures(attempt).await, (1, 1));
	c.finish().await;
}

#[rstest::rstest]
#[tokio::test]
async fn writes_after_the_lease_is_lost_are_rejected(
	#[future(awt)]
	#[from(test_environment)]
	test_environment: Arc<TestEnvironment>,
) {
	// Arrange
	let c = Conversation::new(&test_environment, None).await;
	let worker = Uuid::new_v4();
	c.lease(worker).await;
	let attempt = c.start(worker).await;
	c.f.store.release_lease(c.run.id, worker).await.unwrap();

	// Act
	let append =
		c.f.store
			.append_inference_progress(
				c.run.id,
				worker,
				attempt,
				&[InferenceProgress::Text {
					text: "late".into(),
				}],
			)
			.await;
	let finish =
		c.f.store
			.finish_inference(
				c.run.id,
				worker,
				attempt,
				ProgressOutcome::Interrupted(InterruptionReason::Cancelled),
			)
			.await;
	let start =
		c.f.store
			.start_inference(c.run.id, worker, InferenceAttemptId::new())
			.await;

	// Assert
	for result in [append, finish, start] {
		assert!(matches!(result, Err(Error::Conflict(_))), "{result:?}");
	}
	assert_eq!(c.outcome(attempt).await, (None, None));
	let stored: i64 = sqlx::query_scalar(
		&Query::select()
			.expr(Expr::cust("count(*)"))
			.from(Alias::new("inference_progress"))
			.and_where(Expr::col(Alias::new("run_id")).eq(Expr::value(c.run.id)))
			.to_string(PostgresQueryBuilder),
	)
	.fetch_one(c.f.store.pool.driver())
	.await
	.unwrap();
	assert_eq!(stored, 1, "only the started row was written");
	c.finish().await;
}
