use super::*;
use crate::ports::registry::workbench::sandbox::SandboxScope;
use aidash_domain::registry::workbench::Draft;
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use rstest::rstest;
use serde_json::json;
use std::sync::Mutex;
#[derive(Default)]
struct State {
	active: bool,
	committed: bool,
	calls: Vec<&'static str>,
	status: Option<String>,
	limits: Option<TestLimits>,
}
struct Repository {
	principal: Principal,
	state: Mutex<State>,
	allowed: bool,
	field_valid: bool,
	status: &'static str,
	pause: bool,
}
impl Repository {
	fn new() -> Self {
		Self {
			principal: Principal::Subject {
				tenant: "tenant".into(),
				subject: "reader".into(),
			},
			state: Mutex::new(State::default()),
			allowed: true,
			field_valid: true,
			status: "running",
			pause: false,
		}
	}
}
struct Scope<'a> {
	repository: &'a Repository,
	status: Option<String>,
	limits: Option<TestLimits>,
}
impl Drop for Scope<'_> {
	fn drop(&mut self) {
		self.repository.state.lock().unwrap().active = false;
	}
}
fn limits() -> TestLimits {
	TestLimits {
		tenant: "tenant".into(),
		max_input_bytes: 1024,
		max_output_tokens: 256,
		max_total_tokens: 1024,
		max_steps: 2,
		max_duration_secs: 30,
		max_concurrent: 2,
		payload_days: 3,
		incident_evidence_days: 5,
	}
}
fn session(status: &str) -> TestSession {
	TestSession {
		id: Uuid::from_u128(9),
		draft_id: Uuid::from_u128(1),
		tenant: "tenant".into(),
		revision: 7,
		status: status.into(),
		scenario: json!({}),
		conversation: Some(json!([])),
		tool_calls: Some(json!([])),
		usage: json!({}),
		error: None,
		created_at: Utc.timestamp_opt(100, 0).unwrap(),
		updated_at: Utc.timestamp_opt(100, 0).unwrap(),
		expires_at: Utc.timestamp_opt(200, 0).unwrap(),
		expired_at: None,
	}
}
#[async_trait]
impl SandboxRepository for Repository {
	fn principal(&self) -> Principal {
		self.principal.clone()
	}
	fn validate_limit_fields(&self, _: &TestLimits) -> Result<()> {
		self.state.lock().unwrap().calls.push("field_validation");
		if self.field_valid {
			Ok(())
		} else {
			Err(Error::Invalid("native field diagnostics".into()))
		}
	}
	async fn begin(&self) -> Result<Box<dyn SandboxScope + '_>> {
		let mut s = self.state.lock().unwrap();
		s.active = true;
		s.calls.push("begin");
		Ok(Box::new(Scope {
			repository: self,
			status: None,
			limits: None,
		}))
	}
	async fn purge(&self) -> Result<u64> {
		self.state.lock().unwrap().calls.push("purge");
		Ok(3)
	}
}
#[async_trait]
impl SandboxScope for Scope<'_> {
	async fn draft(&mut self, id: Uuid, lock: bool) -> Result<Draft> {
		assert_eq!(id, Uuid::from_u128(1));
		assert!(!lock);
		self.repository.state.lock().unwrap().calls.push("draft");
		Ok(Draft {
			id,
			tenant: "tenant".into(),
			owner: "owner".into(),
			revision: 7,
			entry: json!({}),
			documents: json!([]),
			release_notes: String::new(),
			source_id: None,
			source_version: None,
			archived: false,
			updated_at: Utc.timestamp_opt(100, 0).unwrap(),
		})
	}
	async fn authorize_draft(&mut self, _: &Draft, action: &str, shares: bool) -> Result<()> {
		assert!(shares);
		assert!(matches!(action, "agent_draft.test" | "agent_draft.read"));
		self.repository
			.state
			.lock()
			.unwrap()
			.calls
			.push(if action == "agent_draft.test" {
				"test_authority"
			} else {
				"read_authority"
			});
		if self.repository.pause {
			std::future::pending::<()>().await;
		}
		if self.repository.allowed {
			Ok(())
		} else {
			Err(Error::Forbidden)
		}
	}
	async fn limits(&mut self, tenant: &str) -> Result<TestLimits> {
		assert_eq!(tenant, "tenant");
		self.repository
			.state
			.lock()
			.unwrap()
			.calls
			.push("limits_locked");
		Ok(limits())
	}
	async fn save_limits(&mut self, input: &TestLimits) -> Result<()> {
		self.repository
			.state
			.lock()
			.unwrap()
			.calls
			.push("save_limits");
		self.limits = Some(input.clone());
		Ok(())
	}
	async fn session(&mut self, id: Uuid, lock: bool) -> Result<TestSession> {
		assert_eq!(id, Uuid::from_u128(9));
		assert!(!lock);
		self.repository.state.lock().unwrap().calls.push("session");
		Ok(session(self.repository.status))
	}
	async fn sessions(&mut self, id: Uuid) -> Result<Vec<TestSession>> {
		assert_eq!(id, Uuid::from_u128(1));
		self.repository.state.lock().unwrap().calls.push("sessions");
		Ok(vec![session("completed")])
	}
	async fn stop(&mut self, id: Uuid) -> Result<TestSession> {
		assert_eq!(id, Uuid::from_u128(9));
		self.repository.state.lock().unwrap().calls.push("stop");
		let status = if self.repository.status == "running" {
			"stopped"
		} else {
			self.repository.status
		};
		self.status = Some(status.into());
		Ok(session(status))
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		let mut s = self.repository.state.lock().unwrap();
		s.committed = true;
		s.status = self.status.clone();
		s.limits = self.limits.clone();
		Ok(())
	}
}
#[rstest]
#[case(true)]
#[case(false)]
#[tokio::test]
async fn tenant_limits_require_current_draft_test_authority(#[case] allowed: bool) {
	let mut r = Repository::new();
	r.allowed = allowed;
	let result = get_limits(&r, Uuid::from_u128(1)).await;
	assert_eq!(result.is_ok(), allowed);
	let s = r.state.lock().unwrap();
	assert!(!s.active);
	assert_eq!(s.committed, allowed);
	if allowed {
		assert_eq!(result.unwrap().max_concurrent, 2);
		assert_eq!(
			s.calls,
			vec!["begin", "draft", "test_authority", "limits_locked"]
		);
	} else {
		assert!(matches!(result, Err(Error::Forbidden)));
		assert!(!s.calls.contains(&"limits_locked"));
	}
}
#[rstest]
#[case("subject")]
#[case("tenant")]
#[case("fields")]
#[case("budget")]
#[tokio::test]
async fn invalid_or_unauthorized_limit_changes_precede_any_transaction(#[case] boundary: &str) {
	let mut r = Repository::new();
	r.principal = Principal::Operator;
	let mut input = limits();
	match boundary {
		"subject" => {
			r.principal = Principal::Subject {
				tenant: "tenant".into(),
				subject: "reader".into(),
			}
		}
		"tenant" => input.tenant = "other".into(),
		"fields" => r.field_valid = false,
		_ => input.max_total_tokens = 64,
	}
	assert!(set_limits(&r, "tenant".into(), input).await.is_err());
	let s = r.state.lock().unwrap();
	assert!(!s.calls.contains(&"begin"));
	assert!(s.limits.is_none());
	if matches!(boundary, "subject" | "tenant") {
		assert!(!s.calls.contains(&"field_validation"));
	}
}
#[rstest]
#[tokio::test]
async fn operator_limit_changes_commit_the_exact_validated_configuration() {
	let mut r = Repository::new();
	r.principal = Principal::Operator;
	let result = set_limits(&r, "tenant".into(), limits()).await.unwrap();
	assert_eq!(
		serde_json::to_value(result).unwrap(),
		serde_json::to_value(limits()).unwrap()
	);
	let s = r.state.lock().unwrap();
	assert!(!s.active && s.committed);
	assert_eq!(s.calls, vec!["field_validation", "begin", "save_limits"]);
}
#[rstest]
#[case(true)]
#[case(false)]
#[tokio::test]
async fn session_listing_keeps_payload_cleanup_before_authorized_draft_read(#[case] allowed: bool) {
	let mut r = Repository::new();
	r.allowed = allowed;
	let result = sessions(&r, Uuid::from_u128(1)).await;
	assert_eq!(result.is_ok(), allowed);
	let s = r.state.lock().unwrap();
	assert_eq!(s.calls[0], "purge");
	assert_eq!(s.calls[3], "read_authority");
	assert_eq!(s.calls.contains(&"sessions"), allowed);
	assert_eq!(s.committed, allowed);
}
#[rstest]
#[case("running", "stopped")]
#[case("completed", "completed")]
#[case("outcome_unknown", "outcome_unknown")]
#[tokio::test]
async fn stop_preserves_terminal_outcomes_and_only_releases_running_sessions(
	#[case] prior: &'static str,
	#[case] expected: &str,
) {
	let mut r = Repository::new();
	r.status = prior;
	let result = stop(&r, Uuid::from_u128(9)).await.unwrap();
	assert_eq!(result.status, expected);
	let s = r.state.lock().unwrap();
	assert!(!s.active && s.committed);
	assert_eq!(s.status.as_deref(), Some(expected));
	assert_eq!(
		s.calls,
		vec!["begin", "session", "draft", "test_authority", "stop"]
	);
}
#[rstest]
#[tokio::test]
async fn stop_denial_cannot_change_the_session_or_commit() {
	let mut r = Repository::new();
	r.allowed = false;
	assert!(matches!(
		stop(&r, Uuid::from_u128(9)).await,
		Err(Error::Forbidden)
	));
	let s = r.state.lock().unwrap();
	assert!(!s.active && !s.committed);
	assert!(s.status.is_none());
	assert!(!s.calls.contains(&"stop"));
}
#[rstest]
#[tokio::test]
async fn cancelling_authority_releases_the_uncommitted_session_scope() {
	let mut r = Repository::new();
	r.pause = true;
	let mut future = Box::pin(stop(&r, Uuid::from_u128(9)));
	assert!(futures_util::poll!(&mut future).is_pending());
	assert!(r.state.lock().unwrap().active);
	drop(future);
	let s = r.state.lock().unwrap();
	assert!(!s.active && !s.committed);
	assert!(s.status.is_none());
}
