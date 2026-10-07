use super::*;
use crate::{
	Error,
	ports::{Credentials, registry::CoreToolCatalog},
	registry::DefinitionValidation,
};
use aidash_domain::{RunControl, RunPhase, registry::Entry};
use async_trait::async_trait;
use rstest::{fixture, rstest};
use std::{
	collections::BTreeMap,
	sync::{Arc, Mutex},
};
use uuid::Uuid;

struct Contracts;
impl Credentials for Contracts {
	fn resolve(&self, _: &str) -> Result<String> {
		panic!("prompt budgeting cannot disclose credentials")
	}
}
impl CoreToolCatalog for Contracts {
	fn specifications(
		&self,
		_: &aidash_domain::capabilities::CoreCapabilities,
	) -> BTreeMap<String, aidash_domain::provider::ToolSpec> {
		BTreeMap::new()
	}
}
struct Scope {
	trace: Mutex<Vec<String>>,
	skills: bool,
	knowledge: bool,
	document_error: bool,
	pinned: usize,
}
#[fixture]
fn scope() -> Scope {
	Scope {
		trace: Mutex::new(vec![]),
		skills: true,
		knowledge: true,
		document_error: false,
		pinned: usize::MAX,
	}
}
#[fixture]
fn run() -> RunMetadata {
	RunMetadata {
		id: Uuid::new_v4(),
		task_id: Uuid::new_v4(),
		workspace_id: Uuid::new_v4(),
		home_node: "local".into(),
		agent_id: "agent".into(),
		agent_version: "1".into(),
		phase: RunPhase::Thinking,
		control: RunControl::Active,
		step: 0,
		revision: 1,
		observed_input_seq: 0,
		ledger_worker_ready: true,
		error: None,
		lease_owner: None,
		lease_until: None,
		updated_at: chrono::Utc::now(),
	}
}
#[async_trait]
impl Definitions for Scope {
	fn node(&self) -> &str {
		"local"
	}

	async fn snapshot(
		&self,
		_: &RunMetadata,
	) -> Result<aidash_domain::registry::bindings::BindingSnapshot> {
		self.trace.lock().unwrap().push("snapshot".into());
		let node = "aidash://local";
		let mut root = crate::test_support::agent("agent");
		if !self.skills {
			root.config["remove_default"] = json!(["skill_list", "skill_load", "skill_read"]);
		}
		let mut extras = vec![];
		if self.knowledge {
			let source = crate::test_support::entry(
				"references",
				"source",
				json!({"schema_version":1,"source":{"adapter":"private_references","digest":"a".repeat(64)}}),
			);
			root.config["bindings"] = json!([{"kind":"source","target":{"registry_node":node,"id":"references","version":"1.0.0"}}]);
			extras.push(source);
		}

		if self.skills {
			root.config["bindings"].as_array_mut().unwrap().push(json!({"kind":"source","target":{"registry_node":node,"id":"mounted-skills","version":"1.0.0"}}));
			extras.push(crate::test_support::entry(
				"mounted-skills",
				"source",
				json!({"schema_version":1,"source":{"adapter":"skill_roots","roots":[".agents/skills"]}}),
			));
		}
		Ok(crate::test_support::resolve(node, &root, false, extras))
	}
	async fn definition(&self, _: &RunMetadata, _: &str, _: &str) -> Result<Entry> {
		panic!("headroom reads only the admitted closure")
	}
	async fn documents(&self, _: &Entry) -> Result<Value> {
		self.trace.lock().unwrap().push("documents".into());
		if self.document_error {
			Err(Error::External("private contents unavailable".into()))
		} else {
			Ok(json!(["private instruction"]))
		}
	}
	fn validation(&self) -> DefinitionValidation {
		self.trace.lock().unwrap().push("contracts".into());
		DefinitionValidation::new(Arc::new(Contracts), Arc::new(Contracts))
	}
	async fn pinned_headroom(&self, _: Uuid) -> Result<usize> {
		self.trace.lock().unwrap().push("pinned".into());
		Ok(self.pinned)
	}
}

#[rstest]
#[tokio::test]
async fn pinned_context_saturates_after_current_definition_and_knowledge_reads(
	scope: Scope,
	run: RunMetadata,
) {
	assert_eq!(request(&scope, &run).await.unwrap(), 0);
	assert_eq!(
		*scope.trace.lock().unwrap(),
		["snapshot", "documents", "contracts", "pinned"]
	);
}

#[rstest]
#[tokio::test]
async fn disabled_skills_skip_pinned_contents_and_cap_corrections(
	mut scope: Scope,
	run: RunMetadata,
) {
	scope.skills = false;
	assert_eq!(message_limit(&scope, &run).await.unwrap(), 16_384);
	assert_eq!(
		*scope.trace.lock().unwrap(),
		["snapshot", "documents", "contracts"]
	);
}

#[rstest]
#[tokio::test]
async fn failed_private_read_precedes_capability_profile_resolution(
	mut scope: Scope,
	run: RunMetadata,
) {
	scope.document_error = true;
	match request(&scope, &run).await {
		Err(Error::External(message)) => assert_eq!(message, "private contents unavailable"),
		other => panic!("unexpected private read outcome: {other:?}"),
	}
	assert_eq!(*scope.trace.lock().unwrap(), ["snapshot", "documents"]);
}

#[rstest]
#[tokio::test]
async fn remote_media_routes_do_not_read_local_definitions(scope: Scope, mut run: RunMetadata) {
	run.home_node = "remote".into();
	assert_eq!(
		media_routes(&scope, &run).await.unwrap(),
		Vec::<Vec<String>>::new()
	);
	assert_eq!(*scope.trace.lock().unwrap(), Vec::<String>::new());
}
