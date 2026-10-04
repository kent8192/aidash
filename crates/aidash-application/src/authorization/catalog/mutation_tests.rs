use super::*;
use crate::ports::catalog::{CatalogAdministrationRead, CatalogAdministrator, CatalogMutation};
use aidash_domain::identity::{Principal, catalog::Binding};
use async_trait::async_trait;
use rstest::rstest;
use serde_json::json;

struct Fixture {
	principal: Principal,
	definition: Entry,
	registered: bool,
	next: Option<Binding>,
	failure: Option<&'static str>,
	calls: Vec<String>,
}
fn reference() -> EntityRef {
	EntityRef {
		id: "skill".into(),
		version: "1.0.0".into(),
	}
}
fn binding() -> Binding {
	Binding {
		tenant: "tenant".into(),
		entry_id: "skill".into(),
		entry_version: "1.0.0".into(),
		enabled: false,
		revision: 2,
	}
}
impl Fixture {
	fn new() -> Self {
		Self { principal:Principal::Operator, definition:serde_json::from_value(json!({"id":"skill","version":"1.0.0","kind":"skill","name":{"en":"Skill"},"description":{"en":"Fixture"},"config":{"instructions":"Frozen"}})).unwrap(), registered:true, next:Some(binding()), failure:None, calls:vec![] }
	}
}
#[async_trait]
impl CatalogMutation for Fixture {
	async fn registered(&mut self, reference: &EntityRef) -> Result<bool> {
		self.calls
			.push(format!("exists:{}:{}", reference.id, reference.version));
		Ok(self.registered)
	}
	async fn compare_and_set(
		&mut self,
		tenant: &str,
		reference: &EntityRef,
		revision: i64,
		enabled: bool,
	) -> Result<Option<Binding>> {
		self.calls.push(format!(
			"cas:{tenant}:{}:{}:{revision}:{enabled}",
			reference.id, reference.version
		));
		if self.failure == Some("cas") {
			return Err(Error::External("fixture CAS failed".into()));
		}
		Ok(self.next.clone())
	}
	async fn history(&mut self, binding: &Binding, actor: &str) -> Result<()> {
		self.calls.push(format!(
			"history:{}:{}:{}:{}:{}:{actor}",
			binding.tenant,
			binding.entry_id,
			binding.entry_version,
			binding.revision,
			binding.enabled
		));
		if self.failure == Some("history") {
			return Err(Error::External("fixture history failed".into()));
		}
		Ok(())
	}
}
#[async_trait]
impl CatalogAdministrator for Fixture {
	fn principal(&self) -> &Principal {
		&self.principal
	}
	async fn load_tenant(&mut self, tenant: &str) -> Result<()> {
		self.calls.push(format!("load:{tenant}"));
		Ok(())
	}
	async fn definition(&mut self, reference: &EntityRef) -> Result<Entry> {
		self.calls
			.push(format!("definition:{}:{}", reference.id, reference.version));
		Ok(self.definition.clone())
	}
}
#[async_trait]
impl CatalogAdministrationRead for Fixture {
	fn principal(&self) -> &Principal {
		&self.principal
	}
	async fn bindings(&mut self, tenant: &str) -> Result<Vec<Binding>> {
		self.calls.push(format!("bindings:{tenant}"));
		Ok(vec![binding()])
	}
}

#[rstest]
#[tokio::test]
async fn operator_history_label_cannot_elevate_a_subject_before_persistence() {
	let mut scope = Fixture::new();
	scope.principal = Principal::Subject {
		tenant: "tenant".into(),
		subject: "alice".into(),
	};
	let result = set_catalog(&mut scope, "tenant", &reference(), 1, false, "operator").await;
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(scope.calls, Vec::<String>::new());
}
#[rstest]
#[tokio::test]
async fn administration_read_requires_operator_authority_for_worker_callers_too() {
	let mut scope = Fixture::new();
	scope.principal = Principal::Subject {
		tenant: "tenant".into(),
		subject: "alice".into(),
	};
	let result = administration(&mut scope, "tenant").await;
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(scope.calls, Vec::<String>::new());
}
#[rstest]
#[case(-1)]
#[case(i64::MAX)]
#[tokio::test]
async fn invalid_revision_cannot_read_or_mutate_catalog(#[case] revision: i64) {
	let mut scope = Fixture::new();
	let result = set_catalog(
		&mut scope,
		"tenant",
		&reference(),
		revision,
		false,
		"operator",
	)
	.await;
	assert!(
		matches!(result,Err(Error::Domain(aidash_domain::Error::Invalid(message))) if message=="invalid catalog revision")
	);
	assert_eq!(scope.calls, Vec::<String>::new());
}
#[rstest]
#[tokio::test]
async fn missing_definition_cannot_create_approval_or_history() {
	let mut scope = Fixture::new();
	scope.registered = false;
	let result = set_in(&mut scope, "tenant", &reference(), 0, true, "operator").await;
	assert!(matches!(result,Err(Error::NotFound(message)) if message=="registry entry"));
	assert_eq!(scope.calls, vec!["exists:skill:1.0.0"]);
}
#[rstest]
#[tokio::test]
async fn lost_revision_race_never_appends_a_new_history_row() {
	let mut scope = Fixture::new();
	scope.next = None;
	let result = set_in(&mut scope, "tenant", &reference(), 1, false, "operator").await;
	assert!(matches!(result,Err(Error::Conflict(message)) if message=="catalog revision changed"));
	assert_eq!(
		scope.calls,
		vec!["exists:skill:1.0.0", "cas:tenant:skill:1.0.0:1:false"]
	);
}
#[rstest]
#[tokio::test]
async fn failed_update_keeps_its_retry_error_without_history() {
	let mut scope = Fixture::new();
	scope.failure = Some("cas");
	let result = set_in(&mut scope, "tenant", &reference(), 1, false, "operator").await;
	assert!(matches!(result,Err(Error::External(message)) if message=="fixture CAS failed"));
	assert_eq!(
		scope.calls,
		vec!["exists:skill:1.0.0", "cas:tenant:skill:1.0.0:1:false"]
	);
}
#[rstest]
#[tokio::test]
async fn failed_history_returns_the_failure_to_the_transaction_owner() {
	let mut scope = Fixture::new();
	scope.failure = Some("history");
	let result = set_in(&mut scope, "tenant", &reference(), 1, false, "operator").await;
	assert!(matches!(result,Err(Error::External(message)) if message=="fixture history failed"));
	assert_eq!(
		scope.calls.last().unwrap(),
		"history:tenant:skill:1.0.0:2:false:operator"
	);
}
#[rstest]
#[tokio::test]
async fn administrator_rechecks_tenant_and_ownership_before_cas_and_history() {
	let mut scope = Fixture::new();
	let result = set_catalog(
		&mut scope,
		"tenant",
		&reference(),
		1,
		false,
		"maintenance-label",
	)
	.await
	.unwrap();
	assert_eq!(result, binding());
	assert_eq!(
		scope.calls,
		vec![
			"load:tenant",
			"definition:skill:1.0.0",
			"exists:skill:1.0.0",
			"cas:tenant:skill:1.0.0:1:false",
			"history:tenant:skill:1.0.0:2:false:maintenance-label"
		]
	);
	assert_eq!(
		serde_json::to_value(result).unwrap(),
		json!({"tenant":"tenant","entry_id":"skill","entry_version":"1.0.0","enabled":false,"revision":2})
	);
}
#[rstest]
#[case::enable(true)]
#[case::disable(false)]
#[tokio::test]
async fn projected_catalog_approvals_are_owned_by_marketplace_activation(#[case] enabled: bool) {
	let mut scope = Fixture::new();
	scope.definition.installation = Some(aidash_domain::registry::Projection {
		contract: 1,
		tenant: "tenant".into(),
		installation: "installation".into(),
		revision: 1,
	});
	let result = set_catalog(&mut scope, "tenant", &reference(), 1, enabled, "operator").await;
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(scope.calls, vec!["load:tenant", "definition:skill:1.0.0"]);
}
#[rstest]
#[tokio::test]
async fn marketplace_uses_the_same_cas_inside_its_existing_protected_scope() {
	let mut scope = Fixture::new();
	let result = set_in(
		&mut scope,
		"tenant",
		&reference(),
		1,
		false,
		"operator-activation",
	)
	.await
	.unwrap();
	assert_eq!(result, binding());
	assert_eq!(
		scope.calls,
		vec![
			"exists:skill:1.0.0",
			"cas:tenant:skill:1.0.0:1:false",
			"history:tenant:skill:1.0.0:2:false:operator-activation"
		]
	);
}
#[rstest]
#[tokio::test]
async fn administrator_listing_retains_the_persisted_approval_state() {
	let mut scope = Fixture::new();
	let result = administration(&mut scope, "tenant").await.unwrap();
	assert_eq!(result, vec![binding()]);
	assert_eq!(scope.calls, vec!["bindings:tenant"]);
}
