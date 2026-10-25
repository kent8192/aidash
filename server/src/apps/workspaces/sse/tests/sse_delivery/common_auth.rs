//! Remote validity must cover frame handoff and idle streams, beyond admission.
use super::*;
use aidash_application::ports::{
	accounts::Authority,
	authorization::dashboard::{AccountPolicy, Issuer},
};
use aidash_domain::accounts::DurableOrigin;
use std::sync::atomic::{AtomicU8, Ordering};

struct SharedAuthority {
	source: Uuid,
	state: AtomicU8,
}
#[async_trait::async_trait]
impl Authority for SharedAuthority {
	async fn validate_interactive(&self, _: Uuid, _: Uuid) -> aidash_application::Result<()> {
		panic!("a durable Subject Credential must not depend on a browser session");
	}
	async fn validate_origin(&self, source: Uuid) -> aidash_application::Result<()> {
		if source != self.source {
			return Err(aidash_application::Error::Forbidden);
		}
		match self.state.load(Ordering::SeqCst) {
			0 => Ok(()),
			1 => Err(aidash_application::Error::Forbidden),
			_ => Err(aidash_application::Error::IdentityStatusUnavailable),
		}
	}
	async fn origin_binding(&self, source: Uuid) -> aidash_application::Result<DurableOrigin> {
		self.validate_origin(source).await?;
		Ok(DurableOrigin {
			identity_id: source,
			account_id: None,
			suspension_epoch: None,
			source_disable_epoch: None,
		})
	}
	async fn validate_bound_origin(
		&self,
		origin: &DurableOrigin,
	) -> aidash_application::Result<()> {
		self.validate_origin(origin.identity_id).await
	}
}

#[rstest::rstest]
#[case::denied(1, 403)]
#[case::unavailable(2, 503)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shared_authority_stops_buffered_frames_and_idle_streams(
	#[case] stopped: u8,
	#[case] reconnect_status: u16,
	#[future(awt)]
	#[from(sse_fixture)]
	#[with(Duration::from_secs(60), Duration::from_secs(30), 128)]
	root: Arc<Fixture>,
) {
	let mut fixture = (*root).clone();
	let source = Uuid::new_v4();
	let issuer = "https://upstream.example.test";
	for insert in [
		Query::insert()
			.into_table(Alias::new("dashboard_identities"))
			.columns(["id", "issuer", "subject", "last_valid_at"].map(Alias::new))
			.values_panic([
				reinhardt::query::IntoValue::into_value(source),
				issuer.into(),
				"original-person".into(),
				chrono::Utc::now().into(),
			])
			.to_string(PostgresQueryBuilder),
		Query::insert()
			.into_table(Alias::new("dashboard_mappings"))
			.columns(["id", "identity_id", "tenant", "subject", "credential_id"].map(Alias::new))
			.values_panic([
				reinhardt::query::IntoValue::into_value(Uuid::new_v4()),
				source.into(),
				"acme".into(),
				"alice".into(),
				fixture.credential.into(),
			])
			.to_string(PostgresQueryBuilder),
	] {
		sqlx::query(&insert)
			.execute(fixture.f.store.pool.fixture_driver())
			.await
			.unwrap();
	}
	let authority = Arc::new(SharedAuthority {
		source,
		state: AtomicU8::new(0),
	});
	fixture.f.store = fixture
		.f
		.store
		.clone()
		.with_dashboard_policy(Some(AccountPolicy {
			issuers: Arc::new(Issuer(issuer.into())),
			authority: Some(authority.clone()),
			personal_tenants: false,
			demo_data: false,
		}));
	fixture.app = common::application_with_event_streams(
		fixture.f.clone(),
		Default::default(),
		fixture.service.clone(),
	)
	.await;

	let workspace = fixture.workspaces[0];
	let first = fixture.emit(workspace, 1).await;
	let buffered = fixture.emit(workspace, 2).await;
	let mut frames = fixture
		.stream(first.sequence - 1, Some(workspace), None)
		.await;
	assert_eq!(frame(&mut frames).await.0, first.sequence);
	// Consuming the first frame loads the page containing the second, without
	// admitting that buffered event for transmission yet.
	let mut idle = fixture.open(-1, Some(fixture.workspaces[1]), None).await;
	assert_eq!(idle.status, 200);
	let later = fixture.emit(workspace, 3).await;
	// Switch immediately before polling the pending page; no new database read
	// or notification is needed to reject that frame at its handoff boundary.
	authority.state.store(stopped, Ordering::SeqCst);
	let remaining = tokio::time::timeout(Duration::from_secs(3), async {
		let mut output = Vec::new();
		while let Some(chunk) = frames.next().await {
			output.extend_from_slice(&chunk.unwrap());
		}
		output
	})
	.await
	.expect("remote denial or outage must close a polled stream");
	assert!(!String::from_utf8_lossy(&remaining).contains("event: mesh"));
	// Keep the idle body unpolled: authentication monitoring must independently
	// close its registration rather than waiting for another event or a poll.
	until(|| fixture.service.snapshot().registered_scopes == 0).await;
	let mut idle_frames = idle.take_stream_body().unwrap();
	let idle_output = tokio::time::timeout(Duration::from_secs(1), async {
		let mut output = Vec::new();
		while let Some(chunk) = idle_frames.next().await {
			output.extend_from_slice(&chunk.unwrap());
		}
		output
	})
	.await
	.unwrap();
	assert!(!String::from_utf8_lossy(&idle_output).contains("event: mesh"));
	assert_eq!(
		fixture
			.open(first.sequence, Some(workspace), None)
			.await
			.status,
		reconnect_status
	);

	// Restoring only the remote response permits a newly checked connection and
	// replays the unsent event: no local credential/policy mutation caused denial.
	authority.state.store(0, Ordering::SeqCst);
	let mut reconnected = fixture.stream(first.sequence, Some(workspace), None).await;
	assert_eq!(frame(&mut reconnected).await.0, buffered.sequence);
	assert_eq!(frame(&mut reconnected).await.0, later.sequence);
	drop((frames, idle_frames, idle, reconnected));
	fixture.finish().await;
}
