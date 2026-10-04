use super::*;
use crate::ports::registry::{DefinitionLookup, workbench::DraftAuthority};
use aidash_domain::{
	identity::Principal,
	policy::{Decision, Evaluation, PolicyBundle},
	registry::Entry,
};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
#[fixture]
fn draft() -> Draft {
	Draft {
		id: Uuid::from_u128(1),
		tenant: "tenant".into(),
		owner: "owner".into(),
		revision: 3,
		entry: json!({"id":"managed"}),
		documents: json!([]),
		release_notes: String::new(),
		source_id: None,
		source_version: None,
		archived: false,
		updated_at: chrono::DateTime::from_timestamp(1000, 0).unwrap(),
	}
}
#[fixture]
fn entry() -> Entry {
	serde_json::from_value(json!({"id":"managed","version":"1.0.0","kind":"agent","name":{},"description":{},"config":{"model":{"id":"model","version":"1.0.0"},"tools":[],"skills":[],"cluster":null}})).unwrap()
}
struct Repository {
	draft: Draft,
	log: Arc<Mutex<Vec<String>>>,
	deny: bool,
}
impl Repository {
	fn new(draft: Draft) -> Self {
		Self {
			draft,
			log: Arc::new(Mutex::new(Vec::new())),
			deny: false,
		}
	}
	fn logs(&self) -> Vec<String> {
		self.log.lock().unwrap().clone()
	}
}
struct Scope {
	draft: Draft,
	log: Arc<Mutex<Vec<String>>>,
	deny: bool,
}
impl Scope {
	fn record(&self, value: impl Into<String>) {
		self.log.lock().unwrap().push(value.into());
	}
}
#[async_trait]
impl DraftRepository for Repository {
	type Scope = Scope;
	fn principal(&self) -> Principal {
		Principal::Subject {
			tenant: "tenant".into(),
			subject: "owner".into(),
		}
	}
	async fn begin(&self) -> Result<Scope> {
		self.log.lock().unwrap().push("begin".into());
		Ok(Scope {
			draft: self.draft.clone(),
			log: self.log.clone(),
			deny: self.deny,
		})
	}
}
#[async_trait]
impl DefinitionLookup for Scope {
	async fn definition(&mut self, _: &str, _: &str) -> Result<Entry> {
		panic!("draft edits do not resolve definitions")
	}
	async fn overrides(&mut self, _: &str, _: &str) -> Result<Option<Value>> {
		panic!("draft edits do not apply overlays")
	}
}
#[async_trait]
impl DraftAuthority for Scope {
	fn principal(&self) -> Principal {
		Principal::Subject {
			tenant: "tenant".into(),
			subject: "owner".into(),
		}
	}
	async fn lock_identity(&mut self) -> Result<()> {
		self.record("identity");
		if self.deny {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
	async fn share(&mut self, _: Uuid, _: &str) -> Result<Option<(bool, String)>> {
		self.record("share");
		Ok(None)
	}
	async fn evaluate(&mut self, _: &str, evaluation: &Evaluation) -> Result<Decision> {
		self.record(&evaluation.action);
		Ok(Decision {
			allowed: true,
			reason: "fixture".into(),
			matched_policies: Vec::new(),
			effective_roles: Default::default(),
			revision: 1,
		})
	}
	async fn bundle(&mut self, _: &str) -> Result<PolicyBundle> {
		self.record("target");
		Ok(serde_json::from_value(
			json!({"tenant":"tenant","subjects":{"owner":{"kind":"user","delegated_by":null}}}),
		)
		.unwrap())
	}
}
#[async_trait]
impl DraftScope for Scope {
	async fn read(&mut self, id: Uuid, lock: bool) -> Result<Draft> {
		assert_eq!(id, self.draft.id);
		self.record(format!("read:{lock}"));
		Ok(self.draft.clone())
	}
	async fn insert(&mut self, draft: &Draft, managed_id: &str) -> Result<Draft> {
		self.record("insert");
		assert_eq!(draft.entry["id"], managed_id);
		assert_eq!(draft.revision, 1);
		Ok(draft.clone())
	}
	async fn save_content(
		&mut self,
		id: Uuid,
		entry: Value,
		documents: Value,
		notes: &str,
	) -> Result<Draft> {
		assert_eq!(id, self.draft.id);
		self.record("save");
		self.draft.entry = entry;
		self.draft.documents = documents;
		self.draft.release_notes = notes.into();
		self.draft.revision += 1;
		Ok(self.draft.clone())
	}
	async fn commit(self) -> Result<()> {
		self.record("commit");
		Ok(())
	}
}
#[rstest]
#[tokio::test]
async fn create_authorizes_enabled_owner_before_insert(draft: Draft, mut entry: Entry) {
	let repository = Repository::new(draft);
	entry.id.clear();
	let saved = create(
		&repository,
		CreateDraft {
			tenant: None,
			owner: None,
			entry,
			documents: Vec::new(),
			release_notes: String::new(),
		},
	)
	.await
	.unwrap();
	assert_eq!(saved.entry["id"], saved.id.to_string());
	assert_eq!(saved.entry["config"]["allow_task_delegation"], json!(false));
	assert_eq!(
		repository.logs(),
		[
			"begin",
			"target",
			"identity",
			"agent_draft.create",
			"insert",
			"commit"
		]
	);
}
#[rstest]
#[tokio::test]
async fn creation_rejects_existing_identity_before_transaction(draft: Draft, entry: Entry) {
	let repository = Repository::new(draft);
	let result = create(
		&repository,
		CreateDraft {
			tenant: None,
			owner: None,
			entry,
			documents: Vec::new(),
			release_notes: String::new(),
		},
	)
	.await;
	assert!(matches!(result, Err(Error::Invalid(_))));
	assert!(repository.logs().is_empty());
}
#[rstest]
#[tokio::test]
async fn edit_uses_locked_revision_and_commits_content(draft: Draft, entry: Entry) {
	let repository = Repository::new(draft.clone());
	let saved = save(
		&repository,
		draft.id,
		SaveDraft {
			expected_revision: 3,
			entry,
			documents: Vec::new(),
			release_notes: "notes".into(),
		},
	)
	.await
	.unwrap();
	assert_eq!(saved.revision, 4);
	assert_eq!(saved.release_notes, "notes");
	assert_eq!(
		repository.logs(),
		[
			"begin",
			"read:true",
			"identity",
			"share",
			"agent_draft.write",
			"save",
			"commit"
		]
	);
}
#[rstest]
#[case::revision(false, 2, "draft revision changed; local edits were not saved")]
#[case::archived(true, 3, "restore the archived draft before editing")]
#[tokio::test]
async fn stale_or_archived_edits_never_write(
	mut draft: Draft,
	entry: Entry,
	#[case] archived: bool,
	#[case] revision: i64,
	#[case] message: &str,
) {
	draft.archived = archived;
	let repository = Repository::new(draft.clone());
	assert_eq!(
		save(
			&repository,
			draft.id,
			SaveDraft {
				expected_revision: revision,
				entry,
				documents: Vec::new(),
				release_notes: String::new()
			}
		)
		.await
		.unwrap_err()
		.to_string(),
		message
	);
	assert!(
		!repository
			.logs()
			.iter()
			.any(|step| step == "save" || step == "commit")
	);
}
#[rstest]
#[tokio::test]
async fn credential_revocation_precedes_revision_checks(draft: Draft, entry: Entry) {
	let mut repository = Repository::new(draft.clone());
	repository.deny = true;
	assert!(matches!(
		save(
			&repository,
			draft.id,
			SaveDraft {
				expected_revision: 0,
				entry,
				documents: Vec::new(),
				release_notes: String::new()
			}
		)
		.await,
		Err(Error::Forbidden)
	));
	assert_eq!(repository.logs(), ["begin", "read:true", "identity"]);
}
#[rstest]
#[tokio::test]
async fn edits_cannot_rebind_managed_agent_identity(draft: Draft, mut entry: Entry) {
	entry.id = "other".into();
	let repository = Repository::new(draft.clone());
	assert_eq!(
		save(
			&repository,
			draft.id,
			SaveDraft {
				expected_revision: 3,
				entry,
				documents: Vec::new(),
				release_notes: String::new()
			}
		)
		.await
		.unwrap_err()
		.to_string(),
		"managed agent identity cannot change"
	);
	assert!(
		!repository
			.logs()
			.iter()
			.any(|step| step == "save" || step == "commit")
	);
}
