use super::*;
use aidash_domain::policy::Resource;
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::json;

struct Scope {
	inherited: bool,
	approved: Vec<EntityRef>,
	enabled: Option<bool>,
	allowed: bool,
	calls: Vec<String>,
	fail: Option<&'static str>,
}
#[fixture]
fn scope() -> Scope {
	Scope {
		inherited: false,
		approved: vec![],
		enabled: Some(true),
		allowed: true,
		calls: vec![],
		fail: None,
	}
}
fn expected_reference() -> EntityRef {
	EntityRef {
		id: "retained".into(),
		version: "1.0.0".into(),
	}
}
fn definition() -> Entry {
	serde_json::from_value(json!({"id":"retained","version":"1.0.0","kind":"skill","name":{"en":"Retained"},"description":{"en":"Frozen definition"},"config":{"instructions":"Frozen"}})).unwrap()
}
impl Scope {
	fn touch(&mut self, name: &'static str) -> Result<()> {
		self.calls.push(name.into());
		if self.fail == Some(name) {
			return Err(Error::Port(Box::new(std::io::Error::other(name))));
		}
		Ok(())
	}
}
#[async_trait]
impl RetainedCatalogScope for Scope {
	fn inherited(&self) -> bool {
		self.inherited
	}
	fn approved(&self, reference: &EntityRef) -> bool {
		self.approved.contains(reference)
	}
	fn remember(&mut self, reference: &EntityRef) {
		self.calls.push("remember".into());
		self.approved.push(reference.clone());
	}
	async fn enabled(&mut self, reference: &EntityRef, lock: bool) -> Result<Option<bool>> {
		assert_eq!(reference, &expected_reference());
		self.calls.push(format!("lock:{lock}"));
		self.touch("enabled")?;
		Ok(self.enabled)
	}
	async fn definition(&mut self, reference: &EntityRef) -> Result<Entry> {
		assert_eq!(reference, &expected_reference());
		self.touch("definition")?;
		if self.fail == Some("missing") {
			Err(Error::NotFound("removed definition".into()))
		} else {
			Ok(definition())
		}
	}
	fn resource(&self, entry: &Entry) -> Resource {
		Resource {
			tenant: "tenant".into(),
			kind: entry.kind.clone(),
			id: entry.id.clone(),
			attributes: super::super::attributes(entry),
		}
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		assert_eq!(action, "registry.read");
		assert_eq!(resource.id, "retained");
		assert_eq!(resource.attributes["version"], "1.0.0");
		self.touch("decide")?;
		Ok(self.allowed)
	}
}
#[rstest]
#[case::normal(false, true)]
#[case::inherited(true, false)]
#[tokio::test]
async fn current_binding_definition_and_policy_precede_approval(
	mut scope: Scope,
	#[case] inherited: bool,
	#[case] lock: bool,
) {
	scope.inherited = inherited;
	scope.approved.push(expected_reference());
	let result = entry(&mut scope, &expected_reference()).await.unwrap();
	assert_eq!(result, definition());
	assert_eq!(
		scope.calls,
		vec![
			format!("lock:{lock}"),
			"enabled".into(),
			"definition".into(),
			"decide".into(),
			"remember".into()
		]
	);
}
#[rstest]
#[case::missing(None)]
#[case::disabled(Some(false))]
#[tokio::test]
async fn disabled_binding_cannot_load_or_approve_definition(
	mut scope: Scope,
	#[case] enabled: Option<bool>,
) {
	scope.enabled = enabled;
	assert!(matches!(
		entry(&mut scope, &expected_reference()).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, vec!["lock:true", "enabled"]);
	assert!(scope.approved.is_empty());
}
#[rstest]
#[case::unapproved(None)]
#[case::other_id(Some(("other","1.0.0")))]
#[case::other_version(Some(("retained","2.0.0")))]
#[tokio::test]
async fn inherited_frontier_requires_exact_saved_reference(
	mut scope: Scope,
	#[case] approved: Option<(&str, &str)>,
) {
	scope.inherited = true;
	if let Some((id, version)) = approved {
		scope.approved.push(EntityRef {
			id: id.into(),
			version: version.into(),
		});
	}
	assert!(matches!(
		entry(&mut scope, &expected_reference()).await,
		Err(Error::Forbidden)
	));
	assert!(scope.calls.is_empty());
}
#[rstest]
#[tokio::test]
async fn denied_policy_does_not_remember_a_definition(mut scope: Scope) {
	scope.allowed = false;
	assert!(matches!(
		entry(&mut scope, &expected_reference()).await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		scope.calls,
		vec!["lock:true", "enabled", "definition", "decide"]
	);
	assert!(scope.approved.is_empty());
}
#[rstest]
#[tokio::test]
async fn missing_retained_definition_is_a_disclosure_denial(mut scope: Scope) {
	scope.fail = Some("missing");
	assert!(matches!(
		entry(&mut scope, &expected_reference()).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, vec!["lock:true", "enabled", "definition"]);
}
#[rstest]
#[case::binding("enabled")]
#[case::definition("definition")]
#[case::policy("decide")]
#[tokio::test]
async fn adapter_errors_keep_their_identity_and_never_remember(
	mut scope: Scope,
	#[case] fail: &'static str,
) {
	scope.fail = Some(fail);
	let Error::Port(error) = entry(&mut scope, &expected_reference()).await.unwrap_err() else {
		panic!("expected opaque adapter error");
	};
	assert!(error.is::<std::io::Error>());
	assert_eq!(error.to_string(), fail);
	assert_eq!(scope.calls.last().unwrap(), fail);
	assert!(scope.approved.is_empty());
}
