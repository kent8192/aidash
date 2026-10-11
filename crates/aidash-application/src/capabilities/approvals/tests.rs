//! Web disclosure approvals share the capability approval records, decisions
//! and grants, with their own resource kind and action.
use super::*;
use crate::ports::{
	capabilities::approvals::{ApprovalScope, Creation, Limits},
	research::DisclosureState,
};
use aidash_domain::{
	RunPhase,
	capabilities::{approvals::ApprovalChoice, operations::MountedFile},
	policy::{Evaluation, PolicyBundle},
	research::Operation,
};
use async_trait::async_trait;
use std::collections::BTreeMap;

struct Fake {
	principal: String,
	subjects: Vec<String>,
	bundle: PolicyBundle,
	records: BTreeMap<Uuid, Record>,
	cache: BTreeMap<Uuid, (String, Value)>,
	run: Run,
}

#[async_trait]
impl ApprovalScope for Fake {
	fn principal(&self) -> &str {
		&self.principal
	}
	fn credential(&self) -> Uuid {
		Uuid::nil()
	}
	fn subjects(&self) -> &[String] {
		&self.subjects
	}
	fn replace_subjects(&mut self, subjects: Vec<String>) -> Vec<String> {
		std::mem::replace(&mut self.subjects, subjects)
	}
	fn policy_revision(&self) -> i64 {
		1
	}
	fn bundle(&self) -> &PolicyBundle {
		&self.bundle
	}
	fn evaluation(&self, subject: &str, target: &Resource, action: &str) -> Evaluation {
		Evaluation {
			subject: subject.into(),
			action: action.into(),
			resource: target.clone(),
			environment: json!({}),
		}
	}
	fn resource(&self, kind: &str, id: &str, attributes: Value) -> Resource {
		Resource {
			tenant: "tenant".into(),
			kind: kind.into(),
			id: id.into(),
			attributes,
		}
	}
	fn limits(&self) -> Result<Limits> {
		Ok(Limits {
			admission: false,
			origins: vec![],
			approval_seconds: 600,
			grant_seconds: 3600,
		})
	}
	async fn run_context(&mut self, _: Uuid) -> Result<Run> {
		Ok(self.run.clone())
	}
	async fn interaction_run(&mut self, _: Uuid) -> Result<Run> {
		Ok(self.run.clone())
	}
	async fn context_authority(&mut self) -> Result<()> {
		Ok(())
	}
	async fn active_run(&mut self) -> Result<Option<Uuid>> {
		Ok(Some(self.run.id))
	}
	async fn require(&mut self, target: &Resource, action: &str) -> Result<()> {
		let evaluation = self.evaluation(&self.principal.clone(), target, action);
		if self.bundle.evaluate(&evaluation).allowed {
			Ok(())
		} else {
			Err(Error::Forbidden)
		}
	}
	async fn load(&mut self, id: Uuid, kind: &str) -> Result<Record> {
		self.records
			.get(&id)
			.filter(|record| record.kind == kind)
			.cloned()
			.ok_or_else(|| Error::NotFound("record unavailable".into()))
	}
	async fn grants(&mut self, run: Uuid) -> Result<Vec<Record>> {
		Ok(self
			.records
			.values()
			.filter(|record| {
				record.kind == "grant"
					&& record.state == "active"
					&& record.data["run_id"] == json!(run)
					&& record.expires_at.is_some_and(|t| t > Utc::now())
			})
			.cloned()
			.collect())
	}
	async fn insert(&mut self, creation: Creation) -> Result<Record> {
		let record = Record {
			id: creation.id,
			tenant: "tenant".into(),
			owner: self.principal.clone(),
			area_id: creation.area,
			kind: creation.kind.into(),
			state: creation.state.into(),
			revision: 1,
			data: creation.data,
			expires_at: creation.expires,
		};
		self.records.insert(record.id, record.clone());
		Ok(record)
	}
	async fn transfer_owner(&mut self, id: Uuid, owner: &str) -> Result<()> {
		self.records.get_mut(&id).unwrap().owner = owner.into();
		Ok(())
	}
	async fn update(&mut self, record: &mut Record) -> Result<()> {
		record.revision += 1;
		self.records.insert(record.id, record.clone());
		Ok(())
	}
	async fn cached(&mut self, key: Uuid, digest: &str) -> Result<Option<Value>> {
		Ok(self
			.cache
			.get(&key)
			.filter(|(cached, _)| cached == digest)
			.map(|(_, value)| value.clone()))
	}
	async fn cache(&mut self, key: Uuid, digest: &str, value: &Value) -> Result<()> {
		self.cache.insert(key, (digest.into(), value.clone()));
		Ok(())
	}
	async fn event(&mut self, _: Uuid, _: &str, _: Value) -> Result<()> {
		Ok(())
	}
	async fn read(&mut self, _: &MountedFile) -> Result<Vec<u8>> {
		unreachable!("disclosures have no output files")
	}
}

/// Alice runs research; Bob is the designated capability approver.
fn scope(extra: Value) -> Fake {
	let mut policies = vec![
		json!({"id":"request","effect":"allow","subjects":{"any":true},"actions":["capability.request"],"resources":{"kinds":["research"]}}),
		json!({"id":"approve","effect":"allow","subjects":{"ids":["bob"]},"actions":["capability.approve"],"resources":{"kinds":["research"]}}),
	];
	if !extra.is_null() {
		policies.push(extra);
	}
	let bundle = serde_json::from_value(json!({
		"tenant":"tenant",
		"subjects":{
			"alice":{"kind":"user"},
			"bob":{"kind":"user","attributes":{"capability_approver":true}}
		},
		"policies":policies
	}))
	.unwrap();
	let now = Utc::now();
	Fake {
		principal: "alice".into(),
		subjects: vec!["alice".into()],
		bundle,
		records: BTreeMap::new(),
		cache: BTreeMap::new(),
		run: Run {
			id: Uuid::from_u128(1),
			task_id: Uuid::from_u128(2),
			workspace_id: Uuid::from_u128(3),
			home_node: "aidash://node".into(),
			agent_id: "agent".into(),
			agent_version: "1.0.0".into(),
			phase: RunPhase::ToolCall,
			control: RunControl::Active,
			step: 1,
			revision: 1,
			observed_input_seq: 0,
			ledger_worker_ready: true,
			error: None,
			lease_owner: None,
			lease_until: None,
			updated_at: now,
		},
	}
}

fn request() -> DisclosureRequest {
	DisclosureRequest {
		digest: "sha256:exact".into(),
		operation: Operation::Search,
		provider: "research.brave@1".into(),
		destination: "api.search.brave.com".into(),
		content: "rust release".into(),
	}
}

async fn decide_as_bob(scope: &mut Fake, id: Uuid, choice: ApprovalChoice) -> Result<Value> {
	scope.principal = "bob".into();
	let revision = scope.records[&id].revision;
	let (targets, expires_at) = match choice {
		ApprovalChoice::AllowRun => (
			Some(vec!["api.search.brave.com".to_owned()]),
			Some(Utc::now() + Duration::minutes(30)),
		),
		_ => (None, None),
	};
	let result = decide(
		scope,
		id,
		ApprovalDecision {
			idempotency_key: Uuid::new_v4(),
			expected_revision: revision,
			choice,
			targets,
			expires_at,
		},
	)
	.await;
	scope.principal = "alice".into();
	result
}

#[tokio::test]
async fn an_allow_once_approval_admits_exactly_one_disclosure() {
	let mut scope = scope(Value::Null);
	let run = scope.run.clone();
	let DisclosureState::Pending { approval_id, .. } =
		disclose(&mut scope, &run, &request()).await.unwrap()
	else {
		panic!("a designated approver must decide first");
	};
	let record = &scope.records[&approval_id];
	assert_eq!(record.kind, DISCLOSURE);
	assert_eq!(record.data["action"], DISCLOSE);
	assert_eq!(record.data["content"], "rust release");
	assert_eq!(record.data["approver"], "bob");
	// Repeating the call while pending waits on the same approval.
	assert!(matches!(
		disclose(&mut scope, &run, &request()).await.unwrap(),
		DisclosureState::Pending { approval_id: same, .. } if same == approval_id
	));

	let decided = decide_as_bob(&mut scope, approval_id, ApprovalChoice::AllowOnce)
		.await
		.unwrap();
	assert_eq!(decided["state"], "approved");
	assert_eq!(
		disclose(&mut scope, &run, &request()).await.unwrap(),
		DisclosureState::Permitted
	);
	assert_eq!(scope.records[&approval_id].state, "consumed");
	// The approval was consumed: the same disclosure needs a new approval.
	assert!(matches!(
		disclose(&mut scope, &run, &request()).await.unwrap(),
		DisclosureState::Pending { approval_id: next, .. } if next != approval_id
	));
}

#[tokio::test]
async fn a_denied_disclosure_stays_denied() {
	let mut scope = scope(Value::Null);
	let run = scope.run.clone();
	let DisclosureState::Pending { approval_id, .. } =
		disclose(&mut scope, &run, &request()).await.unwrap()
	else {
		panic!("pending");
	};
	decide_as_bob(&mut scope, approval_id, ApprovalChoice::Deny)
		.await
		.unwrap();
	assert_eq!(
		disclose(&mut scope, &run, &request()).await.unwrap(),
		DisclosureState::Denied
	);
}

#[tokio::test]
async fn policy_can_permit_or_forbid_a_disclosure_without_approval() {
	let mut permitted = scope(
		json!({"id":"disclose","effect":"allow","subjects":{"ids":["alice"]},"actions":["research.disclose"],"resources":{"kinds":["research"]}}),
	);
	let run = permitted.run.clone();
	assert_eq!(
		disclose(&mut permitted, &run, &request()).await.unwrap(),
		DisclosureState::Permitted
	);
	assert!(
		permitted
			.records
			.values()
			.all(|record| record.state == "consumed" && record.data["ordinary"] == true)
	);
	let mut forbidden = scope(
		json!({"id":"never","effect":"deny","subjects":{"any":true},"actions":["research.disclose"],"resources":{"kinds":["research"]}}),
	);
	assert_eq!(
		disclose(&mut forbidden, &run, &request()).await.unwrap(),
		DisclosureState::Denied
	);
	assert!(forbidden.records.is_empty());
}

#[tokio::test]
async fn without_request_permission_or_an_approver_nothing_is_disclosed() {
	let mut unrequested: Fake = scope(Value::Null);
	unrequested
		.bundle
		.policies
		.retain(|policy| policy.id != "request");
	let run = unrequested.run.clone();
	assert_eq!(
		disclose(&mut unrequested, &run, &request()).await.unwrap(),
		DisclosureState::Denied
	);
	let mut unapproved = scope(Value::Null);
	unapproved
		.bundle
		.policies
		.retain(|policy| policy.id != "approve");
	assert_eq!(
		disclose(&mut unapproved, &run, &request()).await.unwrap(),
		DisclosureState::Denied
	);
	assert!(
		unapproved
			.records
			.values()
			.all(|record| record.state == "blocked")
	);
}

#[tokio::test]
async fn an_allow_run_grant_covers_later_disclosures_to_the_same_destination() {
	let mut scope = scope(Value::Null);
	let run = scope.run.clone();
	let DisclosureState::Pending { approval_id, .. } =
		disclose(&mut scope, &run, &request()).await.unwrap()
	else {
		panic!("pending");
	};
	let decided = decide_as_bob(&mut scope, approval_id, ApprovalChoice::AllowRun)
		.await
		.unwrap();
	let grant: Uuid = serde_json::from_value(decided["grant_id"].clone()).unwrap();
	assert_eq!(scope.records[&grant].data["actions"], json!([DISCLOSE]));
	assert_eq!(scope.records[&grant].owner, "alice");
	let mut other = request();
	other.digest = "sha256:another-query".into();
	other.content = "another query".into();
	assert_eq!(
		disclose(&mut scope, &run, &other).await.unwrap(),
		DisclosureState::Permitted
	);
	let mut elsewhere = other.clone();
	elsewhere.digest = "sha256:elsewhere".into();
	elsewhere.destination = "elsewhere.example.com".into();
	assert!(matches!(
		disclose(&mut scope, &run, &elsewhere).await.unwrap(),
		DisclosureState::Pending { .. }
	));
}
