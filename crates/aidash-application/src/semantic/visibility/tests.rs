use super::*;
use crate::Error;
use aidash_domain::{Artifact, Message, policy::Resource};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::Value;
use std::collections::BTreeSet;

#[fixture]
fn entry() -> Entry {
	Entry {
		id: Uuid::from_u128(1),
		workspace_id: Uuid::from_u128(2),
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
fn artifact() -> Artifact {
	Artifact {
		id: Uuid::from_u128(6),
		workspace_id: Uuid::from_u128(2),
		task_id: Uuid::from_u128(7),
		kind: "data".into(),
		name: "Saved".into(),
		content: json!({"saved":[1,true]}),
		created_by: "artifact-author".into(),
		idempotency_key: "artifact-key".into(),
		created_at: chrono::Utc::now(),
	}
}
#[fixture]
fn message() -> Message {
	Message {
		id: Uuid::from_u128(8),
		workspace_id: Uuid::from_u128(2),
		sender: "stored-sender".into(),
		content: "original message bytes\n".into(),
		idempotency_key: None,
		created_at: chrono::Utc::now(),
	}
}
struct Scope {
	scoped: bool,
	operator: bool,
	managed: Option<(String, String)>,
	artifact: Option<Artifact>,
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
		operator: true,
		managed: None,
		artifact: Some(artifact()),
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
	async fn operator_visible(&mut self, workspace: Uuid) -> Result<bool> {
		assert_eq!(workspace, Uuid::from_u128(2));
		self.touch("operator")?;
		Ok(self.operator)
	}
	async fn workspace_resource(&mut self, workspace: Uuid) -> Result<Resource> {
		assert_eq!(workspace, Uuid::from_u128(2));
		self.touch("workspace")?;
		Ok(Resource {
			tenant: "tenant".into(),
			kind: "workspace".into(),
			id: workspace.to_string(),
			attributes: json!({"owner":"saved-owner","workspace_id":workspace}),
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
	async fn managed_memory(&mut self, id: Uuid) -> Result<Option<(String, String)>> {
		assert_eq!(id, Uuid::from_u128(1));
		self.touch("managed")?;
		Ok(self.managed.clone())
	}
	async fn artifact(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Artifact>> {
		self.touch("artifact")?;
		self.lookups.push((id, workspace));
		Ok(self.artifact.clone())
	}
	async fn message(&mut self, id: Uuid, workspace: Uuid) -> Result<Option<Message>> {
		self.touch("message")?;
		self.lookups.push((id, workspace));
		Ok(self.message.clone())
	}
	async fn artifact_visible(&mut self, row: &Artifact) -> Result<bool> {
		assert_eq!(row.id, Uuid::from_u128(6));
		self.touch("artifact_visible")?;
		Ok(self.visible)
	}
	async fn message_visible(&mut self, row: &Message) -> Result<bool> {
		assert_eq!(row.id, Uuid::from_u128(8));
		self.touch("message_visible")?;
		Ok(self.visible)
	}
}
#[rstest]
#[tokio::test]
async fn saved_authorship_metadata_and_managed_agent_select_current_policy(
	mut scope: Scope,
	entry: Entry,
) {
	scope.managed = Some(("original-agent".into(), "2.0.0".into()));
	assert!(permits(&mut scope, &entry, "semantic.read").await.unwrap());
	assert_eq!(
		scope.calls,
		vec!["workspace", "semantic_policy", "managed", "memory_policy"]
	);
	let (resource, action) = &scope.resources[0];
	assert_eq!(resource.id, entry.id.to_string());
	assert_eq!(action, "semantic.read");
	assert_eq!(
		resource.attributes,
		json!({"owner":"saved-owner","workspace_id":entry.workspace_id,"created_by":"saved-author","agent":"stored-agent","metadata":{"stored":true}})
	);
	let (resource, action) = &scope.resources[1];
	assert_eq!(resource.kind, "memory");
	assert_eq!(resource.id, "original-agent");
	assert_eq!(action, "memory.read");
	assert_eq!(resource.attributes["created_by"], "stored-agent");
	assert_eq!(resource.attributes["version"], "2.0.0");
}
#[rstest]
#[case::write("semantic.write")]
#[case::delete("semantic.delete")]
#[tokio::test]
async fn managed_mutations_use_memory_write_and_do_not_decode_source(
	mut scope: Scope,
	mut entry: Entry,
	#[case] action: &str,
) {
	scope.managed = Some(("managed".into(), "1".into()));
	entry.source = json!({"malformed":true});
	assert!(permits(&mut scope, &entry, action).await.unwrap());
	assert_eq!(scope.resources[1].1, "memory.write");
	assert_eq!(
		scope.calls,
		vec!["workspace", "semantic_policy", "managed", "memory_policy"]
	);
}
#[rstest]
#[case::semantic("semantic.read",vec!["workspace","semantic_policy"])]
#[case::memory("memory.read",vec!["workspace","semantic_policy","managed","memory_policy"])]
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
	assert_eq!(scope.calls, vec!["workspace", "semantic_policy", "managed"]);
}
#[rstest]
#[case::allowed(true)]
#[case::denied(false)]
#[tokio::test]
async fn operator_disclosure_requires_visible_workspace_before_any_source(
	mut scope: Scope,
	mut entry: Entry,
	#[case] allowed: bool,
) {
	scope.scoped = false;
	scope.operator = allowed;
	entry.source = json!({"malformed":true});
	assert_eq!(
		permits(&mut scope, &entry, "semantic.read").await.unwrap(),
		allowed
	);
	assert_eq!(scope.calls, vec!["operator"]);
	assert!(scope.resources.is_empty());
	scope.calls.clear();
	let result = source(
		&mut scope,
		entry.workspace_id,
		&Source::Memory {
			text: "saved".into(),
		},
	)
	.await
	.unwrap();
	assert_eq!(result, allowed.then(|| "saved".to_owned()));
	assert_eq!(scope.calls, vec!["operator"]);
}
#[rstest]
#[case::artifact(true)]
#[case::message(false)]
#[tokio::test]
async fn scoped_source_bytes_require_the_stored_resource_to_be_visible(
	mut scope: Scope,
	#[case] artifact_kind: bool,
) {
	let row_source = if artifact_kind {
		Source::Artifact {
			id: Uuid::from_u128(6),
		}
	} else {
		Source::Message {
			id: Uuid::from_u128(8),
		}
	};
	let result = source(&mut scope, Uuid::from_u128(2), &row_source)
		.await
		.unwrap();
	assert_eq!(
		result,
		Some(if artifact_kind {
			"{\"saved\":[1,true]}".into()
		} else {
			"original message bytes\n".into()
		})
	);
	assert_eq!(
		scope.lookups,
		vec![(
			if artifact_kind {
				Uuid::from_u128(6)
			} else {
				Uuid::from_u128(8)
			},
			Uuid::from_u128(2)
		)]
	);
	assert_eq!(
		scope.calls,
		if artifact_kind {
			vec!["artifact", "artifact_visible"]
		} else {
			vec!["message", "message_visible"]
		}
	);
}
#[rstest]
#[case::missing_artifact(true, true)]
#[case::missing_message(false, true)]
#[case::denied_artifact(true, false)]
#[case::denied_message(false, false)]
#[tokio::test]
async fn absent_or_denied_stored_source_cannot_disclose_content(
	mut scope: Scope,
	#[case] artifact_kind: bool,
	#[case] missing: bool,
) {
	if missing {
		scope.artifact = None;
		scope.message = None;
	} else {
		scope.visible = false;
	}
	let row_source = if artifact_kind {
		Source::Artifact {
			id: Uuid::from_u128(6),
		}
	} else {
		Source::Message {
			id: Uuid::from_u128(8),
		}
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
#[case::artifact(true)]
#[case::message(false)]
#[tokio::test]
async fn operator_source_checks_workspace_and_retains_query_scope(
	mut scope: Scope,
	#[case] artifact_kind: bool,
) {
	scope.scoped = false;
	scope.visible = false;
	let row_source = if artifact_kind {
		Source::Artifact {
			id: Uuid::from_u128(6),
		}
	} else {
		Source::Message {
			id: Uuid::from_u128(8),
		}
	};
	assert!(
		source(&mut scope, Uuid::from_u128(2), &row_source)
			.await
			.unwrap()
			.is_some()
	);
	assert_eq!(
		scope.calls,
		if artifact_kind {
			vec!["operator", "artifact"]
		} else {
			vec!["operator", "message"]
		}
	);
}
#[rstest]
#[case::artifact(true)]
#[case::message(false)]
#[tokio::test]
async fn active_semantic_read_rechecks_its_saved_underlying_source(
	mut scope: Scope,
	mut entry: Entry,
	#[case] artifact_kind: bool,
) {
	scope.visible = false;
	entry.source = serde_json::to_value(if artifact_kind {
		Source::Artifact {
			id: Uuid::from_u128(6),
		}
	} else {
		Source::Message {
			id: Uuid::from_u128(8),
		}
	})
	.unwrap();
	assert!(!permits(&mut scope, &entry, "semantic.read").await.unwrap());
	assert_eq!(
		scope.calls,
		if artifact_kind {
			vec![
				"workspace",
				"semantic_policy",
				"managed",
				"artifact",
				"artifact_visible",
			]
		} else {
			vec![
				"workspace",
				"semantic_policy",
				"managed",
				"message",
				"message_visible",
			]
		}
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
	assert_eq!(scope.calls, vec!["workspace", "semantic_policy", "managed"]);
}
#[rstest]
#[case::workspace("workspace")]
#[case::semantic("semantic_policy")]
#[case::managed("managed")]
#[case::memory("memory_policy")]
#[case::artifact("artifact")]
#[case::artifact_visibility("artifact_visible")]
#[tokio::test]
async fn source_policy_adapter_failures_remain_opaque(
	mut scope: Scope,
	mut entry: Entry,
	#[case] fail: &'static str,
) {
	scope.fail = Some(fail);
	scope.managed = Some(("managed".into(), "1".into()));
	entry.source = serde_json::to_value(Source::Artifact {
		id: Uuid::from_u128(6),
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
#[case::operator("operator", false)]
#[case::message("message", true)]
#[case::message_visibility("message_visible", true)]
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
