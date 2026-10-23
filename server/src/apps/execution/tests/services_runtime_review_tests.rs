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

async fn inference_interruption_poll_errors_do_not_signal_interruption(
	// Arrange: every control and input observation fails.
	#[future(awt)] interruption_store: crate::store::Store,
) {
	// Act
	let interruption = super::wait_for_inference_interruption(
		&interruption_store,
		&aidash_application::tenancy::TEST_FIXTURE,
		uuid::Uuid::new_v4(),
		0,
	);
	tokio::pin!(interruption);
	// Assert
	for _ in 0..3 {
		assert!(
			tokio::time::timeout(std::time::Duration::from_millis(250), &mut interruption)
				.await
				.is_err(),
			"a failed control or input read must not interrupt the in-flight inference"
		);
	}
}

#[rstest::fixture]
async fn interruption_store(
	#[from(reinhardt::test::fixtures::http_client)] http_client: reqwest::Client,
) -> crate::store::Store {
	// An intentionally closed pool preserves the control and input read failures under test.
	// A live test_database fixture would change that failure contract.
	let pool = sqlx::postgres::PgPoolOptions::new()
		.connect_lazy("postgres://localhost/unused")
		.unwrap();
	pool.close().await;
	let tool_clients = aidash_integrations::tools::ToolClients::new().unwrap();
	let keyless = aidash_application::provider_access::KeylessAccess::default();
	crate::store::Store {
		capabilities: crate::capabilities::Runtime::new(Default::default()).unwrap(),
		pool: pool.clone().into(),
		control_pool: pool.into(),
		node_id: "interruption-poll-test".into(),
		semantic_client: http_client.clone(),
		provider_clients: crate::semantic::backend::clients().unwrap(),
		tool_clients: tool_clients.clone(),
		plugin_sessions: crate::store::plugin_sessions(&keyless, &tool_clients),
		keyless,
		research_retention: aidash_domain::retention::RetentionRules::DEFAULT,
		test_limit_ceilings: Default::default(),
		recovery_cursors: Default::default(),
		tenancies: Default::default(),
		tool_slots: aidash_application::ports::execution::ToolSlots::new(1),
		cache_salt: None,
		usage: std::sync::Arc::new(aidash_application::usage::NoUsage),
		admission: std::sync::Arc::new(aidash_application::usage::AlwaysAdmit),
		provider_credentials: None,
		tenant_secrets: None,
		plan_connections: None,
		plan_connections_enabled: false,
		provider_key_material_reader: None,
		capability_issuer: None,
	}
}
