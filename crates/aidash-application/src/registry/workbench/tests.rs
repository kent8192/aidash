use super::*;
use crate::ports::registry::DefinitionLookup;
use aidash_domain::policy::{Decision, PolicyBundle};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::Value;
use uuid::Uuid;
#[fixture]
fn draft() -> Draft {
	Draft {
		id: Uuid::from_u128(1),
		tenant: "tenant".into(),
		owner: "owner".into(),
		revision: 3,
		entry: json!({"id":"agent"}),
		documents: json!([]),
		release_notes: String::new(),
		source_id: None,
		source_version: None,
		archived: false,
		updated_at: chrono::DateTime::from_timestamp(1000, 0).unwrap(),
	}
}
struct Fake {
	principal: Principal,
	log: Vec<&'static str>,
	share: Option<(bool, String)>,
	allowed: bool,
}
impl Fake {
	fn new(subject: &str) -> Self {
		Self {
			principal: Principal::Subject {
				tenant: "tenant".into(),
				subject: subject.into(),
			},
			log: Vec::new(),
			share: None,
			allowed: true,
		}
	}
}
#[async_trait]
impl DefinitionLookup for Fake {
	async fn definition(&mut self, _: &str, _: &str) -> Result<Entry> {
		panic!("authority does not read definitions")
	}
	async fn overrides(&mut self, _: &str, _: &str) -> Result<Option<Value>> {
		panic!("authority does not read overrides")
	}
}
#[async_trait]
impl DraftAuthority for Fake {
	fn principal(&self) -> Principal {
		self.principal.clone()
	}
	async fn lock_identity(&mut self) -> Result<()> {
		self.log.push("credential");
		Ok(())
	}
	async fn share(&mut self, _: Uuid, _: &str) -> Result<Option<(bool, String)>> {
		self.log.push("share");
		Ok(self.share.clone())
	}
	async fn evaluate(&mut self, _: &str, evaluation: &Evaluation) -> Result<Decision> {
		self.log.push("policy");
		assert_eq!(evaluation.resource.kind, "agent_draft");
		Ok(Decision {
			allowed: self.allowed,
			reason: "fixture".into(),
			matched_policies: Vec::new(),
			effective_roles: Default::default(),
			revision: 1,
		})
	}
	async fn bundle(&mut self, _: &str) -> Result<PolicyBundle> {
		panic!("not a target check")
	}
}
#[rstest]
#[tokio::test]
async fn owner_still_requires_current_credential_and_policy(draft: Draft) {
	let mut scope = Fake::new("owner");
	scope.allowed = false;
	assert!(matches!(
		authorize(&mut scope, &draft, "agent_draft.write", false).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.log, ["credential", "policy"]);
}
#[rstest]
#[tokio::test]
async fn tenant_mismatch_does_not_lock_or_disclose(draft: Draft) {
	let mut scope = Fake::new("owner");
	scope.principal = Principal::Subject {
		tenant: "other".into(),
		subject: "owner".into(),
	};
	assert!(matches!(
		authorize(&mut scope, &draft, "agent_draft.read", true).await,
		Err(Error::Forbidden)
	));
	assert!(scope.log.is_empty());
}
#[rstest]
#[case("agent_draft.read", false, true)]
#[case("agent_draft.write", false, false)]
#[case("agent_draft.register", true, true)]
#[tokio::test]
async fn sharing_distinguishes_read_and_edit(
	draft: Draft,
	#[case] action: &str,
	#[case] can_edit: bool,
	#[case] expected: bool,
) {
	let mut scope = Fake::new("guest");
	scope.share = Some((can_edit, digest(&draft.documents)));
	assert_eq!(
		authorize(&mut scope, &draft, action, true).await.is_ok(),
		expected
	);
	assert_eq!(&scope.log[..2], ["credential", "share"]);
	assert_eq!(
		scope.log.last(),
		Some(&if expected { "policy" } else { "share" })
	);
}
#[rstest]
#[tokio::test]
async fn changed_private_documents_require_new_consent(mut draft: Draft) {
	let mut scope = Fake::new("guest");
	scope.share = Some((true, digest(&draft.documents)));
	draft.documents = json!([{"private":"new"}]);
	assert!(matches!(
		authorize(&mut scope, &draft, "agent_draft.read", true).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.log, ["credential", "share"]);
}
#[rstest]
#[tokio::test]
async fn sharing_is_not_accepted_for_owner_operations(draft: Draft) {
	let mut scope = Fake::new("guest");
	scope.share = Some((true, digest(&draft.documents)));
	assert!(matches!(
		authorize(&mut scope, &draft, "agent_draft.share", false).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.log, ["credential"]);
}
#[rstest]
#[tokio::test]
async fn operator_authority_does_not_query_subject_grants(draft: Draft) {
	let mut scope = Fake::new("ignored");
	scope.principal = Principal::Operator;
	authorize(&mut scope, &draft, "agent_draft.register", true)
		.await
		.unwrap();
	assert!(scope.log.is_empty());
}
#[rstest]
fn subject_identity_cannot_be_overridden(draft: Draft) {
	let actor = Principal::Subject {
		tenant: draft.tenant.clone(),
		subject: draft.owner.clone(),
	};
	assert_eq!(
		author_identity(&actor, None, None).unwrap(),
		(draft.tenant, draft.owner)
	);
	assert!(matches!(
		author_identity(&actor, Some("other"), None),
		Err(Error::Forbidden)
	));
}
#[rstest]
#[case(None, Some("owner"), "tenant is required")]
#[case(Some("tenant"), None, "owner is required")]
#[case(Some(" "), Some("owner"), "tenant is required")]
fn operator_requires_explicit_identity(
	#[case] tenant: Option<&str>,
	#[case] owner: Option<&str>,
	#[case] message: &str,
) {
	assert_eq!(
		author_identity(&Principal::Operator, tenant, owner)
			.unwrap_err()
			.to_string(),
		message
	);
}
