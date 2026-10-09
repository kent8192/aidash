use crate::Error;
#[rstest::rstest]

fn transient_provider_statuses_keep_the_worker_retry_path() {
	for status in [408, 429, 500, 503] {
		assert!(super::retryable_inference_error(&Error::ProviderRejected {
			status,
			reason: "upstream rejected the request".into(),
		}));
	}
	for status in [400, 401, 403, 413] {
		assert!(!super::retryable_inference_error(
			&Error::ProviderRejected {
				status,
				reason: "upstream rejected the request".into(),
			}
		));
	}
	assert!(!super::retryable_inference_error(&Error::Invalid(
		"Credential Broker rejected Capability Token (capability_expired)".into(),
	)));
}

#[rstest::rstest]
#[tokio::test(start_paused = true)]

async fn inference_cancellation_poll_errors_do_not_signal_cancellation() {
	let pool = sqlx::postgres::PgPoolOptions::new()
		.connect_lazy("postgres://localhost/unused")
		.unwrap();
	pool.close().await;
	let store = crate::store::Store {
		capabilities: crate::capabilities::Runtime::new(Default::default()).unwrap(),
		pool: pool.clone().into(),
		control_pool: pool.into(),
		node_id: "cancellation-poll-test".into(),
		semantic_client: reqwest::Client::new(),
		recovery_cursors: Default::default(),
		provider_credentials: None,
		capability_issuer: None,
	};
	let cancellation = super::wait_for_inference_cancellation(&store, uuid::Uuid::new_v4());
	tokio::pin!(cancellation);
	for _ in 0..3 {
		assert!(
			tokio::time::timeout(std::time::Duration::from_millis(250), &mut cancellation)
				.await
				.is_err(),
			"a failed control read must not interrupt the in-flight inference"
		);
	}
}
