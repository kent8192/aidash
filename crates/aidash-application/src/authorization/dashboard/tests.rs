use super::*;
use crate::ports::authorization::dashboard::{AccountPolicy, StatusRecovery};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::{collections::BTreeSet, sync::Mutex};

#[derive(Clone, Copy)]
enum ProviderResult {
	Enabled,
	Revoked,
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
#[derive(Clone)]
struct Scope {
	state: Arc<Mutex<State>>,
	configured: bool,
	google: bool,
	tenant_bindings: Option<std::collections::BTreeMap<String, String>>,
	gcip_providers: std::collections::BTreeMap<String, Vec<String>>,
	result: ProviderResult,
	newer_validity: bool,
	delay_seconds: i64,
	valid_since: Option<DateTime<Utc>>,
}
#[fixture]
fn scope() -> Scope {
	let now = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
	let account = Account {
		id: Uuid::new_v4(),
		issuer: "issuer".into(),
		subject: "subject".into(),
		gcip_tenant: None,
		valid_since: None,
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
		tenant_bindings: None,
		gcip_providers: Default::default(),
		result: ProviderResult::Enabled,
		newer_validity: false,
		delay_seconds: 0,
		valid_since: None,
	}
}
#[async_trait]
impl AccountStatus for Scope {
	async fn lookup(
		&self,
		subject: &str,
		gcip_tenant: Option<&str>,
	) -> Result<aidash_domain::identity::dashboard::AccountState> {
		let mut state = self.state.lock().unwrap();
		state.trace.push(format!("lookup:{subject}"));
		assert_eq!(gcip_tenant, state.account.gcip_tenant.as_deref());
		state.now += Duration::seconds(self.delay_seconds);
		match self.result {
			ProviderResult::Enabled => Ok(aidash_domain::identity::dashboard::AccountState {
				disabled: false,
				valid_since: self.valid_since,
			}),
			ProviderResult::Disabled => Ok(aidash_domain::identity::dashboard::AccountState {
				disabled: true,
				valid_since: self.valid_since,
			}),
			ProviderResult::Revoked => Ok(aidash_domain::identity::dashboard::AccountState {
				disabled: false,
				valid_since: Some(state.now - Duration::seconds(60)),
			}),
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
			tenant_bindings: self.tenant_bindings.clone(),
			gcip_providers: self.gcip_providers.clone(),
		})
	}
	fn now(&self) -> DateTime<Utc> {
		self.state.lock().unwrap().now
	}
	async fn active(&self) -> Result<Vec<Account>> {
		Ok(vec![self.state.lock().unwrap().account.clone()])
	}
	async fn record_valid(
		&self,
		_: Uuid,
		started: DateTime<Utc>,
		valid_since: Option<DateTime<Utc>>,
	) -> Result<()> {
		let mut state = self.state.lock().unwrap();
		state.trace.push("record".into());
		state.recorded.push(started);
		if let Some(since) = valid_since {
			state
				.trace
				.push(format!("revoke-before:{}", since.timestamp()));
		}
		Ok(())
	}
	async fn disable_if_current(&self, _: Uuid, _: Option<DateTime<Utc>>) -> Result<bool> {
		let mut state = self.state.lock().unwrap();
		state.trace.push("disable-current".into());
		if !self.newer_validity {
			state.account.disabled_at = Some(state.now);
		}
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
	async fn find(&mut self, _: &str, _: &SignIn) -> Result<Option<Account>> {
		Ok(None)
	}
	async fn register(&mut self, issuer: &str, sign_in: &SignIn) -> Result<Account> {
		let mut state = self.0.lock().unwrap();
		state
			.trace
			.push(format!("register:{issuer}:{}", sign_in.subject));
		Ok(state.account.clone())
	}
}

struct ExistingLogin(Arc<Mutex<State>>);
#[async_trait]
impl crate::ports::authorization::dashboard::LoginAccounts for ExistingLogin {
	async fn find(&mut self, issuer: &str, sign_in: &SignIn) -> Result<Option<Account>> {
		let mut state = self.0.lock().unwrap();
		state.trace.push("find".into());
		Ok((state.account.issuer == issuer
			&& state.account.subject == sign_in.subject
			&& state.account.gcip_tenant == sign_in.gcip_tenant)
			.then(|| state.account.clone()))
	}
	async fn register(&mut self, _: &str, _: &SignIn) -> Result<Account> {
		let mut state = self.0.lock().unwrap();
		state.trace.push("register".into());
		Ok(state.account.clone())
	}
}

#[rstest]
#[tokio::test]
async fn gcip_login_rejects_a_signed_provider_outside_the_pool_allowlist(mut scope: Scope) {
	scope.tenant_bindings = Some([("pool-a".into(), "acme".into())].into());
	scope.gcip_providers = [("pool-a".into(), vec!["google.com".into()])].into();
	scope.state.lock().unwrap().account.gcip_tenant = Some("pool-a".into());
	let sign_in = SignIn {
		gcip_tenant: Some("pool-a".into()),
		gcip_provider: Some("password".into()),
		..sign_in()
	};
	let mut login = ExistingLogin(scope.state.clone());
	assert!(matches!(
		authority(Arc::new(scope.clone()))
			.admit_login(&mut login, &sign_in)
			.await,
		Err(Error::Forbidden)
	));
	assert!(scope.state.lock().unwrap().trace.is_empty());
}

#[rstest]
#[tokio::test]
async fn removed_binding_login_persists_disable_across_binding_restoration(mut scope: Scope) {
	scope.tenant_bindings = Some(Default::default());
	scope.state.lock().unwrap().account.gcip_tenant = Some("pool-a".into());
	let sign_in = SignIn {
		gcip_tenant: Some("pool-a".into()),
		gcip_provider: Some("oidc.company".into()),
		..sign_in()
	};
	let mut login = ExistingLogin(scope.state.clone());
	assert!(matches!(
		authority(Arc::new(scope.clone()))
			.admit_login(&mut login, &sign_in)
			.await,
		Err(Error::Forbidden)
	));
	{
		let state = scope.state.lock().unwrap();
		assert!(state.account.disabled_at.is_some());
		assert_eq!(state.trace, ["find", "disable-current", "mark-disabled"]);
	}
	scope.tenant_bindings = Some([("pool-a".into(), "acme".into())].into());
	scope.gcip_providers = [("pool-a".into(), vec!["oidc.company".into()])].into();
	assert!(matches!(
		authority(Arc::new(scope))
			.admit_login(&mut login, &sign_in)
			.await,
		Err(Error::Forbidden)
	));
}

#[rstest]
#[tokio::test]
async fn removed_binding_unknown_login_never_registers_or_contacts_provider(mut scope: Scope) {
	scope.tenant_bindings = Some(Default::default());
	let sign_in = SignIn {
		gcip_tenant: Some("pool-a".into()),
		..sign_in()
	};
	let mut login = Recovery(scope.state.clone());
	assert!(matches!(
		authority(Arc::new(scope.clone()))
			.admit_login(&mut login, &sign_in)
			.await,
		Err(Error::Forbidden)
	));
	assert!(scope.state.lock().unwrap().trace.is_empty());
}

#[rstest]
#[tokio::test]
async fn rejected_login_never_registers_an_account(mut scope: Scope) {
	scope.result = ProviderResult::Disabled;
	let scope = Arc::new(scope);
	let mut login = Recovery(scope.state.clone());
	assert!(matches!(
		authority(scope.clone())
			.admit_login(&mut login, &sign_in())
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
			.admit_login(&mut login, &sign_in())
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

fn sign_in() -> SignIn {
	SignIn {
		subject: "subject".into(),
		gcip_tenant: None,
		gcip_provider: None,
		auth_time: DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
		verified_email: None,
		display_name: None,
	}
}

#[rstest]
#[tokio::test]
async fn gcip_revocation_ends_older_sessions_without_disabling_work(mut scope: Scope) {
	scope.result = ProviderResult::Revoked;
	scope.tenant_bindings = Some([("pool-a".into(), "acme".into())].into());
	let mut account = scope.state.lock().unwrap().account.clone();
	account.gcip_tenant = Some("pool-a".into());
	scope.state.lock().unwrap().account.gcip_tenant = account.gcip_tenant.clone();
	let now = scope.state.lock().unwrap().now;
	let scope = Arc::new(scope);
	authority(scope.clone())
		.account_valid(&account)
		.await
		.unwrap();
	assert_eq!(
		scope.state.lock().unwrap().trace,
		[
			"lookup:subject".to_owned(),
			"record".into(),
			format!(
				"revoke-before:{}",
				(now - Duration::seconds(60)).timestamp()
			)
		]
	);
}

#[rstest]
#[tokio::test]
async fn disabled_gcip_account_disables_identity_and_marks_work_at_a_boundary(mut scope: Scope) {
	scope.result = ProviderResult::Disabled;
	scope.tenant_bindings = Some([("pool-a".into(), "acme".into())].into());
	let mut account = scope.state.lock().unwrap().account.clone();
	account.gcip_tenant = Some("pool-a".into());
	scope.state.lock().unwrap().account.gcip_tenant = account.gcip_tenant.clone();
	let scope = Arc::new(scope);
	assert!(matches!(
		authority(scope.clone()).account_valid(&account).await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		scope.state.lock().unwrap().trace,
		["lookup:subject", "disable-current", "mark-disabled"]
	);
}

#[rstest]
#[tokio::test]
async fn removed_binding_disables_even_a_fresh_identity_before_provider_io(mut scope: Scope) {
	scope.tenant_bindings = Some(Default::default());
	let mut account = scope.state.lock().unwrap().account.clone();
	account.gcip_tenant = Some("removed-pool".into());
	account.last_valid_at = Some(scope.state.lock().unwrap().now);
	let scope = Arc::new(scope);
	assert!(matches!(
		authority(scope.clone()).account_valid(&account).await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		scope.state.lock().unwrap().trace,
		["disable-current", "mark-disabled"]
	);
}

#[rstest]
#[case("pool-a", "acme", true)]
#[case("pool-a", "other", false)]
#[case("pool-b", "acme", false)]
fn mapping_requires_the_exact_bound_tenant(
	#[case] pool: &str,
	#[case] tenant: &str,
	#[case] allowed: bool,
) {
	let policy = AccountPolicy {
		issuer: "issuer".into(),
		google: false,
		tenant_bindings: Some([("pool-a".into(), "acme".into())].into()),
		gcip_providers: Default::default(),
	};
	assert_eq!(
		policy.require_mapping("issuer", Some(pool), tenant).is_ok(),
		allowed
	);
	assert!(
		policy
			.require_mapping("old-issuer", Some(pool), tenant)
			.is_err()
	);
	assert!(policy.require_mapping("issuer", None, tenant).is_err());
}

#[rstest]
#[case(-1, true)]
#[case(0, false)]
#[case(1, false)]
fn valid_since_compares_authentication_time_strictly(#[case] seconds: i64, #[case] revoked: bool) {
	let since = DateTime::from_timestamp(1_800_000_000, 0).unwrap();
	assert_eq!(
		session_revoked(since + Duration::seconds(seconds), Some(since)),
		revoked
	);
	assert!(!session_revoked(since, None));
}

#[rstest]
#[tokio::test]
async fn stale_authentication_cannot_create_a_session(mut scope: Scope) {
	scope.valid_since = Some(scope.state.lock().unwrap().now + Duration::seconds(1));
	let scope = Arc::new(scope);
	let mut login = Recovery(scope.state.clone());
	assert!(matches!(
		authority(scope.clone())
			.admit_login(&mut login, &sign_in())
			.await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.state.lock().unwrap().trace, ["lookup:subject"]);
}
