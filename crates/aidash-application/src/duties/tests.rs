use super::*;
use aidash_domain::duties::{ModeChoice, Trigger, TriggerChoice};
use rstest::rstest;
use std::collections::{BTreeMap, BTreeSet};

struct Fake {
	subject: String,
	credential: Uuid,
	now: DateTime<Utc>,
	/// Threads the person holds every action on.
	threads: BTreeSet<Uuid>,
	/// Actions refused everywhere.
	refused: BTreeSet<&'static str>,
	administers: bool,
	bypass: bool,
	agents: BTreeMap<String, Vec<String>>,
	people: BTreeSet<String>,
	tier: Option<u32>,
	duties: BTreeMap<Uuid, Duty>,
	ticks: Vec<Tick>,
	/// Who last set each Budget targeting a Duty.
	budgets: BTreeMap<Uuid, Vec<String>>,
	manages_budgets: bool,
	/// The Duties handed over, and to whom.
	handed_over: Vec<(Uuid, String)>,
	/// Deleted Threads, and when.
	deleted: BTreeMap<Uuid, DateTime<Utc>>,
	/// Whether a deleted Thread's Scratch Space can no longer be used.
	scratch_gone: bool,
	/// Threads opened in place of a deleted one: (deleted, new, title).
	reopened: Vec<(Uuid, Uuid, String)>,
}

impl Fake {
	fn new(subject: &str) -> Self {
		Self {
			subject: subject.into(),
			credential: Uuid::new_v4(),
			now: "2026-11-13T00:00:00Z".parse().unwrap(),
			threads: BTreeSet::new(),
			refused: BTreeSet::new(),
			administers: false,
			bypass: false,
			agents: BTreeMap::from([(
				"reporter".to_owned(),
				vec!["1.0.0".to_owned(), "1.2.0".to_owned()],
			)]),
			people: ["alice", "bob", "carol"].map(str::to_owned).into(),
			tier: None,
			duties: BTreeMap::new(),
			ticks: vec![],
			budgets: BTreeMap::new(),
			manages_budgets: false,
			handed_over: vec![],
			deleted: BTreeMap::new(),
			scratch_gone: false,
			reopened: vec![],
		}
	}

	fn acting_as(mut self, subject: &str) -> Self {
		self.subject = subject.into();
		self.credential = Uuid::new_v4();
		self
	}
}

#[async_trait]
impl DutyScope for Fake {
	fn subject(&self) -> &str {
		&self.subject
	}
	async fn requester_credential(&mut self) -> Result<Uuid> {
		Ok(self.credential)
	}
	async fn created_by(&mut self, run: Uuid, key: &str) -> Result<Option<Duty>> {
		Ok(self
			.duties
			.values()
			.find(|duty| {
				duty.created_by_run_id == Some(run) && duty.creation_key.as_deref() == Some(key)
			})
			.cloned())
	}
	fn now(&self) -> DateTime<Utc> {
		self.now
	}
	async fn require_thread(&mut self, thread: Uuid, action: &str) -> Result<()> {
		if self.threads.contains(&thread) && !self.refused.contains(action) {
			Ok(())
		} else {
			Err(Error::Forbidden)
		}
	}
	async fn sees_thread(&mut self, thread: Uuid) -> Result<bool> {
		Ok(self.threads.contains(&thread))
	}
	async fn administers(&mut self) -> Result<bool> {
		Ok(self.administers)
	}
	async fn mode_allowed(&mut self, mode: ApprovalMode) -> Result<bool> {
		Ok(mode != ApprovalMode::Bypass || self.bypass)
	}
	async fn require_agent(&mut self, agent: &EntityRef) -> Result<()> {
		if self
			.agents
			.get(&agent.id)
			.is_some_and(|versions| versions.contains(&agent.version))
		{
			Ok(())
		} else {
			Err(Error::Forbidden)
		}
	}
	async fn newer_agent_version(&mut self, agent: &EntityRef) -> Result<Option<String>> {
		Ok(self
			.agents
			.get(&agent.id)
			.and_then(|versions| versions.last())
			.filter(|newest| **newest != agent.version)
			.cloned())
	}
	async fn is_person(&mut self, subject: &str) -> Result<bool> {
		Ok(self.people.contains(subject))
	}
	async fn tier_cap(&mut self) -> Result<Option<u32>> {
		Ok(self.tier)
	}
	async fn count(&mut self) -> Result<i64> {
		Ok(self.duties.len() as i64)
	}
	async fn open_thread(&mut self, from: Uuid, definition: &Definition) -> Result<Uuid> {
		assert!(self.threads.contains(&from));
		assert!(!definition.title.is_empty());
		// The Requester opens it, so they hold everything in it.
		let own = Uuid::new_v4();
		self.threads.insert(own);
		Ok(own)
	}
	async fn thread_deleted_at(&mut self, thread: Uuid) -> Result<Option<DateTime<Utc>>> {
		Ok(self.deleted.get(&thread).copied())
	}
	async fn reopen_thread(
		&mut self,
		deleted: Uuid,
		deleted_at: DateTime<Utc>,
		definition: &Definition,
	) -> Result<Uuid> {
		assert_eq!(self.deleted.get(&deleted), Some(&deleted_at));
		if self.scratch_gone {
			return Err(Error::Invalid("the Scratch Space is gone".into()));
		}
		let own = Uuid::new_v4();
		self.threads.insert(own);
		self.reopened.push((deleted, own, definition.title.clone()));
		Ok(own)
	}
	async fn bind_replacement_thread(&mut self, duty: &Duty, deleted: Uuid) -> Result<()> {
		let stored = self
			.duties
			.get_mut(&duty.id)
			.ok_or_else(|| Error::NotFound("duty not found".into()))?;
		assert_eq!(stored.thread_id, deleted);
		stored.thread_id = duty.thread_id;
		stored.origin_thread_id = duty.origin_thread_id;
		Ok(())
	}
	async fn insert(&mut self, duty: &Duty) -> Result<()> {
		self.duties.insert(duty.id, duty.clone());
		Ok(())
	}
	async fn load_for_update(&mut self, id: Uuid) -> Result<Duty> {
		self.load(id).await
	}
	async fn load(&mut self, id: Uuid) -> Result<Duty> {
		self.duties
			.get(&id)
			.cloned()
			.ok_or_else(|| Error::NotFound("duty not found".into()))
	}
	async fn save(&mut self, duty: &Duty) -> Result<()> {
		let mut saved = duty.clone();
		saved.revision += 1;
		self.duties.insert(duty.id, saved);
		Ok(())
	}
	async fn delete(&mut self, id: Uuid) -> Result<()> {
		self.duties.remove(&id);
		Ok(())
	}
	async fn hand_over(&mut self, duty: &Duty, successor: &str) -> Result<()> {
		assert_eq!(successor, self.subject);
		// The Duty's own Thread becomes the successor's, with every action.
		if duty.origin_thread_id.is_some() && !self.deleted.contains_key(&duty.thread_id) {
			self.threads.insert(duty.thread_id);
		}
		self.handed_over.push((duty.id, successor.to_owned()));
		Ok(())
	}
	async fn budget_setters(&mut self, duty: Uuid) -> Result<Vec<String>> {
		Ok(self.budgets.get(&duty).cloned().unwrap_or_default())
	}
	async fn manages_budgets(&mut self) -> Result<bool> {
		Ok(self.manages_budgets)
	}
	async fn list(&mut self, listing: Listing) -> Result<Vec<Duty>> {
		Ok(self
			.duties
			.values()
			.filter(|duty| match listing {
				Listing::Thread(thread) => {
					duty.thread_id == thread || duty.origin_thread_id == Some(thread)
				}
				Listing::Mine => duty.requester == self.subject,
				Listing::Tenant => true,
			})
			.cloned()
			.collect())
	}
	async fn ticks(&mut self, id: Uuid, _: u32) -> Result<Vec<Tick>> {
		Ok(self
			.ticks
			.iter()
			.filter(|tick| tick.duty_id == id)
			.cloned()
			.collect())
	}
}

fn definition() -> Definition {
	Definition {
		title: "Weekly dependency check".into(),
		mission: "List outdated dependencies.".into(),
		procedure: "1. Check every manifest.".into(),
		agent: EntityRef {
			id: "reporter".into(),
			version: "1.0.0".into(),
		},
		trigger: Some(Trigger::Cron {
			expression: "0 9 * * mon".into(),
			time_zone: "Asia/Tokyo".into(),
		}),
		approval_mode: None,
		max_lateness_seconds: 3600,
		pause_after_failures: 3,
		router: None,
		watcher: None,
	}
}

fn alice_in(thread: Uuid) -> Fake {
	let mut fake = Fake::new("alice");
	fake.threads.insert(thread);
	fake
}

#[rstest]
#[tokio::test]
async fn a_registered_duty_acts_as_its_requester_and_knows_its_next_time() {
	// Arrange
	let thread = Uuid::new_v4();
	let mut scope = alice_in(thread);
	// Act
	let duty = register(&mut scope, thread, definition(), Origin::Person)
		.await
		.unwrap();
	// Assert: Monday 09:00 in Tokyo after Friday 2026-11-13 00:00 UTC.
	assert_eq!(duty.requester, "alice");
	assert_eq!(duty.credential_id, scope.credential);
	assert_eq!(
		duty.next_scheduled_at,
		Some("2026-11-16T00:00:00Z".parse().unwrap())
	);
	assert_eq!(duty.halt, None);
	assert!(scope.duties.contains_key(&duty.id));
}

#[rstest]
#[tokio::test]
async fn a_duty_opens_its_own_thread_beside_the_one_it_was_registered_from() {
	// Arrange
	let thread = Uuid::new_v4();
	let mut scope = alice_in(thread);
	// Act
	let duty = register(&mut scope, thread, definition(), Origin::Person)
		.await
		.unwrap();
	let again = register(&mut scope, thread, definition(), Origin::Person)
		.await
		.unwrap();
	// Assert: each Duty owns a Thread of its own (#1008, ADR 0140).
	assert_ne!(duty.thread_id, thread);
	assert_ne!(duty.thread_id, again.thread_id);
	assert!(scope.threads.contains(&duty.thread_id));
}

#[rstest]
#[tokio::test]
async fn registering_needs_duty_create_and_task_create_in_the_thread() {
	for action in [duties::CREATE, "task.create", "thread.read"] {
		// Arrange
		let thread = Uuid::new_v4();
		let mut scope = alice_in(thread);
		scope.refused.insert(action);
		// Act
		let result = register(&mut scope, thread, definition(), Origin::Person).await;
		// Assert
		assert!(matches!(result, Err(Error::Forbidden)), "{action}");
		assert!(scope.duties.is_empty());
	}
}

#[rstest]
#[tokio::test]
async fn only_a_person_registers_a_duty() {
	// Arrange: a service subject holding every action in the Thread.
	let thread = Uuid::new_v4();
	let mut scope = Fake::new("nightly-service");
	scope.threads.insert(thread);
	// Act
	let result = register(&mut scope, thread, definition(), Origin::Person).await;
	// Assert
	assert!(matches!(result, Err(Error::Invalid(_))), "{result:?}");
	assert!(scope.duties.is_empty());
}

#[rstest]
#[tokio::test]
async fn a_duty_names_only_what_its_requester_may_use() {
	let thread = Uuid::new_v4();
	let mut unknown_agent = definition();
	unknown_agent.agent.version = "9.9.9".into();
	let mut bypass = definition();
	bypass.approval_mode = Some(ApprovalMode::Bypass);
	let mut negative = definition();
	negative.pause_after_failures = -1;
	for (case, expect_forbidden) in [(unknown_agent, true), (bypass, false), (negative, false)] {
		// Arrange
		let mut scope = alice_in(thread);
		// Act
		let result = register(&mut scope, thread, case, Origin::Person).await;
		// Assert
		match result {
			Err(Error::Forbidden) => assert!(expect_forbidden),
			Err(Error::Invalid(_) | Error::Domain(_)) => assert!(!expect_forbidden),
			other => panic!("unexpected {other:?}"),
		}
		assert!(scope.duties.is_empty());
	}
}

/// There is no minimum interval and no count limit but a plan's (#1012).
#[rstest]
#[tokio::test]
async fn any_frequency_and_any_number_of_duties_unless_a_plan_caps_them() {
	// Arrange
	let thread = Uuid::new_v4();
	let mut scope = alice_in(thread);
	let mut every_second = definition();
	every_second.trigger = Some(Trigger::Interval { every_seconds: 1 });
	// Act
	let frequent = register(&mut scope, thread, every_second, Origin::Person).await;
	for _ in 0..20 {
		register(&mut scope, thread, definition(), Origin::Person)
			.await
			.unwrap();
	}
	scope.tier = Some(21);
	let over_plan = register(&mut scope, thread, definition(), Origin::Person).await;
	// Assert
	assert!(frequent.is_ok(), "{frequent:?}");
	assert_eq!(scope.duties.len(), 21);
	assert!(
		matches!(over_plan, Err(Error::Domain(aidash_domain::Error::Conflict(message))) if message.starts_with("DUTY_LIMIT"))
	);
}

#[rstest]
#[tokio::test]
async fn an_agent_registers_for_the_person_at_its_root_and_the_run_is_kept() {
	// Arrange
	let thread = Uuid::new_v4();
	let run_id = Uuid::new_v4();
	let mut scope = alice_in(thread);
	// Act
	let duty = register(
		&mut scope,
		thread,
		definition(),
		Origin::Agent {
			run_id,
			key: "call-1".into(),
		},
	)
	.await
	.unwrap();
	// Assert
	assert_eq!(duty.requester, "alice");
	assert_eq!(duty.created_by_run_id, Some(run_id));
}

#[rstest]
#[tokio::test]
async fn only_the_requester_changes_it_and_a_new_trigger_counts_from_now() {
	// Arrange
	let thread = Uuid::new_v4();
	let mut alice = alice_in(thread);
	let duty = register(&mut alice, thread, definition(), Origin::Person)
		.await
		.unwrap();
	let mut bob = alice_in(thread).acting_as("bob");
	bob.duties = alice.duties.clone();
	bob.administers = true;
	let patch = Patch {
		trigger: Some(TriggerChoice::Set(Trigger::Interval {
			every_seconds: 86_400,
		})),
		approval_mode: Some(ModeChoice::DontAsk),
		..Patch::default()
	};
	// Act
	let refused = update(&mut bob, duty.id, patch.clone()).await;
	let changed = update(&mut alice, duty.id, patch).await.unwrap();
	// Assert
	assert!(refused.is_err());
	assert_eq!(
		changed.definition.approval_mode,
		Some(ApprovalMode::DontAsk)
	);
	assert_eq!(changed.anchor, alice.now);
	assert_eq!(
		changed.next_scheduled_at,
		Some("2026-11-14T00:00:00Z".parse().unwrap())
	);
}

#[rstest]
#[tokio::test]
async fn an_administrator_pauses_but_only_the_requester_resumes() {
	// Arrange
	let thread = Uuid::new_v4();
	let mut alice = alice_in(thread);
	let duty = register(&mut alice, thread, definition(), Origin::Person)
		.await
		.unwrap();
	// The administrator can see the Thread and register there too, so
	// nothing but the resume rule refuses them.
	let mut admin = alice_in(thread).acting_as("admin");
	admin.administers = true;
	admin.duties = alice.duties.clone();
	// Act
	let paused = pause(&mut admin, duty.id).await.unwrap();
	let refused = resume(&mut admin, duty.id).await;
	alice.duties = admin.duties.clone();
	let resumed = resume(&mut alice, duty.id).await.unwrap();
	// Assert
	assert_eq!(paused.halt, Some(Halt::Paused));
	assert!(
		matches!(&refused, Err(Error::Domain(aidash_domain::Error::Invalid(message)))
			if message.starts_with("only the Duty's Requester can resume it")),
		"{refused:?}"
	);
	assert_eq!(resumed.halt, None);
	assert_eq!(resumed.requester, "alice");
}

#[rstest]
#[tokio::test]
async fn a_successor_takes_a_departed_requesters_duty_over_then_resumes_it() {
	// Arrange: Alice left; the Duty stopped.
	let thread = Uuid::new_v4();
	let mut alice = alice_in(thread);
	let mut duty = register(&mut alice, thread, definition(), Origin::Person)
		.await
		.unwrap();
	duty.stop(Halt::RequesterLeft, alice.now);
	let mut admin = Fake::new("admin");
	admin.administers = true;
	admin.duties.insert(duty.id, duty.clone());
	// Act
	let named = name_successor(&mut admin, duty.id, "carol").await.unwrap();
	// Both may read the Duty's own Thread (#1008); only the successor takes
	// it over, and resuming does not (ADR 0145).
	let mut carol = alice_in(thread).acting_as("carol");
	carol.duties = admin.duties.clone();
	carol.threads.insert(duty.thread_id);
	let mut bob = alice_in(thread).acting_as("bob");
	bob.duties = admin.duties.clone();
	bob.threads.insert(duty.thread_id);
	let refused = take_over(&mut bob, duty.id).await;
	let resumed_first = resume(&mut carol, duty.id).await;
	let taken = take_over(&mut carol, duty.id).await.unwrap();
	let resumed = resume(&mut carol, duty.id).await.unwrap();
	// Assert
	assert_eq!(named.successor.as_deref(), Some("carol"));
	assert!(refused.is_err());
	assert!(resumed_first.is_err());
	assert_eq!(taken.requester, "carol");
	assert_eq!(taken.halt, Some(Halt::Paused));
	assert_eq!(taken.successor, None);
	assert_eq!(
		taken.credential_id, duty.credential_id,
		"taking over changes no credential; resuming does"
	);
	assert_eq!(resumed.credential_id, carol.credential);
	assert_eq!(resumed.halt, None);
	assert!(
		name_successor(&mut admin, duty.id, "mallory")
			.await
			.is_err()
	);
}

#[rstest]
#[tokio::test]
async fn the_named_successor_sees_the_duty_without_its_thread() {
	// Arrange: Alice left, and Carol, who cannot see the Thread, is named.
	let thread = Uuid::new_v4();
	let mut alice = alice_in(thread);
	let mut duty = register(&mut alice, thread, definition(), Origin::Person)
		.await
		.unwrap();
	duty.stop(Halt::RequesterLeft, alice.now);
	duty.successor = Some("carol".into());
	let mut carol = Fake::new("carol");
	carol.duties.insert(duty.id, duty.clone());
	let mut bob = Fake::new("bob");
	bob.duties = carol.duties.clone();
	// Act
	let shown = detail(&mut carol, duty.id).await.unwrap();
	let hidden = detail(&mut bob, duty.id).await;
	// Assert: she may take it over; nobody else outside the Thread sees it.
	assert!(!shown.may_edit);
	assert!(matches!(hidden, Err(Error::NotFound(_))));
}

#[rstest]
#[tokio::test]
async fn others_see_a_duty_only_through_its_thread() {
	// Arrange
	let thread = Uuid::new_v4();
	let mut alice = alice_in(thread);
	let duty = register(&mut alice, thread, definition(), Origin::Person)
		.await
		.unwrap();
	let mut outsider = Fake::new("bob");
	outsider.duties = alice.duties.clone();
	// Act + Assert
	assert!(matches!(
		detail(&mut outsider, duty.id).await,
		Err(Error::NotFound(_))
	));
	let shown = detail(&mut alice, duty.id).await.unwrap();
	assert!(shown.may_edit);
	assert_eq!(shown.newer_agent_version.as_deref(), Some("1.2.0"));
	assert!(matches!(
		list(&mut outsider, Listing::Tenant).await,
		Err(Error::Forbidden)
	));
}

#[rstest]
#[tokio::test]
async fn a_retried_agent_call_returns_the_duty_it_registered() {
	// Arrange
	let thread = Uuid::new_v4();
	let run_id = Uuid::new_v4();
	let mut scope = alice_in(thread);
	let origin = Origin::Agent {
		run_id,
		key: "call-7".into(),
	};
	// Act
	let first = register(&mut scope, thread, definition(), origin.clone())
		.await
		.unwrap();
	let again = register(&mut scope, thread, definition(), origin)
		.await
		.unwrap();
	let another = register(
		&mut scope,
		thread,
		definition(),
		Origin::Agent {
			run_id,
			key: "call-8".into(),
		},
	)
	.await
	.unwrap();
	// Assert
	assert_eq!(again.id, first.id);
	assert_ne!(another.id, first.id);
	assert_eq!(scope.duties.len(), 2);
}

#[rstest]
#[tokio::test]
async fn a_watcher_runs_at_the_next_pass_once_set_and_never_once_removed() {
	// Arrange
	let thread = Uuid::new_v4();
	let mut fake = alice_in(thread);
	let duty = register(&mut fake, thread, definition(), Origin::Person)
		.await
		.unwrap();
	assert_eq!(duty.watcher_next_at, None);
	let watcher: aidash_domain::duties::Patch = serde_json::from_value(serde_json::json!({
		"watcher": {"script": {"language": "shell", "source": "echo '[]'"}, "every_seconds": 300},
	}))
	.unwrap();
	// Act
	let watched = update(&mut fake, duty.id, watcher).await.unwrap();
	let removal: aidash_domain::duties::Patch =
		serde_json::from_value(serde_json::json!({"watcher": {"kind": "none"}})).unwrap();
	let unwatched = update(&mut fake, duty.id, removal).await.unwrap();
	// Assert: the time trigger did not move, the Watcher's own clock did.
	assert_eq!(watched.watcher_next_at, Some(fake.now));
	assert_eq!(watched.next_scheduled_at, duty.next_scheduled_at);
	assert_eq!(unwatched.watcher_next_at, None);
}

mod templates {
	//! Duties set up from Marketplace templates (#1011 D4).
	use super::*;
	use aidash_domain::duties::{
		events::TriggerKind,
		templates::{Slot, Source, Template, TriggerNote},
	};

	fn source(version: &str) -> Source {
		Source {
			key: format!("key-{version}"),
			repository: "aidash://deployment/node".into(),
			owner_tenant: "publisher".into(),
			package_id: "dependency-check".into(),
			version: version.into(),
			install_key: Some(Uuid::nil()),
		}
	}

	fn template() -> Template {
		Template::from_duty(
			&definition(),
			&[TriggerKind::Webhook],
			&[
				TriggerNote {
					kind: Slot::Time,
					instruction: "Monday mornings.".into(),
				},
				TriggerNote {
					kind: Slot::Webhook,
					instruction: "Point the repository's push hook here.".into(),
				},
			],
			true,
		)
		.unwrap()
	}

	#[rstest]
	#[tokio::test]
	async fn the_person_who_installs_a_template_is_its_requester() {
		// Arrange: bob installs a template alice's Duty became.
		let thread = Uuid::new_v4();
		let mut scope = Fake::new("bob");
		scope.threads.insert(thread);
		// Act
		let duty = install_template(&mut scope, thread, &template(), None, source("1.0.0"))
			.await
			.unwrap();
		// Assert: bob's Duty, with the suggested Agent and the time trigger.
		assert_eq!(duty.requester, "bob");
		assert_eq!(duty.credential_id, scope.credential);
		assert_eq!(duty.definition.agent, definition().agent);
		assert_eq!(duty.definition.trigger, definition().trigger);
		assert!(duty.next_scheduled_at.is_some());
		assert_eq!(duty.template, Some(source("1.0.0")));
		assert_eq!(duty.created_by_run_id, None);
	}

	#[rstest]
	#[tokio::test]
	async fn the_chosen_agent_staffs_it_and_an_unapproved_one_is_refused() {
		let thread = Uuid::new_v4();
		let mut scope = Fake::new("bob");
		scope.threads.insert(thread);
		let chosen = EntityRef {
			id: "reporter".into(),
			version: "1.2.0".into(),
		};
		let duty = install_template(
			&mut scope,
			thread,
			&template(),
			Some(chosen.clone()),
			source("1.0.0"),
		)
		.await
		.unwrap();
		assert_eq!(duty.definition.agent, chosen);
		// A suggested Agent this Node never approved is refused, as any is.
		let mut foreign = template();
		foreign.agent.as_mut().unwrap().id = "unknown".into();
		let refused = install_template(&mut scope, thread, &foreign, None, source("1.0.0")).await;
		assert!(matches!(refused, Err(Error::Forbidden)), "{refused:?}");
		// No suggestion and no choice: the person must pick one.
		foreign.agent = None;
		let unstaffed = install_template(&mut scope, thread, &foreign, None, source("1.0.0")).await;
		assert!(matches!(unstaffed, Err(Error::Invalid(_))), "{unstaffed:?}");
	}

	#[rstest]
	#[tokio::test]
	async fn installing_needs_what_setting_up_any_duty_needs() {
		for action in [duties::CREATE, "task.create", "thread.read"] {
			let thread = Uuid::new_v4();
			let mut scope = Fake::new("bob");
			scope.threads.insert(thread);
			scope.refused.insert(action);
			let result =
				install_template(&mut scope, thread, &template(), None, source("1.0.0")).await;
			assert!(matches!(result, Err(Error::Forbidden)), "{action}");
			assert!(scope.duties.is_empty());
		}
	}

	#[rstest]
	#[tokio::test]
	async fn a_later_version_updates_the_work_only_when_the_requester_applies_it() {
		// Arrange
		let thread = Uuid::new_v4();
		let mut scope = Fake::new("bob");
		scope.threads.insert(thread);
		let duty = install_template(&mut scope, thread, &template(), None, source("1.0.0"))
			.await
			.unwrap();
		let renamed = update(
			&mut scope,
			duty.id,
			Patch {
				title: Some("Bob's check".into()),
				..Patch::default()
			},
		)
		.await
		.unwrap();
		let mut next = template();
		next.mission = "List outdated and vulnerable dependencies.".into();
		// Act
		let updated = update_from_template(&mut scope, duty.id, &next, source("1.1.0"))
			.await
			.unwrap();
		// Assert: the new work, bob's title, the new version, the same install key.
		assert_eq!(updated.definition.mission, next.mission);
		assert_eq!(updated.definition.title, renamed.definition.title);
		assert_eq!(updated.template, Some(source("1.1.0")));
		// Act + Assert: an older or the same version never applies.
		for stale in ["1.1.0", "0.9.0"] {
			let result = update_from_template(&mut scope, duty.id, &next, source(stale)).await;
			assert!(matches!(result, Err(Error::Invalid(_))), "{stale}");
		}
		// Act + Assert: only the Requester applies it.
		let mut alice = scope.acting_as("alice");
		alice.administers = true;
		let result = update_from_template(&mut alice, duty.id, &next, source("2.0.0")).await;
		assert!(result.is_err());
	}

	#[rstest]
	#[tokio::test]
	async fn only_its_requester_publishes_a_duty() {
		let thread = Uuid::new_v4();
		let mut scope = alice_in(thread);
		let duty = register(&mut scope, thread, definition(), Origin::Person)
			.await
			.unwrap();
		assert_eq!(own(&mut scope, duty.id).await.unwrap().id, duty.id);
		let mut bob = scope.acting_as("bob");
		bob.threads.insert(thread);
		assert!(matches!(
			own(&mut bob, duty.id).await,
			Err(Error::Invalid(_))
		));
	}
}

/// Taking a Duty over makes its own Thread the successor's, so a policy that
/// limits Threads to their owner lets them resume it (#1011, ADR 0145).
/// Resuming hands nothing over.
#[rstest]
#[tokio::test]
async fn a_successor_takes_over_the_dutys_own_thread() {
	// Arrange: Alice left; Carol, named, holds nothing in the Duty's Thread.
	let thread = Uuid::new_v4();
	let mut alice = alice_in(thread);
	let mut duty = register(&mut alice, thread, definition(), Origin::Person)
		.await
		.unwrap();
	assert!(duty.origin_thread_id.is_some());
	duty.stop(Halt::RequesterLeft, alice.now);
	duty.successor = Some("carol".into());
	let mut carol = alice_in(thread).acting_as("carol");
	carol.duties.insert(duty.id, duty.clone());
	assert!(!carol.threads.contains(&duty.thread_id));
	// Act
	let taken = take_over(&mut carol, duty.id).await.unwrap();
	// Assert
	assert_eq!(taken.requester, "carol");
	assert_eq!(carol.handed_over, vec![(duty.id, "carol".to_owned())]);
	assert!(carol.threads.contains(&duty.thread_id));
	resume(&mut carol, duty.id).await.unwrap();
	assert_eq!(
		carol.handed_over.len(),
		1,
		"the Requester's own resume hands nothing over"
	);
}

/// A Duty whose own Thread was deleted gets a new one when its Requester
/// resumes it, in place of the deleted one, whatever stopped it (#991,
/// ADR 0145).
#[rstest]
#[case(Halt::ThreadDeleted)]
#[case(Halt::Paused)]
#[case(Halt::Failures)]
#[tokio::test]
async fn resuming_a_duty_whose_thread_was_deleted_opens_a_new_one(#[case] halt: Halt) {
	// Arrange
	let thread = Uuid::new_v4();
	let mut alice = alice_in(thread);
	let mut duty = register(&mut alice, thread, definition(), Origin::Person)
		.await
		.unwrap();
	let old = duty.thread_id;
	duty.stop(halt, alice.now);
	alice.duties.insert(duty.id, duty.clone());
	alice.deleted.insert(old, alice.now);
	alice.threads.remove(&old);
	// Act
	let resumed = resume(&mut alice, duty.id).await.unwrap();
	// Assert
	assert_eq!(resumed.halt, None);
	assert_ne!(resumed.thread_id, old);
	assert_eq!(
		alice.reopened,
		vec![(old, resumed.thread_id, definition().title)]
	);
	assert_eq!(resumed.origin_thread_id, Some(thread));
}

/// An edit is the Requester's own action too: it opens the new Thread with
/// the edited title and leaves the Duty stopped.
#[rstest]
#[tokio::test]
async fn editing_a_duty_whose_thread_was_deleted_opens_a_new_one_and_keeps_it_stopped() {
	// Arrange
	let thread = Uuid::new_v4();
	let mut alice = alice_in(thread);
	let mut duty = register(&mut alice, thread, definition(), Origin::Person)
		.await
		.unwrap();
	let old = duty.thread_id;
	duty.stop(Halt::ThreadDeleted, alice.now);
	alice.duties.insert(duty.id, duty.clone());
	alice.deleted.insert(old, alice.now);
	alice.threads.remove(&old);
	// Act
	let edited = update(
		&mut alice,
		duty.id,
		Patch {
			title: Some("Daily dependency check".into()),
			..Patch::default()
		},
	)
	.await
	.unwrap();
	// Assert
	assert_eq!(edited.halt, Some(Halt::ThreadDeleted));
	assert_ne!(edited.thread_id, old);
	assert_eq!(alice.reopened.len(), 1);
	assert_eq!(alice.reopened[0].2, "Daily dependency check");
	// A later resume keeps the new Thread.
	let resumed = resume(&mut alice, duty.id).await.unwrap();
	assert_eq!(resumed.thread_id, edited.thread_id);
	assert_eq!(alice.reopened.len(), 1);
}

/// The issue #991 reported: the Requester left and the Duty's Thread was
/// deleted. The successor takes it over and resumes it in a new Thread.
#[rstest]
#[tokio::test]
async fn a_successor_recovers_a_duty_whose_thread_was_deleted() {
	// Arrange
	let thread = Uuid::new_v4();
	let mut alice = alice_in(thread);
	let mut duty = register(&mut alice, thread, definition(), Origin::Person)
		.await
		.unwrap();
	let old = duty.thread_id;
	duty.stop(Halt::RequesterLeft, alice.now);
	duty.successor = Some("carol".into());
	let mut carol = alice_in(thread).acting_as("carol");
	carol.duties.insert(duty.id, duty.clone());
	carol.deleted.insert(old, alice.now);
	// Act
	let taken = take_over(&mut carol, duty.id).await.unwrap();
	let resumed = resume(&mut carol, duty.id).await.unwrap();
	// Assert
	assert!(
		!carol.threads.contains(&old),
		"a deleted Thread is not handed over"
	);
	assert_eq!(taken.thread_id, old);
	assert_eq!(resumed.requester, "carol");
	assert_eq!(resumed.halt, None);
	assert_ne!(resumed.thread_id, old);
	assert!(carol.threads.contains(&resumed.thread_id));
}

/// The new Thread opens only where the deleted one was: when that Scratch
/// Space is gone, resuming refuses and the Duty stays as it was.
#[rstest]
#[tokio::test]
async fn a_duty_is_never_moved_to_another_scratch_space() {
	// Arrange
	let thread = Uuid::new_v4();
	let mut alice = alice_in(thread);
	let mut duty = register(&mut alice, thread, definition(), Origin::Person)
		.await
		.unwrap();
	let old = duty.thread_id;
	duty.stop(Halt::ThreadDeleted, alice.now);
	alice.duties.insert(duty.id, duty.clone());
	alice.deleted.insert(old, alice.now);
	alice.scratch_gone = true;
	// Act
	let refused = resume(&mut alice, duty.id).await;
	// Assert
	assert!(matches!(refused, Err(Error::Invalid(_))), "{refused:?}");
	let kept = &alice.duties[&duty.id];
	assert_eq!(kept.thread_id, old);
	assert_eq!(kept.halt, Some(Halt::ThreadDeleted));
}

/// A Duty from before #1008 worked in the Thread it was registered in. Its
/// new Thread is its own, so that Thread becomes its origin and a later
/// takeover hands the new one over.
#[rstest]
#[tokio::test]
async fn a_duty_from_before_own_threads_gets_one_when_its_thread_goes() {
	// Arrange
	let thread = Uuid::new_v4();
	let mut alice = alice_in(thread);
	let mut duty = register(&mut alice, thread, definition(), Origin::Person)
		.await
		.unwrap();
	duty.thread_id = thread;
	duty.origin_thread_id = None;
	duty.stop(Halt::ThreadDeleted, alice.now);
	alice.duties.insert(duty.id, duty.clone());
	alice.deleted.insert(thread, alice.now);
	// Act
	let resumed = resume(&mut alice, duty.id).await.unwrap();
	// Assert
	assert_ne!(resumed.thread_id, thread);
	assert_eq!(resumed.origin_thread_id, Some(thread));
}

/// Deleting a Duty removes its Budgets (#1012 decision 25): a person needs
/// `budget.manage` for one someone else set, and an Agent deletes none that
/// a Budget targets, though it may pause it.
#[rstest]
#[tokio::test]
async fn deleting_a_duty_never_lifts_a_budget_someone_else_set() {
	// Arrange: an administrator set a Budget on Alice's Duty.
	let thread = Uuid::new_v4();
	let mut alice = alice_in(thread);
	let duty = register(&mut alice, thread, definition(), Origin::Person)
		.await
		.unwrap();
	alice.budgets.insert(duty.id, vec!["admin".into()]);
	// Act
	let by_agent = delete_by_agent(&mut alice, duty.id).await;
	let by_person = delete(&mut alice, duty.id).await;
	let paused = pause(&mut alice, duty.id).await;
	// Assert
	assert!(
		matches!(&by_agent, Err(Error::Domain(aidash_domain::Error::Invalid(message)))
			if message.contains("an Agent never removes a Budget")),
		"{by_agent:?}"
	);
	assert!(
		matches!(&by_person, Err(Error::Invalid(message)) if message == duties::BUDGET_SET_BY_SOMEONE_ELSE),
		"{by_person:?}"
	);
	assert_eq!(paused.unwrap().halt, Some(Halt::Paused));
	assert!(alice.duties.contains_key(&duty.id));
	// A Budget Alice set herself still stops an Agent, but not Alice.
	alice.budgets.insert(duty.id, vec!["alice".into()]);
	assert!(delete_by_agent(&mut alice, duty.id).await.is_err());
	delete(&mut alice, duty.id).await.unwrap();
	assert!(!alice.duties.contains_key(&duty.id));
}

/// `budget.manage` lets a person delete a Duty whatever Budgets target it.
#[rstest]
#[tokio::test]
async fn budget_manage_deletes_a_duty_with_anyones_budget() {
	// Arrange
	let thread = Uuid::new_v4();
	let mut alice = alice_in(thread);
	let duty = register(&mut alice, thread, definition(), Origin::Person)
		.await
		.unwrap();
	alice.budgets.insert(duty.id, vec!["admin".into()]);
	alice.manages_budgets = true;
	// Act
	delete(&mut alice, duty.id).await.unwrap();
	// Assert
	assert!(!alice.duties.contains_key(&duty.id));
}
