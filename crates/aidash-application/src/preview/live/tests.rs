//! Live preview limits, startup and termination.
use super::*;
use std::collections::BTreeMap;

#[derive(Default)]
struct Fake {
	domain: Option<String>,
	ceilings: Limits,
	tenant: Option<Limits>,
	current: Option<LivePreview>,
	rows: BTreeMap<Uuid, LivePreview>,
}

impl Fake {
	fn new() -> Self {
		Self {
			domain: Some("aidashusercontent.app".into()),
			ceilings: Limits::default(),
			..Self::default()
		}
	}
}

#[async_trait]
impl Scope for Fake {
	fn domain(&self) -> Option<&str> {
		self.domain.as_deref()
	}
	fn ceilings(&self) -> Limits {
		self.ceilings.clone()
	}
	async fn tenant_limits(&mut self, _tenant: &str) -> Result<Option<Limits>> {
		Ok(self.tenant.clone())
	}
	async fn run(&mut self) -> Result<RunFacts> {
		Ok(RunFacts {
			id: Uuid::now_v7(),
			tenant: "acme".into(),
		})
	}
	async fn current(&mut self, _run: Uuid) -> Result<Option<LivePreview>> {
		Ok(self.current.clone())
	}
	async fn by_id(&mut self, id: Uuid) -> Result<Option<LivePreview>> {
		Ok(self.rows.get(&id).cloned())
	}
	async fn insert(&mut self, preview: &LivePreview) -> Result<()> {
		self.rows.insert(preview.id, preview.clone());
		self.current = Some(preview.clone());
		Ok(())
	}
	async fn save(&mut self, preview: &LivePreview) -> Result<()> {
		self.rows.insert(preview.id, preview.clone());
		self.current = Some(preview.clone());
		Ok(())
	}
	async fn unsettled(&mut self) -> Result<Vec<LivePreview>> {
		Ok(self
			.rows
			.values()
			.filter(|preview| !preview.state.is_stopped() || preview.needs_termination())
			.cloned()
			.collect())
	}
}

fn request(command: &str, port: u16) -> Start {
	Start {
		command: command.into(),
		port,
	}
}

#[tokio::test]
async fn a_deployment_without_a_preview_domain_starts_nothing() {
	let mut scope = Fake::new();
	scope.domain = None;
	assert!(matches!(
		start(&mut scope, &request("python3 -m http.server 3000", 3000))
			.await
			.unwrap_err(),
		Error::Conflict(message) if message == "PREVIEW_UNAVAILABLE"
	));
}

#[tokio::test]
async fn a_tenant_lowers_the_limits_and_a_start_uses_them() {
	let mut scope = Fake::new();
	scope.tenant = Some(Limits::new(300, 1800).unwrap());
	let preview = start(&mut scope, &request("python3 -m http.server 3000", 3000))
		.await
		.unwrap();
	assert_eq!(preview.limits, Limits::new(300, 1800).unwrap());
	assert_eq!(
		preview.expires_at,
		preview.started_at + chrono::TimeDelta::seconds(1800)
	);
}

#[tokio::test]
async fn a_started_preview_reports_where_it_serves_but_never_the_address() {
	let mut scope = Fake::new();
	let preview = start(&mut scope, &request("python3 -m http.server 3000", 3000))
		.await
		.unwrap();
	let started = serving(
		&mut scope,
		preview,
		"10.0.0.7",
		"listening on 3000",
		Utc::now(),
	)
	.await
	.unwrap();
	assert_eq!(started.status.state, State::Running);
	assert_eq!(
		serde_json::to_value(&started.status)
			.unwrap()
			.get("pod_host"),
		None
	);
	assert_eq!(started.startup_log, "listening on 3000");
}

#[tokio::test]
async fn the_idle_limit_stops_it_and_waits_for_termination_proof() {
	let mut scope = Fake::new();
	scope.ceilings = Limits::new(60, 600).unwrap();
	let preview = start(&mut scope, &request("python3 -m http.server 3000", 3000))
		.await
		.unwrap();
	let started_at = Utc::now();
	serving(&mut scope, preview.clone(), "10.0.0.7", "", started_at)
		.await
		.unwrap();
	let sweep = sweep(&mut scope, started_at + chrono::TimeDelta::seconds(61))
		.await
		.unwrap();
	assert_eq!(sweep.terminate.len(), 1);
	assert_eq!(sweep.terminate[0].stop_reason, Some(StopReason::Idle));
	assert!(settled(&mut scope, preview.id).await.unwrap().is_some());
}

#[tokio::test]
async fn the_absolute_limit_stops_it_even_with_fresh_requests() {
	let mut scope = Fake::new();
	scope.ceilings = Limits::new(300, 600).unwrap();
	let preview = start(&mut scope, &request("python3 -m http.server 3000", 3000))
		.await
		.unwrap();
	let started_at = Utc::now();
	serving(&mut scope, preview.clone(), "10.0.0.7", "", started_at)
		.await
		.unwrap();
	// A fresh request within the idle limit, but past the absolute deadline.
	scope
		.rows
		.get_mut(&preview.id)
		.unwrap()
		.touched(started_at + chrono::TimeDelta::seconds(599));
	let sweep = sweep(&mut scope, started_at + chrono::TimeDelta::seconds(601))
		.await
		.unwrap();
	assert_eq!(sweep.terminate[0].stop_reason, Some(StopReason::Absolute));
}

#[tokio::test]
async fn stopping_is_idempotent_and_keeps_the_first_reason() {
	let mut scope = Fake::new();
	let preview = start(&mut scope, &request("python3 -m http.server 3000", 3000))
		.await
		.unwrap();
	serving(&mut scope, preview.clone(), "10.0.0.7", "", Utc::now())
		.await
		.unwrap();
	let stopped = stop(&mut scope, preview.id, StopReason::RunCancelled)
		.await
		.unwrap();
	assert_eq!(stopped.stop_reason, Some(StopReason::RunCancelled));
	let again = stop(&mut scope, preview.id, StopReason::Idle)
		.await
		.unwrap();
	assert_eq!(again.stop_reason, Some(StopReason::RunCancelled));
}
