use super::*;
use crate::ports::registry::DefinitionLookup;
use aidash_domain::{
	policy::{Decision, PolicyBundle},
	registry::Entry,
};
use async_trait::async_trait;
use rstest::rstest;
use serde_json::Value;
use uuid::Uuid;
struct Scope {
	principal: Principal,
	allowed: bool,
	calls: Vec<&'static str>,
}
#[async_trait]
impl DefinitionLookup for Scope {
	async fn definition(&mut self, _: &str, _: &str) -> Result<Entry> {
		panic!("inspection authority does not read definitions")
	}
	async fn overrides(&mut self, _: &str, _: &str) -> Result<Option<Value>> {
		panic!("inspection authority does not apply overrides")
	}
}
#[async_trait]
impl DraftAuthority for Scope {
	fn principal(&self) -> Principal {
		self.principal.clone()
	}
	async fn lock_identity(&mut self) -> Result<()> {
		self.calls.push("identity");
		Ok(())
	}
	async fn share(&mut self, _: Uuid, _: &str) -> Result<Option<(bool, String)>> {
		panic!("inspection authority is independent of draft shares")
	}
	async fn evaluate(&mut self, tenant: &str, input: &Evaluation) -> Result<Decision> {
		assert_eq!(tenant, "tenant");
		assert_eq!(input.subject, "reader");
		assert_eq!(input.action, "agent_version.inspect");
		assert_eq!(input.resource.kind, "agent_version");
		assert_eq!(input.resource.id, "agent@1");
		assert_eq!(
			input.resource.attributes,
			json!({"agent_id":"agent","version":"1"})
		);
		assert_eq!(input.environment, json!({}));
		self.calls.push("policy");
		Ok(Decision {
			allowed: self.allowed,
			reason: "fixture".into(),
			revision: 1,
			matched_policies: vec![],
			effective_roles: Default::default(),
		})
	}
	async fn bundle(&mut self, _: &str) -> Result<PolicyBundle> {
		panic!("inspection authority does not enumerate policy bundle")
	}
}
#[rstest]
#[case(true)]
#[case(false)]
#[tokio::test]
async fn inspection_requires_current_identity_and_exact_agent_version_policy(
	#[case] allowed: bool,
) {
	let mut scope = Scope {
		principal: Principal::Subject {
			tenant: "tenant".into(),
			subject: "reader".into(),
		},
		allowed,
		calls: vec![],
	};
	let result = require(
		&mut scope,
		&EntityRef {
			id: "agent".into(),
			version: "1".into(),
		},
	)
	.await;
	assert_eq!(result.is_ok(), allowed);
	if !allowed {
		assert!(matches!(result, Err(Error::Forbidden)));
	}
	assert_eq!(scope.calls, vec!["identity", "policy"]);
}
#[rstest]
#[tokio::test]
async fn operator_inspection_preserves_its_existing_authority_without_subject_reads() {
	let mut scope = Scope {
		principal: Principal::Operator,
		allowed: false,
		calls: vec![],
	};
	require(
		&mut scope,
		&EntityRef {
			id: "agent".into(),
			version: "1".into(),
		},
	)
	.await
	.unwrap();
	assert!(scope.calls.is_empty());
}
