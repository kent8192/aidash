use super::*;
use crate::ports::{Credentials, registry::CoreToolCatalog};
use aidash_domain::{capabilities::CoreCapabilities, provider::ToolSpec, registry::EntityRef};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::Value;
use std::{
	collections::{BTreeMap, BTreeSet},
	sync::{Arc, Mutex},
	time::Duration,
};

struct NoSecrets;
impl Credentials for NoSecrets {
	fn resolve(&self, _: &str) -> Result<String> {
		panic!("policy admission must not execute an approved provider")
	}
}
struct CoreContracts;
impl CoreToolCatalog for CoreContracts {
	fn specifications(&self, _: &CoreCapabilities) -> BTreeMap<String, ToolSpec> {
		BTreeMap::new()
	}
}
#[fixture]
fn validation() -> DefinitionValidation {
	DefinitionValidation::new(Arc::new(NoSecrets), Arc::new(CoreContracts))
}
#[fixture]
fn spec() -> Spec {
	serde_json::from_value(json!({
        "enabled": true,
        "template": {"id":"template", "version":"1.0.0", "kind":"agent", "name":{"en":"Template"}, "description":{"en":""},
            "config":{"schema_version":1,"bindings":[],"remove_default":[],"model":{"id":"model","version":"1.0.0"},"instructions":"Do useful work."}},
        "permissions":{"roles":["role"], "groups":["group"], "attributes":{"team":"research"}},
        "limits":{"max_agents":5, "max_concurrent":2, "max_depth":2, "token_budget":1000000, "tokens_per_agent":500000, "lifetime_seconds":600},
        "approval_required":true
    })).unwrap()
}
fn bundle() -> Value {
	json!({"tenant":"tenant", "roles":{"role":{}}, "groups":{"group":{}}, "subjects":{}, "policies":[]})
}
fn model() -> Value {
	json!({"id":"model", "version":"1.0.0", "kind":"model", "name":{"en":"Model"}, "description":{"en":""},
        "config":{"provider":"openrouter", "model_id":"fixture", "endpoint":"https://provider.invalid", "context_window":4096, "max_output_tokens":1024, "modalities":["text"], "cost":{}}})
}
fn policy(id: &str, revision: i64, spec: Spec) -> Policy {
	Policy {
		tenant: "tenant".into(),
		id: id.into(),
		revision,
		spec,
		generated_count: 2,
		allocated_tokens: 10000,
		allocated_compaction_calls: 3,
		allocated_embedding_calls: 4,
		allocated_summary_calls: 0,
	}
}

#[derive(Default)]
struct State {
	calls: Vec<String>,
	document: Value,
	approved: BTreeMap<(String, String), Value>,
	persisted: BTreeMap<String, Policy>,
	history: Vec<(String, i64, Value, String)>,
	denied: BTreeSet<String>,
	failure: Option<&'static str>,
	pause: Option<&'static str>,
	commits: usize,
	rollbacks: usize,
	denial_audits: usize,
	active_scopes: usize,
}
struct Repository {
	principal: Principal,
	state: Arc<Mutex<State>>,
}
impl Repository {
	fn new() -> Self {
		Self {
			principal: Principal::Operator,
			state: Arc::new(Mutex::new(State {
				document: bundle(),
				approved: crate::test_support::builtin_entries("aidash://local")
					.into_iter()
					.map(|e| ((e.id.clone(), e.version.clone()), json!(e)))
					.chain(std::iter::once((("model".into(), "1.0.0".into()), model())))
					.collect(),
				..State::default()
			})),
		}
	}
	fn subject(&mut self) {
		self.principal = Principal::Subject {
			tenant: "tenant".into(),
			subject: "alice".into(),
		};
	}
	fn calls(&self) -> Vec<String> {
		self.state
			.lock()
			.unwrap()
			.calls
			.iter()
			.filter(|call| !call.starts_with("approved:tenant:aidash."))
			.cloned()
			.collect()
	}
}
struct Scope {
	state: Arc<Mutex<State>>,
	staged: BTreeMap<String, Policy>,
	history: Vec<(String, i64, Value, String)>,
	denial_count: usize,
	completed: bool,
}
impl Scope {
	fn call(&self, operation: String) {
		self.state.lock().unwrap().calls.push(operation)
	}
	async fn point(&self, operation: &'static str) -> Result<()> {
		let (failure, pause) = {
			let state = self.state.lock().unwrap();
			(state.failure, state.pause)
		};
		if failure == Some(operation) {
			return Err(Error::External(format!("fixture {operation} failed")));
		}
		if pause == Some(operation) {
			std::future::pending::<()>().await;
		}
		Ok(())
	}
	fn finish<T>(&mut self, result: Result<T>) -> Result<T> {
		self.call("finish".into());
		let mut state = self.state.lock().unwrap();
		self.completed = true;
		state.active_scopes -= 1;
		if result.is_ok() && state.failure == Some("commit") {
			state.rollbacks += 1;
			return Err(Error::External("fixture commit failed".into()));
		}
		if result.is_ok() {
			state.persisted.append(&mut self.staged);
			state.history.append(&mut self.history);
			state.commits += 1;
		} else {
			state.rollbacks += 1;
			if matches!(&result, Err(Error::Forbidden)) {
				state.denial_audits += self.denial_count;
			}
		}
		result
	}
}
impl Drop for Scope {
	fn drop(&mut self) {
		if !self.completed {
			let mut state = self.state.lock().unwrap();
			state.active_scopes -= 1;
			state.rollbacks += 1;
		}
	}
}
#[async_trait]
impl GenerationPolicies for Repository {
	fn principal(&self) -> &Principal {
		&self.principal
	}
	async fn ids(&self, tenant: &str) -> Result<Vec<String>> {
		let mut state = self.state.lock().unwrap();
		state.calls.push(format!("ids:{tenant}"));
		Ok(state.persisted.keys().cloned().collect())
	}
	async fn begin(&self, tenant: &str, exclusive: bool) -> Result<Box<dyn PolicySession>> {
		let mut state = self.state.lock().unwrap();
		state.calls.push(format!("begin:{tenant}:{exclusive}"));
		state.active_scopes += 1;
		Ok(Box::new(Scope {
			state: self.state.clone(),
			staged: BTreeMap::new(),
			history: vec![],
			denial_count: 0,
			completed: false,
		}))
	}
}
#[async_trait]
impl PolicySession for Scope {
	async fn bindings(
		&mut self,
		entry: &aidash_domain::registry::Entry,
	) -> Result<aidash_domain::registry::bindings::BindingSnapshot> {
		Ok(crate::test_support::resolve(
			"aidash://local",
			entry,
			false,
			self.state
				.lock()
				.unwrap()
				.approved
				.values()
				.filter(|value| value["kind"] == "source")
				.filter_map(|v| serde_json::from_value(v.clone()).ok())
				.collect(),
		))
	}

	async fn decide(&mut self, id: &str, action: &str) -> Result<bool> {
		self.call(format!("decide:{id}:{action}"));
		let allowed = !self.state.lock().unwrap().denied.contains(id);
		if !allowed {
			self.denial_count += 1;
		}
		Ok(allowed)
	}
	async fn bundle(&mut self, tenant: &str) -> Result<Value> {
		self.call(format!("bundle:{tenant}"));
		self.point("bundle").await?;
		Ok(self.state.lock().unwrap().document.clone())
	}
	async fn previous(&mut self, tenant: &str, id: &str, revision: i64) -> Result<Option<Value>> {
		self.call(format!("previous:{tenant}:{id}:{revision}"));
		Ok(self
			.state
			.lock()
			.unwrap()
			.persisted
			.get(id)
			.filter(|p| p.revision == revision)
			.map(|p| json!(p.spec)))
	}
	async fn approved(&mut self, tenant: &str, reference: &EntityRef) -> Result<Option<Value>> {
		self.call(format!(
			"approved:{tenant}:{}:{}",
			reference.id, reference.version
		));
		Ok(self
			.state
			.lock()
			.unwrap()
			.approved
			.get(&(reference.id.clone(), reference.version.clone()))
			.cloned())
	}
	async fn compare_and_set(
		&mut self,
		tenant: &str,
		id: &str,
		expected: i64,
		spec: &Spec,
	) -> Result<Option<i64>> {
		self.call(format!("cas:{tenant}:{id}:{expected}"));
		self.point("cas").await?;
		let previous = self.state.lock().unwrap().persisted.get(id).cloned();
		let revision = match previous {
			None if expected == 0 => 1,
			Some(policy) if policy.revision == expected => expected + 1,
			_ => return Ok(None),
		};
		self.staged
			.insert(id.into(), policy(id, revision, spec.clone()));
		Ok(Some(revision))
	}
	async fn history(
		&mut self,
		tenant: &str,
		id: &str,
		revision: i64,
		spec: &Spec,
		actor: &str,
	) -> Result<()> {
		self.call(format!("history:{tenant}:{id}:{revision}:{actor}"));
		self.point("history").await?;
		self.history
			.push((id.into(), revision, json!(spec), actor.into()));
		Ok(())
	}
	async fn load(&mut self, tenant: &str, id: &str, exclusive: bool) -> Result<Policy> {
		self.call(format!("load:{tenant}:{id}:{exclusive}"));
		self.point("load").await?;
		self.staged
			.get(id)
			.cloned()
			.or_else(|| self.state.lock().unwrap().persisted.get(id).cloned())
			.ok_or_else(|| Error::NotFound("generation policy".into()))
	}
	async fn finish_update(mut self: Box<Self>, result: Result<Policy>) -> Result<Policy> {
		self.finish(result)
	}
	async fn finish_list(mut self: Box<Self>, result: Result<Vec<Policy>>) -> Result<Vec<Policy>> {
		self.finish(result)
	}
}

#[rstest]
#[tokio::test]
async fn authorized_subject_update_commits_policy_and_history_with_trusted_actor(
	spec: Spec,
	validation: DefinitionValidation,
) {
	let mut repo = Repository::new();
	repo.subject();
	let result = set(&repo, "tenant", "policy", 0, &spec, &validation)
		.await
		.unwrap();
	assert_eq!(result.revision, 1);
	assert_eq!(
		repo.calls(),
		vec![
			"begin:tenant:true",
			"decide:policy:generation.manage",
			"bundle:tenant",
			"approved:tenant:model:1.0.0",
			"cas:tenant:policy:0",
			"history:tenant:policy:1:alice",
			"load:tenant:policy:false",
			"finish"
		]
	);
	let state = repo.state.lock().unwrap();
	assert_eq!(state.persisted["policy"].revision, 1);
	assert_eq!(
		state.history,
		vec![("policy".into(), 1, json!(spec), "alice".into())]
	);
	assert_eq!(
		(state.commits, state.rollbacks, state.active_scopes),
		(1, 0, 0)
	);
}
#[rstest]
#[tokio::test]
async fn cross_tenant_subject_cannot_open_a_mutation_transaction(
	spec: Spec,
	validation: DefinitionValidation,
) {
	let mut repo = Repository::new();
	repo.subject();
	assert!(matches!(
		set(&repo, "other", "policy", 0, &spec, &validation).await,
		Err(Error::Forbidden)
	));
	assert_eq!(repo.calls(), Vec::<String>::new());
}
#[rstest]
#[tokio::test]
async fn denied_worker_call_preserves_denial_audit_without_bundle_read_or_write(
	spec: Spec,
	validation: DefinitionValidation,
) {
	let mut repo = Repository::new();
	repo.subject();
	repo.state.lock().unwrap().denied.insert("policy".into());
	assert!(matches!(
		set(&repo, "tenant", "policy", 0, &spec, &validation).await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		repo.calls(),
		vec![
			"begin:tenant:true",
			"decide:policy:generation.manage",
			"finish"
		]
	);
	let state = repo.state.lock().unwrap();
	assert!(state.persisted.is_empty());
	assert!(state.history.is_empty());
	assert_eq!(
		(state.commits, state.rollbacks, state.denial_audits),
		(0, 1, 1)
	);
}
#[rstest]
#[case(-1)]
#[case(i64::MAX)]
#[tokio::test]
async fn invalid_revision_never_reads_or_writes_policy_rows(
	spec: Spec,
	validation: DefinitionValidation,
	#[case] revision: i64,
) {
	let repo = Repository::new();
	assert!(
		matches!(set(&repo,"tenant","policy",revision,&spec,&validation).await,Err(Error::Invalid(message)) if message=="invalid generation policy revision")
	);
	assert_eq!(repo.calls(), vec!["begin:tenant:true", "finish"]);
}
#[rstest]
#[tokio::test]
async fn unchanged_disable_survives_revoked_catalog_and_invalid_authorization_document(
	mut spec: Spec,
	validation: DefinitionValidation,
) {
	let repo = Repository::new();
	{
		let mut state = repo.state.lock().unwrap();
		state
			.persisted
			.insert("policy".into(), policy("policy", 7, spec.clone()));
		state.approved.clear();
		state.document = json!({"unavailable":"revoked"});
	}
	spec.enabled = false;
	let result = set(&repo, "tenant", "policy", 7, &spec, &validation)
		.await
		.unwrap();
	assert_eq!((result.revision, result.spec.enabled), (8, false));
	assert_eq!(
		repo.calls(),
		vec![
			"begin:tenant:true",
			"bundle:tenant",
			"previous:tenant:policy:7",
			"cas:tenant:policy:7",
			"history:tenant:policy:8:operator",
			"load:tenant:policy:false",
			"finish"
		]
	);
}
#[rstest]
#[case::edit(false)]
#[case::reenable(true)]
#[tokio::test]
async fn edits_and_reenable_revalidate_revoked_components(
	mut spec: Spec,
	validation: DefinitionValidation,
	#[case] reenable: bool,
) {
	let repo = Repository::new();
	let mut previous = spec.clone();
	previous.enabled = false;
	{
		let mut state = repo.state.lock().unwrap();
		state
			.persisted
			.insert("policy".into(), policy("policy", 7, previous));
		state.approved.clear();
	}
	spec.enabled = reenable;
	if !reenable {
		spec.permissions.attributes = json!({"team":"changed"});
	}
	assert!(
		matches!(set(&repo,"tenant","policy",7,&spec,&validation).await,Err(Error::Invalid(message)) if message=="generation components require tenant catalog approval")
	);
	let state = repo.state.lock().unwrap();
	assert_eq!(state.persisted["policy"].revision, 7);
	assert!(state.history.is_empty());
	assert!(!state.calls.iter().any(|call| call.starts_with("cas:")));
}
#[rstest]
#[case("history")]
#[case("load")]
#[case("commit")]
#[tokio::test]
async fn failures_after_cas_leave_no_revision_or_history_and_retry_once(
	spec: Spec,
	validation: DefinitionValidation,
	#[case] failure: &'static str,
) {
	let repo = Repository::new();
	repo.state.lock().unwrap().failure = Some(failure);
	let result = set(&repo, "tenant", "policy", 0, &spec, &validation).await;
	assert!(
		matches!(result,Err(Error::External(message)) if message==format!("fixture {failure} failed"))
	);
	{
		let state = repo.state.lock().unwrap();
		assert!(state.persisted.is_empty());
		assert!(state.history.is_empty());
		assert_eq!(
			(state.commits, state.rollbacks, state.active_scopes),
			(0, 1, 0)
		);
	}
	repo.state.lock().unwrap().failure = None;
	assert_eq!(
		set(&repo, "tenant", "policy", 0, &spec, &validation)
			.await
			.unwrap()
			.revision,
		1
	);
	let state = repo.state.lock().unwrap();
	assert_eq!(state.history.len(), 1);
	assert_eq!(state.persisted["policy"].revision, 1);
}
#[rstest]
#[case("cas")]
#[case("history")]
#[case("load")]
#[tokio::test]
async fn cancellation_drops_the_owned_scope_and_all_staged_effects(
	spec: Spec,
	validation: DefinitionValidation,
	#[case] pause: &'static str,
) {
	let repo = Repository::new();
	repo.state.lock().unwrap().pause = Some(pause);
	assert!(
		tokio::time::timeout(
			Duration::from_millis(10),
			set(&repo, "tenant", "policy", 0, &spec, &validation)
		)
		.await
		.is_err()
	);
	let state = repo.state.lock().unwrap();
	assert!(state.persisted.is_empty());
	assert!(state.history.is_empty());
	assert_eq!(
		(state.commits, state.rollbacks, state.active_scopes),
		(0, 1, 0)
	);
}
#[rstest]
#[tokio::test]
async fn stale_revision_does_not_append_history(spec: Spec, validation: DefinitionValidation) {
	let repo = Repository::new();
	repo.state
		.lock()
		.unwrap()
		.persisted
		.insert("policy".into(), policy("policy", 2, spec.clone()));
	assert!(
		matches!(set(&repo,"tenant","policy",1,&spec,&validation).await,Err(Error::Conflict(message)) if message=="generation policy revision changed")
	);
	let state = repo.state.lock().unwrap();
	assert_eq!(state.persisted["policy"].revision, 2);
	assert!(state.history.is_empty());
	assert!(!state.calls.iter().any(|call| call.starts_with("history:")));
}
#[rstest]
#[case::subject(true)]
#[case::operator(false)]
#[tokio::test]
async fn listing_keeps_order_and_shared_authority_while_filtering_subjects(
	spec: Spec,
	#[case] subject: bool,
) {
	let mut repo = Repository::new();
	if subject {
		repo.subject();
	}
	{
		let mut state = repo.state.lock().unwrap();
		for id in ["a", "b", "c"] {
			state
				.persisted
				.insert(id.into(), policy(id, 3, spec.clone()));
		}
		state.denied.insert("b".into());
	}
	let policies = list(&repo, "tenant").await.unwrap();
	let expected = if subject {
		vec!["a", "c"]
	} else {
		vec!["a", "b", "c"]
	};
	assert_eq!(
		policies
			.iter()
			.map(|policy| policy.id.as_str())
			.collect::<Vec<_>>(),
		expected
	);
	assert_eq!(&repo.calls()[..2], ["ids:tenant", "begin:tenant:false"]);
	assert_eq!(repo.calls().last().unwrap(), "finish");
	if subject {
		assert!(!repo.calls().contains(&"load:tenant:b:false".into()));
	} else {
		assert!(!repo.calls().iter().any(|call| call.starts_with("decide:")));
	}
	for policy in policies {
		assert_eq!(
			(
				policy.generated_count,
				policy.allocated_tokens,
				policy.allocated_compaction_calls,
				policy.allocated_embedding_calls
			),
			(2, 10000, 3, 4)
		);
	}
}
#[rstest]
#[tokio::test]
async fn cross_tenant_listing_preserves_discovery_order_but_never_acquires_authority() {
	let mut repo = Repository::new();
	repo.subject();
	assert!(matches!(list(&repo, "other").await, Err(Error::Forbidden)));
	assert_eq!(repo.calls(), vec!["ids:other"]);
}
#[rstest]
#[case("max_agents", 0)]
#[case("max_agents", 513)]
#[case("max_concurrent", 6)]
#[case("max_depth", 32)]
#[case("token_budget", 1000000000001_i64)]
#[case("tokens_per_agent", 1000001)]
#[case("lifetime_seconds", 2592001)]
fn bounded_generation_limits_reject_each_excess(
	mut spec: Spec,
	validation: DefinitionValidation,
	#[case] field: &str,
	#[case] value: i64,
) {
	let mut value_spec = json!(spec);
	value_spec["limits"][field] = json!(value);
	spec = serde_json::from_value(value_spec).unwrap();
	assert!(
		matches!(validate(&validation,&spec,&serde_json::from_value(bundle()).unwrap()),Err(Error::Invalid(message)) if message=="invalid generation limits")
	);
}
#[rstest]
#[case("role", "generated role does not exist")]
#[case("group", "generated group does not exist")]
#[case("array", "generated attributes must be an object of at most 16 KiB")]
#[case("large", "generated attributes must be an object of at most 16 KiB")]
#[case(
	"private",
	"private reference documents belong to a registered agent, not a generation template"
)]
fn policy_permissions_and_private_reference_boundary_remain_strict(
	mut spec: Spec,
	validation: DefinitionValidation,
	#[case] mutation: &str,
	#[case] message: &str,
) {
	match mutation {
		"role" => {
			spec.permissions.roles.insert("missing".into());
		}
		"group" => {
			spec.permissions.groups.insert("missing".into());
		}
		"array" => spec.permissions.attributes = json!([]),
		"large" => spec.permissions.attributes = json!({"large":"a".repeat(16385)}),
		"private" => {
			spec.template.config["knowledge_digest"] = json!("a".repeat(64));
			assert!(
				validate(
					&validation,
					&spec,
					&serde_json::from_value(bundle()).unwrap()
				)
				.is_err()
			);
			return;
		}
		_ => panic!("unknown fixture mutation"),
	}
	assert!(
		matches!(validate(&validation,&spec,&serde_json::from_value(bundle()).unwrap()),Err(Error::Invalid(actual)) if actual==message)
	);
}
#[rstest]
#[case::kind(true, "generation component must be a model")]
#[case::budget(false, "agent token allowance is smaller than one model reservation")]
#[tokio::test]
async fn approved_component_kind_and_model_reservation_are_checked_before_cas(
	mut spec: Spec,
	validation: DefinitionValidation,
	#[case] wrong_kind: bool,
	#[case] message: &str,
) {
	let repo = Repository::new();
	if wrong_kind {
		repo.state
			.lock()
			.unwrap()
			.approved
			.get_mut(&("model".into(), "1.0.0".into()))
			.unwrap()["kind"] = json!("tool");
	} else {
		spec.limits.tokens_per_agent = 5119;
	}
	assert!(
		matches!(set(&repo,"tenant","policy",0,&spec,&validation).await,Err(Error::Invalid(actual)) if actual==message)
	);
	let state = repo.state.lock().unwrap();
	assert!(state.persisted.is_empty());
	assert!(state.history.is_empty());
	assert!(!state.calls.iter().any(|call| call.starts_with("cas:")));
}
#[rstest]
#[tokio::test]
async fn corrupt_saved_metadata_retains_json_error_instead_of_becoming_a_permission_denial(
	spec: Spec,
	validation: DefinitionValidation,
) {
	let repo = Repository::new();
	repo.state.lock().unwrap().approved.insert(
		("model".into(), "1.0.0".into()),
		json!({"invalid":"metadata"}),
	);
	assert!(matches!(
		set(&repo, "tenant", "policy", 0, &spec, &validation).await,
		Err(Error::Json(_))
	));
	assert!(repo.state.lock().unwrap().persisted.is_empty());
}

#[rstest]
#[tokio::test]
async fn approved_private_source_cannot_escape_through_a_generation_template(
	mut spec: Spec,
	validation: DefinitionValidation,
) {
	let repo = Repository::new();
	let source = crate::test_support::entry(
		"registered-private",
		"source",
		json!({"schema_version":1,"source":{"adapter":"private_references","digest":"a".repeat(64)}}),
	);
	spec.template.config["bindings"] = json!([crate::test_support::binding(
		"source",
		"aidash://local",
		&source.id
	)]);
	repo.state
		.lock()
		.unwrap()
		.approved
		.insert((source.id.clone(), source.version.clone()), json!(source));
	assert!(
		matches!(set(&repo,"tenant","policy",0,&spec,&validation).await, Err(Error::Invalid(message)) if message.contains("private reference documents"))
	);
	let state = repo.state.lock().unwrap();
	assert!(state.persisted.is_empty());
	assert!(state.history.is_empty());
	assert!(!state.calls.iter().any(|call| call.starts_with("cas:")));
}
