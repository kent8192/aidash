use super::*;
use crate::{authorization::Snapshot, generation::test_support};
use aidash_domain::policy::{Group, Role, Subject};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rstest::{fixture, rstest};
use serde_json::json;
use std::{collections::BTreeSet, time::Duration};
struct Publication {
	snapshot: Snapshot,
	calls: Vec<&'static str>,
	fail: Option<&'static str>,
	pause: Option<&'static str>,
	effects: Vec<String>,
}
#[fixture]
fn publication() -> Publication {
	Publication {
		snapshot: Snapshot {
			revision: 7,
			bundle: serde_json::from_value(
				json!({"tenant":"tenant","subjects":{"alice":{"kind":"user"}}}),
			)
			.unwrap(),
		},
		calls: vec![],
		fail: None,
		pause: None,
		effects: vec![],
	}
}
impl Publication {
	async fn point(&mut self, name: &'static str) -> Result<()> {
		self.calls.push(name);
		if self.fail == Some(name) {
			return Err(Error::External(format!("{name} failed")));
		}
		if self.pause == Some(name) {
			std::future::pending::<()>().await;
		}
		Ok(())
	}
}
#[fixture]
fn specification() -> Spec {
	serde_json::from_value(test_support::specification()).unwrap()
}
#[fixture]
fn job(specification: Spec) -> Request {
	let mut job = test_support::request(1);
	let mut definition = specification.template;
	definition.id = "generated-fixture".into();
	job.agent_id = definition.id.clone();
	job.definition = json!(definition);
	job
}
#[async_trait]
impl GenerationPublication for Publication {
	fn node_id(&self) -> &str {
		"aidash://local"
	}
	fn snapshot(&self) -> &Snapshot {
		&self.snapshot
	}
	fn replace_snapshot(&mut self, snapshot: Snapshot) {
		self.calls.push("replace");
		self.snapshot = snapshot;
	}
	async fn save_authority(&mut self, job: &Request) -> Result<()> {
		assert_eq!(job.tenant, "tenant");
		self.point("authority").await?;
		self.effects
			.push(format!("authority:{}", self.snapshot.revision));
		Ok(())
	}
	async fn register(&mut self, entry: &Entry) -> Result<()> {
		self.point("register").await?;
		self.effects
			.push(format!("register:{}@{}", entry.id, entry.version));
		Ok(())
	}
	async fn approve(&mut self, job: &Request, entry: &Entry) -> Result<()> {
		assert_eq!(job.agent_id, entry.id);
		self.point("approve").await?;
		self.effects.push("approve".into());
		Ok(())
	}
	async fn catalog_history(&mut self, _job: &Request, _entry: &Entry) -> Result<()> {
		self.point("history").await?;
		self.effects.push("history".into());
		Ok(())
	}
}
#[rstest]
#[tokio::test]
async fn publication_assigns_only_pinned_permissions_and_exact_revision(
	mut publication: Publication,
	mut specification: Spec,
	job: Request,
) {
	publication
		.snapshot
		.bundle
		.roles
		.insert("research".into(), Role::default());
	publication
		.snapshot
		.bundle
		.groups
		.insert("team".into(), Group::default());
	specification.permissions.roles = BTreeSet::from(["research".into()]);
	specification.permissions.groups = BTreeSet::from(["team".into()]);
	specification.permissions.attributes = json!({"allow_tool":false});
	publish(&mut publication, &job, &specification)
		.await
		.unwrap();
	let subject = &publication.snapshot.bundle.subjects
		[&qualified_agent("aidash://local", &job.agent_id, &job.agent_version)];
	assert_eq!(subject.kind, SubjectKind::Agent);
	assert!(subject.enabled);
	assert_eq!(subject.roles, specification.permissions.roles);
	assert_eq!(subject.groups, specification.permissions.groups);
	assert_eq!(subject.attributes, json!({"allow_tool":false}));
	assert_eq!(subject.delegated_by.as_deref(), Some("alice"));
	assert_eq!(publication.snapshot.revision, 8);
	assert_eq!(
		publication.calls,
		["replace", "authority", "register", "approve", "history"]
	);
	assert_eq!(
		publication.effects,
		[
			"authority:8",
			"register:generated-fixture@1.0.0",
			"approve",
			"history"
		]
	);
}
#[rstest]
#[tokio::test]
async fn a_nested_subject_is_delegated_only_by_the_last_current_ancestor(
	mut publication: Publication,
	specification: Spec,
	mut job: Request,
) {
	publication.snapshot.bundle.subjects.insert(
		"ancestor".into(),
		Subject {
			kind: SubjectKind::Agent,
			enabled: true,
			roles: BTreeSet::new(),
			groups: BTreeSet::new(),
			attributes: json!({}),
			delegated_by: Some("alice".into()),
		},
	);
	job.subject_chain.push("ancestor".into());
	publish(&mut publication, &job, &specification)
		.await
		.unwrap();
	assert_eq!(
		publication.snapshot.bundle.subjects
			[&qualified_agent("aidash://local", &job.agent_id, &job.agent_version)]
			.delegated_by
			.as_deref(),
		Some("ancestor")
	);
}
#[rstest]
#[tokio::test]
async fn an_existing_generated_identity_cannot_be_overwritten(
	mut publication: Publication,
	specification: Spec,
	job: Request,
) {
	let key = qualified_agent("aidash://local", &job.agent_id, &job.agent_version);
	publication.snapshot.bundle.subjects.insert(
		key.clone(),
		publication.snapshot.bundle.subjects["alice"].clone(),
	);
	let before = json!(publication.snapshot);
	assert!(matches!(
		publish(&mut publication, &job, &specification).await,
		Err(Error::Conflict(_))
	));
	assert_eq!(json!(publication.snapshot), before);
	assert!(publication.calls.is_empty());
	assert!(publication.effects.is_empty());
}
#[rstest]
#[case("role")]
#[case("group")]
#[case("attributes")]
#[case("delegator")]
#[case("bundle_limit")]
#[tokio::test]
async fn invalid_generated_authority_never_reaches_storage(
	mut publication: Publication,
	mut specification: Spec,
	mut job: Request,
	#[case] defect: &str,
) {
	match defect {
		"role" => {
			specification.permissions.roles.insert("missing".into());
		}
		"group" => {
			specification.permissions.groups.insert("missing".into());
		}
		"attributes" => specification.permissions.attributes = json!([]),
		"delegator" => job.subject_chain.push("missing".into()),
		"bundle_limit" => {
			for n in 0..511 {
				publication.snapshot.bundle.subjects.insert(
					format!("member-{n}"),
					publication.snapshot.bundle.subjects["alice"].clone(),
				);
			}
		}
		_ => panic!("unknown defect"),
	};
	let before = json!(publication.snapshot);
	assert!(matches!(
		publish(&mut publication, &job, &specification).await,
		Err(Error::Domain(aidash_domain::Error::Invalid(_)))
	));
	assert_eq!(json!(publication.snapshot), before);
	assert!(publication.calls.is_empty());
}
#[rstest]
#[tokio::test]
async fn revision_exhaustion_does_not_mutate_or_publish_authority(
	mut publication: Publication,
	specification: Spec,
	job: Request,
) {
	publication.snapshot.revision = i64::MAX;
	let before = json!(publication.snapshot);
	assert!(
		matches!(publish(&mut publication,&job,&specification).await,Err(Error::Invalid(message)) if message=="authorization revision exhausted")
	);
	assert_eq!(json!(publication.snapshot), before);
	assert!(publication.calls.is_empty());
}
#[rstest]
#[case("authority",vec!["replace","authority"])]
#[case("register",vec!["replace","authority","register"])]
#[case("approve",vec!["replace","authority","register","approve"])]
#[case("history",vec!["replace","authority","register","approve","history"])]
#[tokio::test]
async fn a_publication_failure_stops_before_the_next_atomic_step(
	mut publication: Publication,
	specification: Spec,
	job: Request,
	#[case] operation: &'static str,
	#[case] expected: Vec<&'static str>,
) {
	publication.fail = Some(operation);
	assert!(
		publish(&mut publication, &job, &specification)
			.await
			.is_err()
	);
	assert_eq!(publication.calls, expected);
}
#[rstest]
#[tokio::test]
async fn malformed_definition_remains_a_json_error_before_authority_mutation(
	mut publication: Publication,
	specification: Spec,
	mut job: Request,
) {
	job.definition = json!({"id":false});
	assert!(matches!(
		publish(&mut publication, &job, &specification).await,
		Err(Error::Json(_))
	));
	assert!(publication.calls.is_empty());
}
#[rstest]
#[tokio::test]
async fn cancellation_cannot_advance_from_authority_to_registration(
	mut publication: Publication,
	specification: Spec,
	job: Request,
) {
	publication.pause = Some("authority");
	assert!(
		tokio::time::timeout(
			Duration::from_millis(20),
			publish(&mut publication, &job, &specification)
		)
		.await
		.is_err()
	);
	assert_eq!(publication.calls, ["replace", "authority"]);
	assert!(publication.effects.is_empty());
}
struct Live {
	jobs: Vec<Request>,
	enabled: bool,
	now: DateTime<Utc>,
	calls: Vec<String>,
	fail: bool,
}
#[fixture]
fn live() -> Live {
	Live {
		jobs: vec![],
		enabled: true,
		now: DateTime::from_timestamp(999, 0).unwrap(),
		calls: vec![],
		fail: false,
	}
}
#[async_trait]
impl GenerationLive for Live {
	fn tenant(&self) -> &str {
		"tenant"
	}
	fn now(&self) -> DateTime<Utc> {
		self.now
	}
	async fn jobs(&mut self, node: &str, agent: &EntityRef) -> Result<Vec<Request>> {
		assert_eq!(node, "aidash://local");
		assert_eq!(agent.id, "agent");
		self.calls.push("jobs".into());
		Ok(self.jobs.clone())
	}
	async fn policy_enabled(&mut self, job: &Request) -> Result<bool> {
		self.calls.push(format!("policy:{}", job.id));
		if self.fail {
			return Err(Error::External("policy query failed".into()));
		}
		Ok(self.enabled)
	}
}
async fn check(live: &mut Live) -> Result<()> {
	require_live(
		live,
		"aidash://local",
		Uuid::from_u128(2),
		&EntityRef {
			id: "agent".into(),
			version: "1.0.0".into(),
		},
	)
	.await
}
#[rstest]
#[tokio::test]
async fn ordinary_agents_need_no_generation_ancestry(mut live: Live) {
	check(&mut live).await.unwrap();
	assert_eq!(live.calls, ["jobs"]);
}
#[rstest]
#[case("tenant")]
#[case("status")]
#[case("expiry")]
#[case("disabled_policy")]
#[case("task")]
#[case("foreign")]
#[tokio::test]
async fn current_worker_checks_deny_each_stale_or_unbound_generation(
	mut live: Live,
	#[case] defect: &str,
) {
	let mut job = test_support::request(1);
	match defect {
		"tenant" => job.tenant = "other".into(),
		"status" => job.status = "QUEUED".into(),
		"expiry" => job.expires_at = live.now,
		"disabled_policy" => live.enabled = false,
		"task" => job.task_id = Uuid::from_u128(9),
		"foreign" => job.home_node = "aidash://home".into(),
		_ => panic!("unknown defect"),
	};
	live.jobs.push(job);
	assert!(matches!(check(&mut live).await, Err(Error::Forbidden)));
	assert_eq!(
		live.calls,
		["jobs", &format!("policy:{}", Uuid::from_u128(1))]
	);
}
#[rstest]
#[tokio::test]
async fn ancestor_task_binding_is_distinct_from_the_executing_generated_agent(mut live: Live) {
	let mut ancestor = test_support::request(5);
	ancestor.agent_id = "ancestor".into();
	ancestor.task_id = Uuid::from_u128(7);
	ancestor.home_node = "aidash://home".into();
	live.jobs = [ancestor, test_support::request(1)].into();
	check(&mut live).await.unwrap();
	assert_eq!(live.calls.len(), 3);
}
#[rstest]
#[tokio::test]
async fn any_revoked_ancestor_stops_the_whole_worker_chain(mut live: Live) {
	let mut ancestor = test_support::request(5);
	ancestor.agent_id = "ancestor".into();
	ancestor.status = "STOPPED".into();
	live.jobs = [ancestor, test_support::request(1)].into();
	assert!(matches!(check(&mut live).await, Err(Error::Forbidden)));
	assert_eq!(live.calls.len(), 2);
}
#[rstest]
#[tokio::test]
async fn policy_storage_failure_stops_before_execution(mut live: Live) {
	live.jobs.push(test_support::request(1));
	live.fail = true;
	assert!(matches!(check(&mut live).await, Err(Error::External(_))));
	assert_eq!(live.calls.len(), 2);
}
