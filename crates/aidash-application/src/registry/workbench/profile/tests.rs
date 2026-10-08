use super::*;
use crate::ports::registry::workbench::profile::ProfileDraftScope;
use aidash_domain::registry::{EntityRef, workbench::Draft};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use rstest::rstest;
use serde_json::{Value, json};
use std::sync::Mutex;
use uuid::Uuid;
#[derive(Default)]
struct State {
	active: bool,
	committed: bool,
	calls: Vec<&'static str>,
	saved: Option<Value>,
}
struct Repository {
	principal: Principal,
	state: Mutex<State>,
	authorized: bool,
	pause: bool,
	revision: i64,
}
impl Repository {
	fn new(principal: Principal) -> Self {
		Self {
			principal,
			state: Mutex::new(State::default()),
			authorized: true,
			pause: false,
			revision: 0,
		}
	}
}
struct Scope<'a>(&'a Repository);
impl Drop for Scope<'_> {
	fn drop(&mut self) {
		self.0.state.lock().unwrap().active = false;
	}
}
struct Configuration {
	present: bool,
	reads: Mutex<Vec<String>>,
}
impl Configuration {
	fn new() -> Self {
		Self {
			present: true,
			reads: Mutex::new(vec![]),
		}
	}
}
impl ProfileConfiguration for Configuration {
	fn require_secret(&self, name: &str) -> Result<()> {
		self.reads.lock().unwrap().push(name.into());
		if self.present {
			Ok(())
		} else {
			Err(Error::Invalid(format!(
				"credential reference {name} is not configured"
			)))
		}
	}
}
fn reference(id: &str) -> EntityRef {
	EntityRef {
		id: id.into(),
		version: "1.0.0".into(),
	}
}
fn subject() -> Principal {
	Principal::Subject {
		tenant: "tenant".into(),
		subject: "reader".into(),
	}
}
fn rule() -> RealToolRule {
	RealToolRule {
		read_only_verified: true,
		tool: reference("tool"),
		endpoint: "https://test.example/rpc".into(),
		credential_env: Some("AIDASH_SECRET_TEST_TOOL".into()),
		allowed_actions: vec!["read".into()],
		allowed_resources: vec!["fixture".into()],
	}
}
fn tool() -> Entry {
	let mut tool = crate::test_support::http_tool("aidash://local", "tool", "lookup");
	tool.config["transport"]["credential_env"] = json!("AIDASH_SECRET_PRODUCTION_TOOL");
	tool
}
fn draft_entry() -> Entry {
	let mut entry = crate::test_support::agent("agent");
	entry.config["bindings"] = json!([crate::test_support::binding(
		"tool",
		"aidash://local",
		"tool"
	)]);
	entry
}

fn input() -> ProfileInput {
	ProfileInput {
		expected_revision: 0,
		enabled: true,
		rules: vec![rule()],
	}
}
fn profile(id: &str, enabled: bool, rules: Value) -> TestProfile {
	TestProfile {
		tenant: "tenant".into(),
		id: id.into(),
		revision: 7,
		enabled,
		rules,
		updated_at: Utc.timestamp_opt(100, 0).unwrap(),
	}
}
#[async_trait]
impl ProfileRepository for Repository {
	fn principal(&self) -> Principal {
		self.principal.clone()
	}
	async fn begin_draft(&self) -> Result<Box<dyn ProfileDraftScope + '_>> {
		let mut s = self.state.lock().unwrap();
		s.active = true;
		s.calls.push("begin");
		Ok(Box::new(Scope(self)))
	}
	async fn page(&self, tenant: &str) -> Result<Vec<TestProfile>> {
		assert_eq!(tenant, "tenant");
		let mut s = self.state.lock().unwrap();
		assert!(!s.active);
		s.calls.push("page");
		if self.principal != Principal::Operator {
			assert!(s.committed);
		}
		let mut foreign = rule();
		foreign.tool = reference("other");
		Ok(vec![
			profile("allowed", true, json!([rule()])),
			profile("disabled", false, json!([rule()])),
			profile("foreign", true, json!([foreign.clone()])),
			profile("empty", true, json!([])),
			profile("invalid", true, json!({"malformed":true})),
			profile("partial", true, json!([rule(), foreign])),
		])
	}
	async fn definition(&self, entry: &EntityRef) -> Result<Entry> {
		assert_eq!(*entry, reference("tool"));
		self.state.lock().unwrap().calls.push("definition");
		Ok(tool())
	}
	async fn save(&self, tenant: &str, id: &str, input: &ProfileInput) -> Result<TestProfile> {
		assert_eq!(tenant, "tenant");
		assert_eq!(id, "profile");
		let mut s = self.state.lock().unwrap();
		s.calls.push("save");
		if input.expected_revision != self.revision {
			return Err(Error::Conflict("test profile changed".into()));
		}
		let rules = serde_json::to_value(&input.rules)?;
		s.saved = Some(rules.clone());
		let mut row = profile(id, input.enabled, rules);
		row.revision = self.revision + 1;
		Ok(row)
	}
}
#[async_trait]
impl ProfileDraftScope for Scope<'_> {
	async fn bindings(
		&mut self,
		entry: &aidash_domain::registry::Entry,
	) -> Result<aidash_domain::registry::bindings::BindingSnapshot> {
		Ok(crate::test_support::resolve(
			"aidash://local",
			entry,
			false,
			vec![tool()],
		))
	}

	async fn draft(&mut self, id: Uuid) -> Result<Draft> {
		assert_eq!(id, Uuid::from_u128(1));
		self.0.state.lock().unwrap().calls.push("draft");
		Ok(Draft {
			id,
			tenant: "tenant".into(),
			owner: "owner".into(),
			revision: 5,
			entry: json!(draft_entry()),
			documents: json!([]),
			release_notes: String::new(),
			source_id: None,
			source_version: None,
			archived: false,
			updated_at: Utc.timestamp_opt(100, 0).unwrap(),
		})
	}
	async fn authorize(&mut self, draft: &Draft, action: &str, shares: bool) -> Result<()> {
		assert_eq!(draft.id, Uuid::from_u128(1));
		assert_eq!(action, "agent_draft.test");
		assert!(shares);
		self.0.state.lock().unwrap().calls.push("authority");
		if self.0.pause {
			std::future::pending::<()>().await;
		}
		if self.0.authorized {
			Ok(())
		} else {
			Err(Error::Forbidden)
		}
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		let mut s = self.0.state.lock().unwrap();
		s.committed = true;
		s.calls.push("commit");
		Ok(())
	}
}
#[rstest]
#[case("subject")]
#[case("operator")]
#[tokio::test]
async fn profile_listing_retains_subject_rule_matching_and_operator_visibility(
	#[case] actor: &str,
) {
	let principal = if actor == "subject" {
		subject()
	} else {
		Principal::Operator
	};
	let r = Repository::new(principal);
	let rows = list(
		&r,
		ProfileQuery {
			tenant: Some("tenant".into()),
			draft_id: Some(Uuid::from_u128(1)),
		},
	)
	.await
	.unwrap();
	if actor == "subject" {
		assert_eq!(
			rows.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
			vec!["allowed"]
		);
		assert_eq!(
			r.state.lock().unwrap().calls,
			vec!["begin", "draft", "authority", "commit", "page"]
		);
	} else {
		assert_eq!(rows.len(), 6);
		assert_eq!(r.state.lock().unwrap().calls, vec!["page"]);
	}
	let wire = serde_json::to_value(&rows[0]).unwrap();
	assert_eq!(wire.as_object().unwrap().len(), 3);
	assert_eq!(wire["revision"], 7);
}
#[rstest]
#[case("tenant")]
#[case("missing_draft")]
#[case("authority")]
#[tokio::test]
async fn subject_profiles_are_not_read_without_current_draft_test_authority(#[case] denial: &str) {
	let mut r = Repository::new(subject());
	let mut query = ProfileQuery {
		tenant: Some("tenant".into()),
		draft_id: Some(Uuid::from_u128(1)),
	};
	match denial {
		"tenant" => query.tenant = Some("other".into()),
		"missing_draft" => query.draft_id = None,
		_ => r.authorized = false,
	}
	assert!(list(&r, query).await.is_err());
	let s = r.state.lock().unwrap();
	assert!(!s.active && !s.committed);
	assert!(!s.calls.contains(&"page"));
	if denial != "authority" {
		assert!(s.calls.is_empty());
	}
}
#[rstest]
#[tokio::test]
async fn profile_writes_remain_operator_only_before_external_reads() {
	let r = Repository::new(subject());
	let config = Configuration::new();
	assert!(matches!(
		put(&r, &config, "tenant".into(), "profile".into(), input()).await,
		Err(Error::Forbidden)
	));
	assert!(r.state.lock().unwrap().calls.is_empty());
	assert!(config.reads.lock().unwrap().is_empty());
}
#[rstest]
#[case("duplicate")]
#[case("no_action")]
#[case("no_resource")]
#[case("many_actions")]
#[case("large_resource")]
#[tokio::test]
async fn rule_identity_and_allowlist_bounds_gate_atomic_save(#[case] boundary: &str) {
	let r = Repository::new(Principal::Operator);
	let config = Configuration::new();
	let mut input = input();
	match boundary {
		"duplicate" => input.rules.push(rule()),
		"no_action" => input.rules[0].allowed_actions.clear(),
		"no_resource" => input.rules[0].allowed_resources.clear(),
		"many_actions" => input.rules[0].allowed_actions = vec!["read".into(); 65],
		_ => input.rules[0].allowed_resources = vec!["a".repeat(201)],
	}
	assert!(matches!(
		put(&r, &config, "tenant".into(), "profile".into(), input).await,
		Err(Error::Invalid(_))
	));
	let s = r.state.lock().unwrap();
	assert!(!s.calls.contains(&"save"));
	assert!(s.saved.is_none());
}
#[rstest]
#[case(0, true)]
#[case(1, false)]
#[tokio::test]
async fn validated_profiles_forward_the_original_atomic_revision_fence(
	#[case] expected: i64,
	#[case] success: bool,
) {
	let r = Repository::new(Principal::Operator);
	let config = Configuration::new();
	let mut input = input();
	input.expected_revision = expected;
	let result = put(&r, &config, "tenant".into(), "profile".into(), input).await;
	assert_eq!(result.is_ok(), success);
	if success {
		assert_eq!(result.unwrap().revision, 1);
	} else {
		assert!(matches!(result, Err(Error::Conflict(_))));
	}
	assert_eq!(
		config.reads.lock().unwrap().as_slice(),
		["AIDASH_SECRET_TEST_TOOL"]
	);
	assert_eq!(r.state.lock().unwrap().saved.is_some(), success);
}
#[rstest]
#[case("insecure")]
#[case("production_endpoint")]
#[case("production_credential")]
#[case("write_tool")]
#[case("non_http")]
#[case("non_tool")]
#[case("missing_credential")]
fn real_rule_preserves_endpoint_credential_and_replay_isolation(#[case] denial: &str) {
	let mut rule = rule();
	let mut tool = tool();
	let mut config = Configuration::new();
	match denial {
		"insecure" => rule.endpoint = "http://test.example/rpc".into(),
		"production_endpoint" => rule.endpoint = "https://production.example/rpc".into(),
		"production_credential" => {
			rule.credential_env = Some("AIDASH_SECRET_PRODUCTION_TOOL".into())
		}
		"write_tool" => rule.read_only_verified = false,
		"non_http" => tool.config = json!({"transport":"native","operation":"read"}),
		"non_tool" => tool.kind = "model".into(),
		_ => config.present = false,
	}
	assert!(validate_real_rule(&config, &rule, &tool).is_err());
}
#[rstest]
#[case("https://test.example/rpc")]
#[case("http://localhost/rpc")]
#[case("http://127.0.0.1/rpc")]
#[case("http://[::1]/rpc")]
fn read_only_test_rule_accepts_the_existing_secure_and_loopback_endpoints(#[case] endpoint: &str) {
	let config = Configuration::new();
	let mut rule = rule();
	rule.endpoint = endpoint.into();
	validate_real_rule(&config, &rule, &tool()).unwrap();
	assert_eq!(config.reads.lock().unwrap().len(), 1);
}
#[rstest]
#[tokio::test]
async fn cancelling_current_draft_authority_releases_the_uncommitted_scope() {
	let mut r = Repository::new(subject());
	r.pause = true;
	let mut future = Box::pin(list(
		&r,
		ProfileQuery {
			tenant: None,
			draft_id: Some(Uuid::from_u128(1)),
		},
	));
	assert!(futures_util::poll!(&mut future).is_pending());
	assert!(r.state.lock().unwrap().active);
	drop(future);
	let s = r.state.lock().unwrap();
	assert!(!s.active && !s.committed);
	assert!(!s.calls.contains(&"page"));
}
