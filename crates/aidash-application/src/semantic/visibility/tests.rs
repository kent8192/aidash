use super::*;
use crate::Error;
use aidash_domain::{Message, policy::Resource};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::Value;
use std::collections::BTreeSet;

#[fixture]
fn entry() -> Entry {
	Entry {
		id: Uuid::from_u128(1),
		thread_id: Uuid::from_u128(2),
		key: "stored-key".into(),
		source: json!({"kind":"memory","text":"saved text"}),
		agent: Some("stored-agent".into()),
		metadata: json!({"stored":true}),
		revision: 3,
		point_id: Uuid::from_u128(4),
		index_revision: 5,
		deleted: false,
		state: "READY".into(),
		attempts: 0,
		last_error: None,
		created_by: "saved-author".into(),
		updated_at: chrono::Utc::now(),
	}
}
#[fixture]
fn message() -> Message {
	Message {
		id: Uuid::from_u128(8),
		thread_id: Uuid::from_u128(2),
		sender: "stored-sender".into(),
		content: "original message bytes\n".into(),
		completion: None,
		idempotency_key: None,
		created_at: chrono::Utc::now(),
		run_id: None,
	}
}
struct Scope {
	scoped: bool,
	deployment_administrator: bool,
	managed: Option<(String, String)>,
	message: Option<Message>,
	visible: bool,
	denied: BTreeSet<String>,
	calls: Vec<&'static str>,
	resources: Vec<(Resource, String)>,
	lookups: Vec<(Uuid, Uuid)>,
	fail: Option<&'static str>,
}
#[fixture]
fn scope() -> Scope {
	Scope {
		scoped: true,
		deployment_administrator: true,
		managed: None,
		message: Some(message()),
		visible: true,
		denied: BTreeSet::new(),
		calls: vec![],
		resources: vec![],
		lookups: vec![],
		fail: None,
	}
}
impl Scope {
	fn touch(&mut self, name: &'static str) -> Result<()> {
		self.calls.push(name);
		if self.fail == Some(name) {
			Err(Error::Port(Box::new(std::io::Error::other(name))))
		} else {
			Ok(())
		}
	}
}
#[async_trait]
impl SemanticDisclosureScope for Scope {
	fn scoped(&self) -> bool {
		self.scoped
	}
	async fn deployment_administrator_visible(&mut self, thread: Uuid) -> Result<bool> {
		assert_eq!(thread, Uuid::from_u128(2));
		self.touch("deployment_administrator")?;
		Ok(self.deployment_administrator)
	}
	async fn thread_resource(&mut self, thread: Uuid) -> Result<Resource> {
		assert_eq!(thread, Uuid::from_u128(2));
		self.touch("thread")?;
		Ok(Resource {
			tenant: "tenant".into(),
			kind: "thread".into(),
			id: thread.to_string(),
			attributes: json!({"owner":"saved-owner","thread_id":thread}),
		})
	}
	fn resource(&mut self, kind: &str, id: &str, attributes: Value) -> Resource {
		Resource {
			tenant: "tenant".into(),
			kind: kind.into(),
			id: id.into(),
			attributes,
		}
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		let call = if resource.kind == "semantic" {
			"semantic_policy"
		} else {
			"memory_policy"
		};
		self.touch(call)?;
		self.resources.push((resource.clone(), action.into()));
		Ok(!self.denied.contains(action))
	}
	async fn message(&mut self, id: Uuid, thread: Uuid) -> Result<Option<Message>> {
		self.touch("message")?;
		self.lookups.push((id, thread));
		Ok(self.message.clone())
	}
	async fn message_visible(&mut self, row: &Message) -> Result<bool> {
		assert_eq!(row.id, Uuid::from_u128(8));
		self.touch("message_visible")?;
		Ok(self.visible)
	}
}
#[rstest]
#[tokio::test]
async fn saved_authorship_metadata_selects_current_semantic_policy(mut scope: Scope, entry: Entry) {
	scope.managed = Some(("original-agent".into(), "2.0.0".into()));
	assert!(permits(&mut scope, &entry, "semantic.read").await.unwrap());
	assert_eq!(scope.calls, vec!["thread", "semantic_policy"]);
	let (resource, action) = &scope.resources[0];
	assert_eq!(resource.id, entry.id.to_string());
	assert_eq!(action, "semantic.read");
	assert_eq!(
		resource.attributes,
		json!({"owner":"saved-owner","thread_id":entry.thread_id,"created_by":"saved-author","agent":"stored-agent","metadata":{"stored":true}})
	);
}
#[rstest]
#[case::semantic("semantic.read",vec!["thread","semantic_policy"])]
#[tokio::test]
async fn denied_policy_stops_before_reading_any_source(
	mut scope: Scope,
	mut entry: Entry,
	#[case] denied: &str,
	#[case] calls: Vec<&'static str>,
) {
	scope.managed = Some(("managed".into(), "1".into()));
	scope.denied.insert(denied.into());
	entry.source = json!({"kind":"artifact","id":6});
	assert!(!permits(&mut scope, &entry, "semantic.read").await.unwrap());
	assert_eq!(scope.calls, calls);
	assert!(scope.lookups.is_empty());
}
#[rstest]
#[tokio::test]
async fn deleted_history_retains_policy_and_skips_revoked_source_decode(
	mut scope: Scope,
	mut entry: Entry,
) {
	entry.deleted = true;
	entry.source = json!({"removed":true});
	assert!(permits(&mut scope, &entry, "semantic.read").await.unwrap());
	assert_eq!(scope.calls, vec!["thread", "semantic_policy"]);
}
#[rstest]
#[case::allowed(true)]
#[case::denied(false)]
#[tokio::test]
async fn deployment_administrator_disclosure_requires_visible_thread_before_any_source(
	mut scope: Scope,
	mut entry: Entry,
	#[case] allowed: bool,
) {
	scope.scoped = false;
	scope.deployment_administrator = allowed;
	entry.source = json!({"malformed":true});
	assert_eq!(
		permits(&mut scope, &entry, "semantic.read").await.unwrap(),
		allowed
	);
	assert_eq!(scope.calls, vec!["deployment_administrator"]);
	assert!(scope.resources.is_empty());
	scope.calls.clear();
	let result = source(
		&mut scope,
		entry.thread_id,
		&Source::Memory {
			text: "saved".into(),
		},
	)
	.await
	.unwrap();
	assert_eq!(result, allowed.then(|| "saved".to_owned()));
	assert_eq!(scope.calls, vec!["deployment_administrator"]);
}
#[rstest]
#[tokio::test]
async fn scoped_source_bytes_require_the_stored_resource_to_be_visible(mut scope: Scope) {
	let row_source = Source::Message {
		id: Uuid::from_u128(8),
	};
	let result = source(&mut scope, Uuid::from_u128(2), &row_source)
		.await
		.unwrap();
	assert_eq!(result, Some("original message bytes\n".into()));
	assert_eq!(
		scope.lookups,
		vec![(Uuid::from_u128(8), Uuid::from_u128(2))]
	);
	assert_eq!(scope.calls, vec!["message", "message_visible"]);
}
#[rstest]
#[case::missing_message(true)]
#[case::denied_message(false)]
#[tokio::test]
async fn absent_or_denied_stored_source_cannot_disclose_content(
	mut scope: Scope,
	#[case] missing: bool,
) {
	if missing {
		scope.message = None;
	} else {
		scope.visible = false;
	}
	let row_source = Source::Message {
		id: Uuid::from_u128(8),
	};
	assert_eq!(
		source(&mut scope, Uuid::from_u128(2), &row_source)
			.await
			.unwrap(),
		None
	);
	assert_eq!(scope.calls.len(), if missing { 1 } else { 2 });
}
#[rstest]
#[tokio::test]
async fn deployment_administrator_source_checks_thread_and_retains_query_scope(mut scope: Scope) {
	scope.scoped = false;
	scope.visible = false;
	let row_source = Source::Message {
		id: Uuid::from_u128(8),
	};
	assert!(
		source(&mut scope, Uuid::from_u128(2), &row_source)
			.await
			.unwrap()
			.is_some()
	);
	assert_eq!(scope.calls, vec!["deployment_administrator", "message"]);
}
#[rstest]
#[tokio::test]
async fn active_semantic_read_rechecks_its_saved_underlying_source(
	mut scope: Scope,
	mut entry: Entry,
) {
	scope.visible = false;
	entry.source = serde_json::to_value(Source::Message {
		id: Uuid::from_u128(8),
	})
	.unwrap();
	assert!(!permits(&mut scope, &entry, "semantic.read").await.unwrap());
	assert_eq!(
		scope.calls,
		vec!["thread", "semantic_policy", "message", "message_visible"]
	);
}
#[rstest]
#[tokio::test]
async fn malformed_active_source_returns_its_decode_error_after_policy(
	mut scope: Scope,
	mut entry: Entry,
) {
	entry.source = json!({"kind":"unknown"});
	assert!(matches!(
		permits(&mut scope, &entry, "semantic.read").await,
		Err(Error::Json(_))
	));
	assert_eq!(scope.calls, vec!["thread", "semantic_policy"]);
}
#[rstest]
#[case::thread("thread")]
#[case::semantic("semantic_policy")]
#[tokio::test]
async fn source_policy_adapter_failures_remain_opaque(
	mut scope: Scope,
	mut entry: Entry,
	#[case] fail: &'static str,
) {
	scope.fail = Some(fail);
	scope.managed = Some(("managed".into(), "1".into()));
	entry.source = serde_json::to_value(Source::Message {
		id: Uuid::from_u128(7),
	})
	.unwrap();
	let Error::Port(error) = permits(&mut scope, &entry, "semantic.read")
		.await
		.unwrap_err()
	else {
		panic!("expected opaque adapter error");
	};
	assert!(error.is::<std::io::Error>());
	assert_eq!(error.to_string(), fail);
	assert_eq!(scope.calls.last(), Some(&fail));
}
#[rstest]
#[case::deployment_administrator("deployment_administrator", false)]
#[tokio::test]
async fn source_adapter_failures_stop_at_the_original_boundary(
	mut scope: Scope,
	#[case] fail: &'static str,
	#[case] scoped: bool,
) {
	scope.fail = Some(fail);
	scope.scoped = scoped;
	let Error::Port(error) = source(
		&mut scope,
		Uuid::from_u128(2),
		&Source::Message {
			id: Uuid::from_u128(8),
		},
	)
	.await
	.unwrap_err() else {
		panic!("expected opaque adapter error");
	};
	assert!(error.is::<std::io::Error>());
	assert_eq!(error.to_string(), fail);
	assert_eq!(scope.calls.last(), Some(&fail));
}
