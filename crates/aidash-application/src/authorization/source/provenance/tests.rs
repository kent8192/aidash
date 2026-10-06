use super::*;
use crate::authorization::visits::{ReadVisit, ReadVisits};
use aidash_domain::semantic::{Source, mutations::Entry, remote::SourceRead};
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use rstest::rstest;
use serde_json::json;
struct Scope {
	visits: ReadVisits,
	entry: Option<Entry>,
	permitted: bool,
	text: Option<String>,
	blocked: bool,
	calls: Vec<&'static str>,
}
fn key() -> (String, Uuid, String) {
	(
		"aidash://home".into(),
		Uuid::from_u128(1),
		"semantic:producer".into(),
	)
}
impl Scope {
	fn new() -> Self {
		Self {
			visits: Default::default(),
			entry: Some(Entry {
				id: Uuid::from_u128(2),
				workspace_id: Uuid::from_u128(3),
				key: "memory".into(),
				source: json!({"kind":"memory","text":"source text"}),
				agent: None,
				metadata: json!({}),
				revision: 7,
				point_id: Uuid::from_u128(4),
				index_revision: 5,
				deleted: false,
				state: "READY".into(),
				attempts: 0,
				last_error: None,
				created_by: "producer".into(),
				updated_at: Utc.timestamp_opt(0, 0).unwrap(),
			}),
			permitted: true,
			text: Some("source text".into()),
			blocked: false,
			calls: vec![],
		}
	}
}
#[async_trait]
impl SemanticSourceScope for Scope {
	fn source_visit(&self, _: Uuid) -> Option<ReadVisit> {
		self.visits.enter(key())
	}
	async fn disclosed_sources(&mut self, _: Uuid) -> Result<Vec<SourceRead>> {
		self.calls.push("sources");
		Ok(vec![SourceRead {
			entry_id: Uuid::from_u128(2),
			revision: 7,
			content_digest: content_digest("source text"),
		}])
	}
	async fn disclosed_entry(&mut self, id: Uuid) -> Result<Option<Entry>> {
		assert_eq!(id, Uuid::from_u128(2));
		self.calls.push("entry");
		Ok(self.entry.clone())
	}
	async fn source_permitted(&mut self, _: &Entry) -> Result<bool> {
		self.calls.push("permission");
		if self.blocked {
			std::future::pending::<()>().await;
		}
		Ok(self.permitted)
	}
	async fn source_text(&mut self, workspace: Uuid, source: &Source) -> Result<Option<String>> {
		assert_eq!(workspace, Uuid::from_u128(3));
		assert_eq!(
			*source,
			Source::Memory {
				text: "source text".into()
			}
		);
		self.calls.push("text");
		Ok(self.text.clone())
	}
}
#[rstest]
#[tokio::test]
async fn exact_revision_and_digest_require_current_permission_before_content() {
	let mut scope = Scope::new();
	verify(&mut scope, Uuid::from_u128(1)).await.unwrap();
	assert_eq!(scope.calls, vec!["sources", "entry", "permission", "text"]);
	assert!(scope.visits.enter(key()).is_some());
}
#[rstest]
#[case("missing")]
#[case("deleted")]
#[case("revision")]
#[tokio::test]
async fn changed_source_is_invalidated_before_permission_or_content(#[case] change: &str) {
	let mut scope = Scope::new();
	match change {
		"missing" => scope.entry = None,
		"deleted" => scope.entry.as_mut().unwrap().deleted = true,
		"revision" => scope.entry.as_mut().unwrap().revision += 1,
		_ => panic!("unknown source change"),
	}
	assert!(matches!(
		verify(&mut scope, Uuid::from_u128(1)).await,
		Err(Error::RemoteSemantic(Failure::Invalidated))
	));
	assert_eq!(scope.calls, vec!["sources", "entry"]);
	assert!(scope.visits.enter(key()).is_some());
}
#[rstest]
#[tokio::test]
async fn revoked_permission_does_not_read_the_source_text() {
	let mut scope = Scope::new();
	scope.permitted = false;
	assert!(matches!(
		verify(&mut scope, Uuid::from_u128(1)).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, vec!["sources", "entry", "permission"]);
}
#[rstest]
#[tokio::test]
async fn unavailable_source_remains_an_authority_denial() {
	let mut scope = Scope::new();
	scope.text = None;
	assert!(matches!(
		verify(&mut scope, Uuid::from_u128(1)).await,
		Err(Error::Forbidden)
	));
	assert_eq!(scope.calls, vec!["sources", "entry", "permission", "text"]);
}
#[rstest]
#[tokio::test]
async fn linked_content_change_invalidates_even_the_same_entry_revision() {
	let mut scope = Scope::new();
	scope.text = Some("updated linked content".into());
	assert!(matches!(
		verify(&mut scope, Uuid::from_u128(1)).await,
		Err(Error::RemoteSemantic(Failure::Invalidated))
	));
	assert_eq!(scope.calls, vec!["sources", "entry", "permission", "text"]);
}
#[rstest]
#[tokio::test]
async fn recursive_check_does_not_start_another_disclosure_read() {
	let mut scope = Scope::new();
	let _visit = scope.visits.enter(key()).unwrap();
	verify(&mut scope, Uuid::from_u128(1)).await.unwrap();
	assert!(scope.calls.is_empty());
}
#[rstest]
#[tokio::test]
async fn cancellation_releases_the_producer_visit_guard() {
	let mut scope = Scope::new();
	scope.blocked = true;
	let mut pending = Box::pin(verify(&mut scope, Uuid::from_u128(1)));
	assert!(futures_util::poll!(&mut pending).is_pending());
	drop(pending);
	assert_eq!(scope.calls, vec!["sources", "entry", "permission"]);
	assert!(scope.visits.enter(key()).is_some());
}
