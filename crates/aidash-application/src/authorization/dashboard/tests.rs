use super::*;
use crate::ports::authorization::dashboard::{AccountPolicy, StatusRecovery};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::{collections::BTreeSet, sync::Mutex};

#[derive(Clone, Copy)]
enum ProviderResult {
	Enabled,
	Disabled,
	Offline,
}
struct State {
	account: Account,
	now: DateTime<Utc>,
	trace: Vec<String>,
	recorded: Vec<DateTime<Utc>>,
	waiting: Vec<Uuid>,
	permitted: BTreeSet<Uuid>,
	notified: usize,
}
struct Scope {
	state: Arc<Mutex<State>>,
	configured: bool,
	google: bool,
	result: ProviderResult,
	newer_validity: bool,
	delay_seconds: i64,
}
#[fixture]
fn scope() -> Scope {
	let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
	let account = Account {
		id: Uuid::new_v4(),
		issuer: "issuer".into(),
		subject: "subject".into(),
		last_valid_at: Some(now - Duration::seconds(400)),
		disabled_at: None,
	};
	Scope {
		state: Arc::new(Mutex::new(State {
			account,
			now,
			trace: vec![],
			recorded: vec![],
			waiting: vec![],
			permitted: BTreeSet::new(),
			notified: 0,
		})),
		configured: true,
		google: false,
		result: ProviderResult::Enabled,
		newer_validity: false,
		delay_seconds: 0,
	}
}
#[async_trait]
impl AccountStatus for Scope {
	async fn enabled(&self, subject: &str) -> Result<bool> {
		let mut state = self.state.lock().unwrap();
		state.trace.push(format!("lookup:{subject}"));
		state.now += Duration::seconds(self.delay_seconds);
		match self.result {
			ProviderResult::Enabled => Ok(true),
			ProviderResult::Disabled => Ok(false),
			ProviderResult::Offline => Err(Error::External("provider unavailable".into())),
		}
	}
}
#[async_trait]
impl Accounts for Scope {
	fn policy(&self) -> Option<AccountPolicy> {
		self.configured.then(|| AccountPolicy {
			issuer: "issuer".into(),
			google: self.google,
		})
	}
	fn now(&self) -> DateTime<Utc> {
		self.state.lock().unwrap().now
	}
	async fn active(&self) -> Result<Vec<Account>> {
		Ok(vec![self.state.lock().unwrap().account.clone()])
	}
	async fn record_valid(&self, _: Uuid, started: DateTime<Utc>) -> Result<()> {
		let mut state = self.state.lock().unwrap();
		state.trace.push("record".into());
		state.recorded.push(started);
		Ok(())
	}
	async fn disable_if_current(&self, _: Uuid, _: Option<DateTime<Utc>>) -> Result<bool> {
		self.state
			.lock()
			.unwrap()
			.trace
			.push("disable-current".into());
		Ok(!self.newer_validity)
	}
	async fn mark_waiting_disabled(&self, _: Uuid) -> Result<()> {
		self.state
			.lock()
			.unwrap()
			.trace
			.push("mark-disabled".into());
		Ok(())
	}
	async fn recovery(&self) -> Result<Box<dyn StatusRecovery>> {
		Ok(Box::new(Recovery(self.state.clone())))
	}
}
struct Recovery(Arc<Mutex<State>>);
#[async_trait]
impl StatusRecovery for Recovery {
	async fn account(&mut self, _: Uuid) -> Result<Account> {
		Ok(self.0.lock().unwrap().account.clone())
	}
	async fn waiting(&mut self, _: Uuid) -> Result<Vec<Uuid>> {
		Ok(self.0.lock().unwrap().waiting.clone())
	}
	async fn resume_original(&mut self, _: Uuid, run: Uuid) -> Result<bool> {
		let mut state = self.0.lock().unwrap();
		state.trace.push(format!("resume:{run}"));
		Ok(state.permitted.contains(&run))
	}
	async fn restore(&mut self, _: Uuid, started: DateTime<Utc>) -> Result<()> {
		let mut state = self.0.lock().unwrap();
		state.trace.push("restore".into());
		state.recorded.push(started);
		Ok(())
	}

	fn notify(&self) {
		self.0.lock().unwrap().notified += 1;
	}
}
fn authority(scope: Arc<Scope>) -> DashboardAuthority {
	DashboardAuthority {
		accounts: scope.clone(),
		status: scope,
	}
}

#[rstest]
#[tokio::test]
async fn slow_lookup_records_its_start_and_does_not_extend_validity(mut scope: Scope) {
	scope.delay_seconds = 600;
	let account = scope.state.lock().unwrap().account.clone();
	let started = scope.state.lock().unwrap().now;
	let scope = Arc::new(scope);
	authority(scope.clone())
		.account_valid(&account)
		.await
		.unwrap();
	assert_eq!(scope.state.lock().unwrap().recorded, [started]);
	assert_eq!(
		scope.state.lock().unwrap().trace,
		["lookup:subject", "record"]
	);
}

#[rstest]
#[case(299, false, false)]
#[case(300, true, false)]
#[case(899, true, false)]
#[case(900, true, true)]
#[tokio::test]
async fn provider_failure_retains_exact_freshness_and_hard_deadlines(
	mut scope: Scope,
	#[case] age: i64,
	#[case] checked: bool,
	#[case] unavailable: bool,
) {
	scope.result = ProviderResult::Offline;
	let mut account = scope.state.lock().unwrap().account.clone();
	account.last_valid_at = Some(scope.state.lock().unwrap().now - Duration::seconds(age));
	let scope = Arc::new(scope);
	let result = authority(scope.clone()).account_valid(&account).await;
	if unavailable {
		assert!(matches!(result, Err(Error::IdentityStatusUnavailable)));
	} else {
		result.unwrap();
	}
	let expected = if checked {
		vec!["lookup:subject"]
	} else {
		vec![]
	};
	assert_eq!(scope.state.lock().unwrap().trace, expected);
}

#[rstest]
#[tokio::test]
async fn old_issuer_refresh_disables_without_contacting_the_new_provider(scope: Scope) {
	let mut account = scope.state.lock().unwrap().account.clone();
	account.issuer = "old".into();
	let scope = Arc::new(scope);
	authority(scope.clone()).refresh(account).await;
	assert_eq!(
		scope.state.lock().unwrap().trace,
		["disable-current", "mark-disabled"]
	);
}

#[rstest]
#[tokio::test]
async fn recovery_rechecks_original_subjects_and_notifies_only_committed_resumes(scope: Scope) {
	let allowed = Uuid::new_v4();
	let revoked = Uuid::new_v4();
	{
		let mut state = scope.state.lock().unwrap();
		state.account.last_valid_at = Some(state.now);
		state.waiting = vec![revoked, allowed];
		state.permitted.insert(allowed);
	}
	let id = scope.state.lock().unwrap().account.id;
	let scope = Arc::new(scope);
	authority(scope.clone()).resume(id).await.unwrap();
	assert_eq!(
		scope.state.lock().unwrap().trace,
		[format!("resume:{revoked}"), format!("resume:{allowed}")]
	);
	assert_eq!(scope.state.lock().unwrap().notified, 1);
}

#[rstest]
#[tokio::test]
async fn google_refresh_keeps_existing_provider_independent_behavior(mut scope: Scope) {
	scope.google = true;
	let account = scope.state.lock().unwrap().account.clone();
	let scope = Arc::new(scope);
	authority(scope.clone()).refresh(account).await;
	assert_eq!(scope.state.lock().unwrap().trace.len(), 0);
}

#[rstest]
#[tokio::test]
async fn restoring_a_disabled_identity_checks_provider_before_the_same_scope_write(
	mut scope: Scope,
) {
	scope.delay_seconds = 600;
	let (id, started) = {
		let mut state = scope.state.lock().unwrap();
		state.account.disabled_at = Some(state.now);
		(state.account.id, state.now)
	};
	let scope = Arc::new(scope);
	authority(scope.clone()).restore(id).await.unwrap();
	assert_eq!(
		scope.state.lock().unwrap().trace,
		["lookup:subject", "restore"]
	);
	assert_eq!(scope.state.lock().unwrap().recorded, [started]);
}

#[rstest]
#[tokio::test]
async fn active_identity_cannot_be_restored_without_a_disabled_state(scope: Scope) {
	let id = scope.state.lock().unwrap().account.id;
	let scope = Arc::new(scope);
	assert!(
		matches!(authority(scope.clone()).restore(id).await, Err(Error::Conflict(reason)) if reason == "identity is not disabled for the configured issuer")
	);
	assert_eq!(scope.state.lock().unwrap().trace.len(), 0);
}

#[async_trait]
impl crate::ports::authorization::dashboard::LoginAccounts for Recovery {
	async fn register(&mut self, issuer: &str, subject: &str) -> Result<Account> {
		let mut state = self.0.lock().unwrap();
		state.trace.push(format!("register:{issuer}:{subject}"));
		Ok(state.account.clone())
	}
}

#[rstest]
#[tokio::test]
async fn rejected_login_never_registers_an_account(mut scope: Scope) {
	scope.result = ProviderResult::Disabled;
	let scope = Arc::new(scope);
	let mut login = Recovery(scope.state.clone());
	assert!(matches!(
		authority(scope.clone())
			.admit_login(&mut login, "subject")
			.await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.state.lock().unwrap().trace, ["lookup:subject"]);
}

#[rstest]
#[tokio::test]
async fn login_cannot_reactivate_a_persisted_disabled_account(scope: Scope) {
	{
		let mut state = scope.state.lock().unwrap();
		state.account.disabled_at = Some(state.now);
	}
	let scope = Arc::new(scope);
	let mut login = Recovery(scope.state.clone());
	assert!(matches!(
		authority(scope.clone())
			.admit_login(&mut login, "subject")
			.await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		scope.state.lock().unwrap().trace,
		["lookup:subject", "register:issuer:subject"]
	);
}

#[async_trait]
impl crate::ports::authorization::dashboard::BackchannelLogout for Recovery {
	fn now(&self) -> DateTime<Utc> {
		self.0.lock().unwrap().now
	}
	async fn verified_claims(&self, _: &str) -> Result<serde_json::Value> {
		self.0.lock().unwrap().trace.push("verify-signature".into());
		// A valid signed envelope still cannot establish logout semantics.
		Ok(serde_json::json!({"sub":"subject","jti":"replay","nonce":"login-nonce"}))
	}
	async fn revoke(&self, _: &aidash_domain::identity::dashboard::LogoutClaims) -> Result<()> {
		self.0.lock().unwrap().trace.push("revoke".into());
		Ok(())
	}
}

#[rstest]
#[tokio::test]
async fn signed_login_token_cannot_revoke_sessions_through_backchannel(scope: Scope) {
	let logout = Recovery(scope.state.clone());
	assert!(
		matches!(backchannel_logout(&logout, "signed").await, Err(Error::Domain(aidash_domain::Error::Invalid(reason))) if reason == "invalid logout token claims")
	);
	assert_eq!(scope.state.lock().unwrap().trace, ["verify-signature"]);
}
