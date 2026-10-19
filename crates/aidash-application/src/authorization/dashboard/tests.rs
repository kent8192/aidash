use super::*;
use crate::ports::authorization::dashboard::{AccountPolicy, Issuer, Issuers, StatusRecovery};
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
	active: bool,
	/// The issuer reports no Account Status, as generic OIDC does.
	statusless: bool,
	personal_tenants: bool,
	/// Issuers whose identities are restricted to one Tenant each.
	restrictions: Option<std::collections::BTreeMap<String, String>>,
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
		active: true,
		statusless: false,
		personal_tenants: false,
		restrictions: None,
		result: ProviderResult::Enabled,
		newer_validity: false,
		delay_seconds: 0,
		valid_since: None,
	}
}
struct Restricted(std::collections::BTreeMap<String, String>);
impl Issuers for Restricted {
	fn admitted(&self) -> Vec<String> {
		self.0.keys().cloned().collect()
	}
	fn restriction(&self, issuer: &str) -> Option<String> {
		self.0.get(issuer).cloned()
	}
	fn restricted_issuer(&self, tenant: &str) -> Option<String> {
		self.0
			.iter()
			.find_map(|(issuer, restricted)| (restricted == tenant).then(|| issuer.clone()))
	}
	fn retires_refused(&self) -> bool {
		true
	}
}
#[async_trait]
impl AccountStatus for Scope {
	async fn lookup(
		&self,
		issuer: &str,
		subject: &str,
	) -> Result<aidash_domain::identity::dashboard::AccountState> {
		let mut state = self.state.lock().unwrap();
		state.trace.push(format!("lookup:{subject}"));
		assert_eq!(issuer, state.account.issuer);
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
			issuers: match &self.restrictions {
				Some(restrictions) => Arc::new(Restricted(restrictions.clone())),
				None => Arc::new(Issuer("issuer".into())),
			},
			personal_tenants: self.personal_tenants,
		})
	}
	fn now(&self) -> DateTime<Utc> {
		self.state.lock().unwrap().now
	}
	async fn identities(&self) -> Result<Vec<Account>> {
		let account = self.state.lock().unwrap().account.clone();
		Ok(if account.disabled_at.is_none() {
			vec![account]
		} else {
			vec![]
		})
	}
	async fn active(&self) -> Result<Vec<Account>> {
		Ok(if self.active {
			vec![self.state.lock().unwrap().account.clone()]
		} else {
			vec![]
		})
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
	let status = (!scope.statusless).then(|| scope.clone() as Arc<dyn AccountStatus>);
	DashboardAuthority {
		accounts: scope,
		status,
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
async fn a_statusless_issuer_refresh_reads_no_account_status(mut scope: Scope) {
	scope.statusless = true;
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
	async fn provision_personal(&mut self, identity: Uuid) -> Result<()> {
		self.0
			.lock()
			.unwrap()
			.trace
			.push(format!("personal:{identity}"));
		Ok(())
	}
	async fn find(&mut self, _: &SignIn) -> Result<Option<Account>> {
		Ok(None)
	}
	async fn register(&mut self, sign_in: &SignIn) -> Result<Account> {
		let mut state = self.0.lock().unwrap();
		state
			.trace
			.push(format!("register:{}:{}", sign_in.issuer, sign_in.subject));
		Ok(state.account.clone())
	}
}

struct ExistingLogin(Arc<Mutex<State>>);
#[async_trait]
impl crate::ports::authorization::dashboard::LoginAccounts for ExistingLogin {
	async fn provision_personal(&mut self, identity: Uuid) -> Result<()> {
		self.0
			.lock()
			.unwrap()
			.trace
			.push(format!("personal:{identity}"));
		Ok(())
	}
	async fn find(&mut self, sign_in: &SignIn) -> Result<Option<Account>> {
		let mut state = self.0.lock().unwrap();
		state.trace.push("find".into());
		Ok(
			(state.account.issuer == sign_in.issuer && state.account.subject == sign_in.subject)
				.then(|| state.account.clone()),
		)
	}
	async fn register(&mut self, _: &SignIn) -> Result<Account> {
		let mut state = self.0.lock().unwrap();
		state.trace.push("register".into());
		Ok(state.account.clone())
	}
}

#[rstest]
#[case::enabled(false)]
#[case::disabled(true)]
#[tokio::test]
async fn login_provisions_personal_tenants_only_for_enabled_accounts(
	mut scope: Scope,
	#[case] disabled: bool,
) {
	scope.personal_tenants = true;
	let id = {
		let mut state = scope.state.lock().unwrap();
		if disabled {
			state.account.disabled_at = Some(state.now);
		}
		state.account.id
	};
	let scope = Arc::new(scope);
	let mut login = ExistingLogin(scope.state.clone());
	let result = authority(scope.clone())
		.admit_login(&mut login, &sign_in())
		.await;
	if disabled {
		assert!(matches!(result, Err(Error::Forbidden)));
		assert_eq!(
			scope.state.lock().unwrap().trace,
			["lookup:subject", "register"]
		);
	} else {
		assert_eq!(result.unwrap().id, id);
		assert_eq!(
			scope.state.lock().unwrap().trace,
			[
				"lookup:subject".to_owned(),
				"register".to_owned(),
				format!("personal:{id}"),
				"record".to_owned()
			]
		);
	}
}

#[rstest]
#[case::rule_off(false, None)]
#[case::bound_pool(true, Some("pool-a"))]
#[tokio::test]
async fn login_skips_personal_tenants_when_the_rules_do_not_cover_the_identity(
	mut scope: Scope,
	#[case] personal_tenants: bool,
	#[case] pool: Option<&str>,
) {
	scope.personal_tenants = personal_tenants;
	if let Some(pool) = pool {
		scope.restrictions = Some([(pool.to_owned(), "acme".to_owned())].into());
		scope.state.lock().unwrap().account.issuer = pool.into();
	}
	let scope = Arc::new(scope);
	let mut login = ExistingLogin(scope.state.clone());
	let sign_in = SignIn {
		issuer: pool.unwrap_or("issuer").to_owned(),
		..sign_in()
	};
	authority(scope.clone())
		.admit_login(&mut login, &sign_in)
		.await
		.unwrap();
	assert!(
		!scope
			.state
			.lock()
			.unwrap()
			.trace
			.iter()
			.any(|step| step.starts_with("personal:"))
	);
}

#[rstest]
#[case::removed(false)]
#[case::bound(true)]
#[tokio::test]
async fn removed_binding_refresh_disables_an_inactive_identity(
	mut scope: Scope,
	#[case] bound: bool,
) {
	scope.active = false;
	scope.restrictions = Some(if bound {
		[("pool-a".into(), "acme".into())].into()
	} else {
		Default::default()
	});
	scope.state.lock().unwrap().account.issuer = "pool-a".into();
	assert!(
		authority(Arc::new(scope.clone()))
			.active()
			.await
			.unwrap()
			.is_empty()
	);
	let state = scope.state.lock().unwrap();
	assert_eq!(state.account.disabled_at.is_some(), !bound);
	assert_eq!(
		state.trace,
		if bound {
			vec![]
		} else {
			vec!["disable-current", "mark-disabled"]
		}
	);
}

#[rstest]
#[tokio::test]
async fn removed_binding_login_persists_disable_across_binding_restoration(mut scope: Scope) {
	scope.restrictions = Some(Default::default());
	scope.state.lock().unwrap().account.issuer = "pool-a".into();
	let sign_in = SignIn {
		issuer: "pool-a".into(),
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
	scope.restrictions = Some([("pool-a".into(), "acme".into())].into());
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
	scope.restrictions = Some(Default::default());
	let sign_in = SignIn {
		issuer: "pool-a".into(),
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
		issuer: "issuer".into(),
		subject: "subject".into(),
		auth_time: DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
		verified_email: None,
		display_name: None,
	}
}

#[rstest]
#[tokio::test]
async fn restricted_revocation_ends_older_sessions_without_disabling_work(mut scope: Scope) {
	scope.result = ProviderResult::Revoked;
	scope.restrictions = Some([("pool-a".into(), "acme".into())].into());
	let mut account = scope.state.lock().unwrap().account.clone();
	account.issuer = "pool-a".into();
	scope.state.lock().unwrap().account.issuer = account.issuer.clone();
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
async fn disabled_restricted_account_disables_identity_and_marks_work_at_a_boundary(
	mut scope: Scope,
) {
	scope.result = ProviderResult::Disabled;
	scope.restrictions = Some([("pool-a".into(), "acme".into())].into());
	let mut account = scope.state.lock().unwrap().account.clone();
	account.issuer = "pool-a".into();
	scope.state.lock().unwrap().account.issuer = account.issuer.clone();
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
	scope.restrictions = Some(Default::default());
	let mut account = scope.state.lock().unwrap().account.clone();
	account.issuer = "removed-pool".into();
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
		issuers: Arc::new(Restricted([("pool-a".into(), "acme".into())].into())),
		personal_tenants: true,
	};
	assert_eq!(policy.require_mapping(pool, tenant).is_ok(), allowed);
	assert!(policy.require_mapping("old-issuer", tenant).is_err());
	let unrestricted = AccountPolicy {
		issuers: Arc::new(Issuer("issuer".into())),
		personal_tenants: true,
	};
	assert!(unrestricted.require_mapping("issuer", tenant).is_ok());
	assert!(unrestricted.require_mapping(pool, tenant).is_err());
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
