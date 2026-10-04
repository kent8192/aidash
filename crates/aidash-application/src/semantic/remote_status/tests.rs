use super::*;
use crate::ports::generation::visibility::GenerationVisibility;
use aidash_domain::{
	Task,
	generation::{remote::Ancestor, requests::Request},
	policy::Resource,
	registry::EntityRef,
	semantic::{
		remote::{Provider, journal::Record, status::Allowance},
		results::SearchResult,
	},
};
use async_trait::async_trait;
use chrono::DateTime;
use rstest::{fixture, rstest};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

fn owner(node: &str, tenant: &str, id: u128) -> Ancestor {
	Ancestor {
		node_id: node.into(),
		tenant: tenant.into(),
		request_id: Uuid::from_u128(id),
		policy_id: "policy".into(),
		policy_revision: 1,
		depth: 0,
		expires_at: DateTime::from_timestamp(2000, 0).unwrap(),
	}
}
#[fixture]
fn receipt() -> Value {
	json!(Receipt {
		operation_id: Uuid::from_u128(20),
		operation_digest: "operation-digest".into(),
		home_node: "home".into(),
		tenant: "tenant".into(),
		workspace_id: Uuid::from_u128(21),
		grant_id: Uuid::from_u128(22),
		admission_id: Uuid::from_u128(23),
		executor: "executor".into(),
		binding: Binding::RequiredHome {
			home_lineage: vec![
				owner("remote", "tenant", 1),
				owner("local", "other-tenant", 2),
				owner("local", "tenant", 3),
				owner("local", "tenant", 4),
				owner("local", "tenant", 5)
			],
			execution_lineage: vec![owner("local", "tenant", 6)],
			version: 1,
			index_revision: 1,
			index_digest: "index-digest".into(),
			embedding: Box::new(Provider {
				node_id: "home".into(),
				entry: EntityRef {
					id: "embedding".into(),
					version: "1".into()
				},
				digest: "digest".into(),
				configuration_digest: "configuration".into()
			}),
			compactor: None
		},
		retrieved_at: DateTime::from_timestamp(1000, 0).unwrap(),
		query_truncated: false,
		candidate_digest: "candidates".into(),
		sources: vec![],
		result: SearchResult {
			workspace_id: Uuid::from_u128(21),
			index_revision: 1,
			model: "embedding".into(),
			model_version: "1".into(),
			matches: vec![],
			estimated_tokens: 0,
			truncated: false
		},
		estimated_tokens: 0
	})
}
struct Scope {
	requests: Vec<Uuid>,
	allowances: Vec<Uuid>,
	receipt_reads: usize,
	visible: bool,
	value: Option<Value>,
}
#[fixture]
fn scope(receipt: Value) -> Scope {
	Scope {
		requests: vec![],
		allowances: vec![],
		receipt_reads: 0,
		visible: true,
		value: Some(receipt),
	}
}
#[async_trait]
impl GenerationVisibility for Scope {
	fn inherited_lease(&self) -> bool {
		false
	}
	fn context(&mut self, _: Value) {}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		Resource {
			tenant: "tenant".into(),
			kind: kind.into(),
			id: id.into(),
			attributes,
		}
	}
	async fn workspace(&mut self, _: Uuid) -> Result<Resource> {
		panic!("foreign request visibility uses qualified task authority")
	}
	async fn task(&mut self, _: Uuid, _: Uuid) -> Result<Option<Task>> {
		panic!("foreign request visibility uses qualified task authority")
	}
	async fn task_visible(&mut self, _: &Task) -> Result<bool> {
		panic!("foreign request visibility uses qualified task authority")
	}
	async fn decide(&mut self, resource: &Resource, action: &str) -> Result<bool> {
		Ok(!(action == "generation.read" && resource.id == Uuid::from_u128(4).to_string()))
	}
}
#[async_trait]
impl StatusScope for Scope {
	fn node_id(&self) -> &str {
		"local"
	}
	fn tenant(&self) -> &str {
		"tenant"
	}
	async fn request(&mut self, id: Uuid, tenant: &str) -> Result<Option<Request>> {
		assert_eq!(tenant, "tenant");
		self.requests.push(id);
		if id == Uuid::from_u128(5) {
			return Ok(None);
		}
		let timestamp = DateTime::from_timestamp(1000, 0).unwrap();
		Ok(Some(Request {
			id,
			tenant: "tenant".into(),
			policy_id: "policy".into(),
			policy_revision: 7,
			task_id: Uuid::from_u128(10),
			home_node: "home".into(),
			foreign_intent: None,
			prepared: true,
			grant_id: None,
			admission_id: None,
			workspace_id: Uuid::from_u128(11),
			credential_id: Uuid::from_u128(12),
			root_subject: "alice".into(),
			subject_chain: vec!["alice".into()],
			agent_id: "agent".into(),
			agent_version: "1.0.0".into(),
			definition: json!({}),
			status: "ACTIVE".into(),
			reason: "work".into(),
			depth: 1,
			token_limit: 10000,
			quota_released: false,
			expires_at: timestamp,
			created_at: timestamp,
		}))
	}
	async fn allowance(&mut self, id: Uuid) -> Result<Allowance> {
		self.allowances.push(id);
		Ok(Allowance {
			request_id: id,
			token_limit: 100,
			used_tokens: 7,
			embedding_call_limit: 4,
			embedding_calls: 2,
			compaction_call_limit: 3,
			compaction_calls: 1,
		})
	}
	async fn run_visible(&mut self, _: Uuid) -> Result<bool> {
		Ok(self.visible)
	}
	async fn receipt(&mut self, _: Uuid) -> Result<Option<Value>> {
		self.receipt_reads += 1;
		Ok(self.value.clone())
	}
}
#[rstest]
#[tokio::test]
async fn counters_require_local_origin_same_tenant_and_current_request_visibility(
	mut scope: Scope,
	receipt: Value,
) {
	let view = provenance(&mut scope, Some(receipt))
		.await
		.unwrap()
		.unwrap();
	assert_eq!(view.allowance_node, "local");
	assert_eq!(
		scope.requests,
		vec![3, 4, 5, 6]
			.into_iter()
			.map(Uuid::from_u128)
			.collect::<Vec<_>>()
	);
	assert_eq!(
		scope.allowances,
		vec![Uuid::from_u128(3), Uuid::from_u128(6)]
	);
	assert_eq!(
		view.allowances
			.iter()
			.map(|a| (
				a.request_id,
				a.used_tokens,
				a.embedding_calls,
				a.compaction_calls
			))
			.collect::<Vec<_>>(),
		vec![(Uuid::from_u128(3), 7, 2, 1), (Uuid::from_u128(6), 7, 2, 1)]
	);
}
#[rstest]
#[tokio::test]
async fn invisible_run_blocks_receipt_and_counter_reads(mut scope: Scope) {
	scope.visible = false;
	let result = run_receipt(&mut scope, Uuid::from_u128(25)).await;
	assert!(matches!(result, Err(Error::Forbidden)));
	assert_eq!(scope.receipt_reads, 0);
	assert!(scope.requests.is_empty());
	assert!(scope.allowances.is_empty());
}
#[rstest]
#[tokio::test]
async fn absent_receipt_does_not_read_generation_counters(mut scope: Scope) {
	scope.value = None;
	assert!(
		run_receipt(&mut scope, Uuid::from_u128(25))
			.await
			.unwrap()
			.is_none()
	);
	assert_eq!(scope.receipt_reads, 1);
	assert!(scope.requests.is_empty());
}
struct Repository {
	calls: AtomicUsize,
}
#[async_trait]
impl StatusRepository for Repository {
	async fn latest(&self, _: Uuid) -> Result<Option<Record>> {
		self.calls.fetch_add(1, Ordering::SeqCst);
		Err(Error::External("must not query disabled binding".into()))
	}
}
#[rstest]
#[tokio::test]
async fn disabled_summary_does_not_touch_the_journal() {
	let repository = Repository {
		calls: AtomicUsize::new(0),
	};
	let status = load(
		&repository,
		Uuid::from_u128(22),
		&Binding::Disabled {},
		Some(Failure::Authority),
	)
	.await
	.unwrap();
	assert_eq!(status.state, "disabled");
	assert_eq!(status.reason, Some(Failure::Authority));
	assert_eq!(repository.calls.load(Ordering::SeqCst), 0);
}
