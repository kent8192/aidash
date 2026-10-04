use super::*;
use crate::ports::registry::{DefinitionLookup, workbench::DraftAuthority};
use aidash_domain::registry::workbench::ShareRecord;
use aidash_domain::{
	identity::Principal,
	policy::{Decision, Evaluation, PolicyBundle},
	registry::Entry,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
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
	pages: Vec<Vec<Draft>>,
	operator: bool,
	managed: bool,
}
impl Repository {
	fn new(draft: Draft) -> Self {
		Self {
			draft,
			log: Arc::new(Mutex::new(Vec::new())),
			deny: false,
			pages: Vec::new(),
			operator: false,
			managed: false,
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
	pages: Vec<Vec<Draft>>,
	operator: bool,
	managed: bool,
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
		if self.operator {
			return Principal::Operator;
		}
		Principal::Subject {
			tenant: "tenant".into(),
			subject: "owner".into(),
		}
	}
	async fn original_entry(&self, id: &str, version: &str) -> Result<Entry> {
		self.log.lock().unwrap().push("original".into());
		let entry: Entry = serde_json::from_value(self.draft.entry.clone())?;
		assert_eq!((entry.id.as_str(), entry.version.as_str()), (id, version));
		Ok(entry)
	}
	async fn original_documents(&self, _: &Entry) -> Result<Value> {
		self.log.lock().unwrap().push("documents".into());
		Ok(self.draft.documents.clone())
	}
	async fn begin(&self) -> Result<Scope> {
		self.log.lock().unwrap().push("begin".into());
		Ok(Scope {
			draft: self.draft.clone(),
			log: self.log.clone(),
			deny: self.deny,
			pages: self.pages.clone(),
			operator: self.operator,
			managed: self.managed,
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
		if self.operator {
			return Principal::Operator;
		}
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
			json!({"tenant":"tenant","subjects":{"owner":{"kind":"user","delegated_by":null},"guest":{"kind":"user","delegated_by":null}}}),
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
	async fn page(
		&mut self,
		tenant: Option<&str>,
		cursor: Option<(DateTime<Utc>, Uuid)>,
	) -> Result<Vec<Draft>> {
		assert_eq!(tenant, Some("tenant"));
		self.record(format!(
			"page:{}",
			cursor.map_or_else(|| "none".into(), |(_, id)| id.to_string())
		));
		if self.pages.is_empty() {
			return Ok(Vec::new());
		}
		Ok(self.pages.remove(0))
	}
	async fn managed(&mut self, _: &str) -> Result<bool> {
		self.record("managed");
		Ok(self.managed)
	}
	async fn append_event(&mut self, kind: &str, payload: Value) -> Result<()> {
		self.record(format!("event:{kind}:{payload}"));
		Ok(())
	}
	async fn shares(&mut self, _: Uuid) -> Result<Vec<ShareRecord>> {
		self.record("shares");
		Ok(vec![
			ShareRecord {
				subject: "current".into(),
				can_edit: true,
				documents_digest: digest(&self.draft.documents),
			},
			ShareRecord {
				subject: "stale".into(),
				can_edit: false,
				documents_digest: "old".into(),
			},
		])
	}
	async fn save_share(
		&mut self,
		id: Uuid,
		subject: &str,
		can_edit: bool,
		documents_digest: &str,
	) -> Result<()> {
		assert_eq!(id, self.draft.id);
		assert_eq!(documents_digest, digest(&self.draft.documents));
		self.record(format!("share-save:{subject}:{can_edit}"));
		Ok(())
	}
	async fn remove_share(&mut self, id: Uuid, subject: &str) -> Result<()> {
		assert_eq!(id, self.draft.id);
		self.record(format!("share-remove:{subject}"));
		Ok(())
	}
	async fn transfer(&mut self, id: Uuid, owner: &str) -> Result<()> {
		assert_eq!(id, self.draft.id);
		self.record(format!("transfer:{owner}"));
		self.draft.owner = owner.into();
		self.draft.revision += 1;
		Ok(())
	}
	async fn archive(&mut self, id: Uuid, archived: bool) -> Result<()> {
		assert_eq!(id, self.draft.id);
		self.record(format!("archive:{archived}"));
		self.draft.archived = archived;
		self.draft.revision += 1;
		Ok(())
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

#[rstest]
#[tokio::test]
async fn get_checks_current_sharing_in_read_scope(draft: Draft) {
	let repository = Repository::new(draft.clone());
	let saved = get(&repository, draft.id).await.unwrap();
	assert_eq!(saved.id, draft.id);
	assert_eq!(
		repository.logs(),
		[
			"begin",
			"read:false",
			"identity",
			"share",
			"agent_draft.read",
			"commit"
		]
	);
}
#[rstest]
#[tokio::test]
async fn incomplete_cursor_does_not_query_a_page(draft: Draft) {
	let repository = Repository::new(draft.clone());
	assert_eq!(
		list(
			&repository,
			DraftPage {
				before_updated_at: Some(draft.updated_at),
				before_id: None
			}
		)
		.await
		.unwrap_err()
		.to_string(),
		"draft cursor requires both timestamp and ID"
	);
	assert_eq!(repository.logs(), ["begin"]);
}
#[rstest]
#[tokio::test]
async fn hidden_full_page_does_not_hide_later_visible_drafts(draft: Draft) {
	let mut repository = Repository::new(draft.clone());
	let hidden: Vec<_> = (1..=100)
		.map(|number| {
			let mut row = draft.clone();
			row.id = Uuid::from_u128(number);
			row.owner = "other".into();
			row
		})
		.collect();
	let after = hidden.last().unwrap().id;
	repository.pages = vec![hidden, vec![draft.clone()]];
	let visible = list(&repository, DraftPage::default()).await.unwrap();
	assert_eq!(
		visible.iter().map(|row| row.id).collect::<Vec<_>>(),
		[draft.id]
	);
	let pages: Vec<_> = repository
		.logs()
		.into_iter()
		.filter(|step| step.starts_with("page:"))
		.collect();
	assert_eq!(pages, ["page:none".to_owned(), format!("page:{after}")]);
	assert_eq!(repository.logs().last().map(String::as_str), Some("commit"));
}
#[rstest]
#[tokio::test]
async fn revoked_credential_drafts_are_not_disclosed(draft: Draft) {
	let mut repository = Repository::new(draft.clone());
	repository.deny = true;
	repository.pages = vec![vec![draft]];
	let visible = list(&repository, DraftPage::default()).await.unwrap();
	assert!(visible.is_empty());
	assert_eq!(
		repository.logs(),
		["begin", "page:none", "identity", "commit"]
	);
}

#[rstest]
#[tokio::test]
async fn duplicate_binds_new_identity_and_records_original_provenance(
	mut draft: Draft,
	mut entry: Entry,
) {
	entry.version = "2.0.0".into();
	draft.entry = serde_json::to_value(&entry).unwrap();
	let repository = Repository::new(draft.clone());
	let copied = duplicate(
		&repository,
		draft.id,
		RevisionInput {
			expected_revision: 3,
		},
	)
	.await
	.unwrap();
	assert_ne!(copied.id, draft.id);
	assert_eq!(copied.entry["id"], copied.id.to_string());
	assert_eq!(copied.entry["version"], "1.0.0");
	assert_eq!(copied.source_id, Some("managed".into()));
	assert_eq!(copied.source_version, Some("2.0.0".into()));
	assert_eq!(copied.owner, "owner");
	let logs = repository.logs();
	assert!(
		logs.iter()
			.any(|step| step.starts_with("event:agent_draft.duplicated:"))
	);
	assert_eq!(logs.last().map(String::as_str), Some("commit"));
}
#[rstest]
#[tokio::test]
async fn duplicate_checks_revision_before_allocating_or_writing(draft: Draft) {
	let repository = Repository::new(draft.clone());
	assert_eq!(
		duplicate(
			&repository,
			draft.id,
			RevisionInput {
				expected_revision: 2
			}
		)
		.await
		.unwrap_err()
		.to_string(),
		"draft revision changed"
	);
	assert!(
		!repository
			.logs()
			.iter()
			.any(|step| step == "insert" || step == "commit")
	);
}
#[rstest]
#[tokio::test]
async fn only_operator_can_adopt_existing_agents(draft: Draft) {
	let repository = Repository::new(draft);
	assert!(matches!(
		adopt(
			&repository,
			("managed".into(), "1.0.0".into()),
			AdoptInput {
				tenant: "tenant".into(),
				owner: "owner".into()
			}
		)
		.await,
		Err(Error::Forbidden)
	));
	assert!(repository.logs().is_empty());
}
#[rstest]
#[tokio::test]
async fn adoption_preserves_registry_identity_and_checks_managed_conflict(
	mut draft: Draft,
	entry: Entry,
) {
	draft.entry = serde_json::to_value(entry).unwrap();
	let mut repository = Repository::new(draft);
	repository.operator = true;
	repository.managed = true;
	assert_eq!(
		adopt(
			&repository,
			("managed".into(), "1.0.0".into()),
			AdoptInput {
				tenant: "tenant".into(),
				owner: "owner".into()
			}
		)
		.await
		.unwrap_err()
		.to_string(),
		"agent identity is already managed"
	);
	assert_eq!(
		repository.logs(),
		["original", "documents", "begin", "target", "managed"]
	);
}
#[rstest]
#[tokio::test]
async fn adopted_agent_keeps_original_version(mut draft: Draft, entry: Entry) {
	draft.entry = serde_json::to_value(entry).unwrap();
	let mut repository = Repository::new(draft);
	repository.operator = true;
	let adopted = adopt(
		&repository,
		("managed".into(), "1.0.0".into()),
		AdoptInput {
			tenant: "tenant".into(),
			owner: "owner".into(),
		},
	)
	.await
	.unwrap();
	assert_eq!(adopted.entry["id"], "managed");
	assert_eq!(adopted.source_version, Some("1.0.0".into()));
	assert_eq!(repository.logs().last().map(String::as_str), Some("commit"));
}

#[rstest]
#[tokio::test]
async fn sharing_private_documents_requires_explicit_consent(mut draft: Draft) {
	draft.documents = json!([{"private":"text"}]);
	let repository = Repository::new(draft.clone());
	assert_eq!(
		share(
			&repository,
			draft.id,
			ShareInput {
				subject: "guest".into(),
				can_edit: true,
				enabled: true,
				include_documents: false
			}
		)
		.await
		.unwrap_err()
		.to_string(),
		"sharing this draft also shares its private documents; acknowledge include_documents"
	);
	assert!(
		!repository
			.logs()
			.iter()
			.any(|step| step.starts_with("share-save") || step == "commit")
	);
}
#[rstest]
#[tokio::test]
async fn owner_sharing_uses_live_authority_and_current_digest(draft: Draft) {
	let repository = Repository::new(draft.clone());
	share(
		&repository,
		draft.id,
		ShareInput {
			subject: "guest".into(),
			can_edit: true,
			enabled: true,
			include_documents: true,
		},
	)
	.await
	.unwrap();
	let logs = repository.logs();
	assert_eq!(
		&logs[..6],
		[
			"begin",
			"read:true",
			"identity",
			"agent_draft.share",
			"target",
			"share-save:guest:true"
		]
	);
	assert!(logs[6].starts_with("event:agent_draft.share_changed:"));
	assert_eq!(logs[7], "commit");
}
#[rstest]
#[tokio::test]
async fn revocation_does_not_require_the_old_target_to_remain_enabled(draft: Draft) {
	let repository = Repository::new(draft.clone());
	share(
		&repository,
		draft.id,
		ShareInput {
			subject: "disabled".into(),
			can_edit: false,
			enabled: false,
			include_documents: false,
		},
	)
	.await
	.unwrap();
	assert!(repository.logs().contains(&"share-remove:disabled".into()));
	assert!(!repository.logs().contains(&"target".into()));
}
#[rstest]
#[tokio::test]
async fn share_listing_labels_current_private_document_consent(draft: Draft) {
	let repository = Repository::new(draft.clone());
	let grants = shares(&repository, draft.id).await.unwrap();
	assert_eq!(
		grants
			.iter()
			.map(|row| (row.subject.as_str(), row.documents_current))
			.collect::<Vec<_>>(),
		[("current", true), ("stale", false)]
	);
	assert_eq!(
		repository.logs(),
		[
			"begin",
			"read:false",
			"identity",
			"agent_draft.share",
			"shares",
			"commit"
		]
	);
}
#[rstest]
#[tokio::test]
async fn nonowners_cannot_share_before_policy_checks(mut draft: Draft) {
	draft.owner = "other".into();
	let repository = Repository::new(draft.clone());
	assert!(matches!(
		share(
			&repository,
			draft.id,
			ShareInput {
				subject: "guest".into(),
				can_edit: false,
				enabled: true,
				include_documents: true
			}
		)
		.await,
		Err(Error::Forbidden)
	));
	assert_eq!(repository.logs(), ["begin", "read:true"]);
}

#[rstest]
#[tokio::test]
async fn transfer_removes_new_owner_share_before_changing_owner(draft: Draft) {
	let repository = Repository::new(draft.clone());
	let saved = transfer(
		&repository,
		draft.id,
		TransferInput {
			expected_revision: 3,
			new_owner: "guest".into(),
		},
	)
	.await
	.unwrap();
	assert_eq!(saved.owner, "guest");
	assert_eq!(saved.revision, 4);
	let logs = repository.logs();
	assert_eq!(
		&logs[..7],
		[
			"begin",
			"read:true",
			"identity",
			"agent_draft.transfer",
			"target",
			"share-remove:guest",
			"transfer:guest"
		]
	);
	assert!(logs[7].starts_with("event:agent_draft.owner_changed:"));
	assert_eq!(&logs[8..], ["read:false", "commit"]);
}
#[rstest]
#[tokio::test]
async fn transfer_requires_current_revision_before_target_lookup(draft: Draft) {
	let repository = Repository::new(draft.clone());
	assert_eq!(
		transfer(
			&repository,
			draft.id,
			TransferInput {
				expected_revision: 2,
				new_owner: "guest".into()
			}
		)
		.await
		.unwrap_err()
		.to_string(),
		"draft revision changed"
	);
	assert_eq!(
		repository.logs(),
		["begin", "read:true", "identity", "agent_draft.transfer"]
	);
}
#[rstest]
#[tokio::test]
async fn disabled_transfer_target_does_not_mutate_draft(draft: Draft) {
	let repository = Repository::new(draft.clone());
	assert_eq!(
		transfer(
			&repository,
			draft.id,
			TransferInput {
				expected_revision: 3,
				new_owner: "missing".into()
			}
		)
		.await
		.unwrap_err()
		.to_string(),
		"target subject must exist and be enabled in the same tenant"
	);
	assert!(
		!repository
			.logs()
			.iter()
			.any(|step| step.starts_with("transfer:")
				|| step.starts_with("share-remove:")
				|| step == "commit")
	);
}

#[rstest]
#[case(true)]
#[case(false)]
#[tokio::test]
async fn archive_and_restore_retain_revision_fence(draft: Draft, #[case] archived: bool) {
	let repository = Repository::new(draft.clone());
	let saved = archive(
		&repository,
		draft.id,
		ArchiveInput {
			expected_revision: 3,
			archived,
		},
	)
	.await
	.unwrap();
	assert_eq!(saved.archived, archived);
	assert_eq!(saved.revision, 4);
	let logs = repository.logs();
	assert_eq!(
		&logs[..4],
		["begin", "read:true", "identity", "agent_draft.archive"]
	);
	assert_eq!(logs[4], format!("archive:{archived}"));
	assert!(logs[5].starts_with("event:agent_draft.archived_changed:"));
	assert_eq!(&logs[6..], ["read:false", "commit"]);
}
#[rstest]
#[tokio::test]
async fn stale_archive_does_not_write_or_commit(draft: Draft) {
	let repository = Repository::new(draft.clone());
	assert_eq!(
		archive(
			&repository,
			draft.id,
			ArchiveInput {
				expected_revision: 2,
				archived: true
			}
		)
		.await
		.unwrap_err()
		.to_string(),
		"draft revision changed"
	);
	assert_eq!(
		repository.logs(),
		["begin", "read:true", "identity", "agent_draft.archive"]
	);
}
