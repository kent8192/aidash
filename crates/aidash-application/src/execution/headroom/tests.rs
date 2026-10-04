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
	async fn definition(&self, _: &RunMetadata, id: &str, version: &str) -> Result<Entry> {
		self.trace.lock().unwrap().push(format!("definition:{id}"));
		let config = if id == "agent" {
			json!({"model":{"id":"model","version":"1"},"core_capabilities":{"skills":self.skills},"knowledge_digest":self.knowledge.then_some("digest")})
		} else {
			json!({"provider":"openrouter","model_id":"test","endpoint":"https://example.test","context_window":200_000,"max_output_tokens":1024,"modalities":["text"],"cost":{}})
		};
		Ok(serde_json::from_value(
			json!({"id":id,"version":version,"kind":id,"name":{},"description":{},"config":config}),
		)?)
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
		[
			"definition:agent",
			"definition:model",
			"documents",
			"contracts",
			"pinned"
		]
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
		[
			"definition:agent",
			"definition:model",
			"documents",
			"contracts"
		]
	);
}

#[rstest]
#[tokio::test]
async fn failed_private_read_precedes_capability_profile_resolution(
	mut scope: Scope,
	run: RunMetadata,
) {
	scope.document_error = true;
	assert_eq!(
		request(&scope, &run).await.unwrap_err(),
		Error::External("private contents unavailable".into())
	);
	assert_eq!(
		*scope.trace.lock().unwrap(),
		["definition:agent", "definition:model", "documents"]
	);
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
