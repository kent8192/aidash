//! Degraded context (ADR 0059): a degraded workspace slot is recorded, emitted
//! and never cached, and only the last outage retry may degrade.
use super::*;
use aidash_domain::semantic::{
	Failure as SemanticFailure,
	degraded::{DegradedStep, OutageCause, marker},
};

fn degraded_envelope() -> Value {
	json!({"workspace":marker(OutageCause::EmbeddingProvider),"memory":{"status":"disabled"}})
}

fn saved(fixture: &Fixture, event: &str) -> Vec<Run> {
	fixture
		.backend
		.0
		.writes
		.lock()
		.unwrap()
		.iter()
		.filter(|(kind, _)| kind == event)
		.map(|(_, run)| run.clone())
		.collect()
}

#[async_trait]
impl aidash_application::ports::ExecutionRecoveryStore for Backend {
	async fn leased_run(&self, token: Uuid) -> Result<Option<Run>> {
		assert_eq!(token, self.0.token);
		Ok(self
			.0
			.writes
			.lock()
			.unwrap()
			.last()
			.map(|(_, run)| run.clone()))
	}
	async fn save(&self, run: &Run, token: Uuid, event: &str) -> Result<()> {
		ExecutionStore::save_run(self, run, token, event).await
	}
	async fn pause(&self, _: &Run, _: Uuid, _: &str, _: &str) -> Result<()> {
		unexpected("ExecutionRecoveryStore.pause")
	}
	async fn pause_semantic(&self, _: &Run, _: Uuid, _: SemanticFailure) -> Result<()> {
		unexpected("ExecutionRecoveryStore.pause_semantic")
	}
	async fn pause_context(
		&self,
		_: &Run,
		_: Uuid,
		_: aidash_domain::context::recovery::Failure,
	) -> Result<()> {
		unexpected("ExecutionRecoveryStore.pause_context")
	}
}

#[rstest]
#[case::legacy(false)]
#[case::ordered(true)]
#[tokio::test]
async fn a_workspace_success_is_durable_before_memory_failure_reloads_the_run(
	mut canonical: Fixture,
	#[case] is_ordered: bool,
) {
	use aidash_application::{ports::ExecutionRecoveryStore, recovery::ExecutionFailure};
	if is_ordered {
		ordered(&mut canonical);
	}
	canonical
		.run
		.recovery
		.degraded
		.record(canonical.run.step - 1, OutageCause::EmbeddingProvider);
	let backend = canonical.backend.clone();
	backend
		.persist(&canonical.run, "fixture.saved", None)
		.unwrap();
	*backend.0.semantic_failure.lock().unwrap() = Some((true, SemanticFailure::Pending));
	assert!(matches!(
		advance_sources(&mut canonical).await,
		Err(Error::RemoteSemantic(SemanticFailure::Pending))
	));
	// Real worker recovery reloads the stored Run, rather than trusting the
	// executor's mutated copy. The workspace success must already be durable.
	crate::recovery::recover(
		&backend,
		backend.0.token,
		ExecutionFailure::Semantic(SemanticFailure::Pending),
		chrono::Utc::now(),
	)
	.await
	.unwrap();
	canonical.run = backend.leased_run(backend.0.token).await.unwrap().unwrap();
	assert!(!canonical.run.recovery.degraded.outage);
	assert_eq!(
		canonical.run.recovery.degraded.count, 1,
		"past degradation evidence is retained"
	);
	assert!(
		saved(&canonical, "run.sources_observed").is_empty(),
		"partial retrieval is never cached"
	);

	*backend.0.semantic_failure.lock().unwrap() = Some((false, SemanticFailure::Unavailable));
	assert!(matches!(
		advance_sources(&mut canonical).await,
		Err(Error::RemoteSemantic(SemanticFailure::Unavailable))
	));
	assert_eq!(
		*backend.0.semantic_degrade.lock().unwrap(),
		[true, false],
		"a fresh outage gets its bounded retry"
	);
	crate::recovery::recover(
		&backend,
		backend.0.token,
		ExecutionFailure::Semantic(SemanticFailure::Unavailable),
		chrono::Utc::now(),
	)
	.await
	.unwrap();
	let stored = backend.leased_run(backend.0.token).await.unwrap().unwrap();
	assert_eq!(stored.recovery.retry.as_ref().unwrap().count, 1);
	assert_eq!(stored.control.as_str(), "ACTIVE");
}

#[rstest]
#[case::legacy(false)]
#[case::ordered(true)]
#[tokio::test]
async fn degraded_context_is_recorded_and_retrieved_again_by_the_next_step(
	mut canonical: Fixture,
	#[case] is_ordered: bool,
) {
	if is_ordered {
		ordered(&mut canonical);
	}
	*canonical.backend.0.memory_value.lock().unwrap() = degraded_envelope();
	assert!(advance_sources(&mut canonical).await.is_err());

	let degraded = saved(&canonical, "run.semantic_degraded");
	assert_eq!(degraded.len(), 1);
	assert_eq!(
		degraded[0].recovery.degraded.recent,
		[DegradedStep {
			step: canonical.run.step,
			cause: OutageCause::EmbeddingProvider
		}]
	);
	assert!(degraded[0].recovery.degraded.outage);
	// Never cached: no observation was saved for the degraded step.
	assert!(saved(&canonical, "run.sources_observed").is_empty());
	assert!(degraded[0].context.source_observation.is_none());

	// The provider recovers. The next step retrieves again, even under the same
	// Retrieval Key, and a successful retrieval ends the outage.
	*canonical.backend.0.memory_value.lock().unwrap() = json!({"fact":"recovered"});
	canonical.run.step += 1;
	assert!(advance_sources(&mut canonical).await.is_err());
	assert_eq!(count(&canonical, "source.memory"), 2);
	let observed = saved(&canonical, "run.sources_observed");
	assert_eq!(observed.len(), 1);
	assert!(!observed[0].recovery.degraded.outage);
	assert_eq!(observed[0].recovery.degraded.count, 1);
	if is_ordered {
		let requests = canonical.backend.0.requests.lock().unwrap();
		assert_eq!(semantic_memory(&requests[0]), degraded_envelope());
		assert_eq!(semantic_memory(&requests[1]), json!({"fact":"recovered"}));
	}
}

#[rstest]
#[case::legacy(false)]
#[case::ordered(true)]
#[tokio::test]
async fn a_step_that_degrades_again_saves_one_degradation_and_no_observation(
	mut canonical: Fixture,
	#[case] is_ordered: bool,
) {
	if is_ordered {
		ordered(&mut canonical);
	}
	*canonical.backend.0.memory_value.lock().unwrap() = degraded_envelope();
	assert!(advance_sources(&mut canonical).await.is_err());
	// The same step is entered again, as after a worker crash.
	assert!(advance_sources(&mut canonical).await.is_err());

	let degraded = saved(&canonical, "run.semantic_degraded");
	assert_eq!(degraded.len(), 1);
	assert_eq!(degraded[0].recovery.degraded.count, 1);
	assert!(degraded[0].recovery.degraded.outage);
	// The replay observed no source, so it reports none.
	assert!(saved(&canonical, "run.sources_observed").is_empty());
}

#[rstest]
#[case::first_attempt(None, None, false, false)]
#[case::fourth_retry(Some(SemanticFailure::Unavailable), Some(4), false, false)]
#[case::last_retry(Some(SemanticFailure::Unavailable), Some(5), false, true)]
#[case::pending_replay_before_the_last_retry(Some(SemanticFailure::Pending), Some(4), false, false)]
#[case::pending_replay_of_the_last_retry(Some(SemanticFailure::Pending), Some(5), false, true)]
#[case::after_a_degraded_step(None, None, true, true)]
#[tokio::test]
async fn only_the_last_outage_retry_or_an_ongoing_outage_may_degrade(
	mut canonical: Fixture,
	#[case] reason: Option<SemanticFailure>,
	#[case] retries: Option<u32>,
	#[case] outage: bool,
	#[case] degrade: bool,
) {
	canonical.run.recovery.semantic_reason = reason;
	canonical.run.recovery.retry = retries.map(|count| RetryState {
		count,
		at: chrono::Utc::now(),
	});
	canonical.run.recovery.degraded.outage = outage;
	assert!(advance_sources(&mut canonical).await.is_err());
	assert_eq!(
		*canonical.backend.0.semantic_degrade.lock().unwrap(),
		[degrade]
	);
}
