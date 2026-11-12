use super::*;
use aidash_domain::schedules::{ModeChoice, Trigger};
use rstest::rstest;
use std::collections::{BTreeMap, BTreeSet};

struct Fake {
	subject: String,
	credential: Uuid,
	now: DateTime<Utc>,
	/// Workspaces the person holds every action on.
	workspaces: BTreeSet<Uuid>,
	/// Actions refused everywhere.
	refused: BTreeSet<&'static str>,
	administers: bool,
	bypass: bool,
	agents: BTreeMap<String, Vec<String>>,
	threads: BTreeSet<(Uuid, Uuid)>,
	people: BTreeSet<String>,
	limits: Limits,
	ceilings: Ceilings,
	schedules: BTreeMap<Uuid, Schedule>,
	ticks: Vec<Tick>,
}

impl Fake {
	fn new(subject: &str) -> Self {
		Self {
			subject: subject.into(),
			credential: Uuid::new_v4(),
			now: "2026-11-13T00:00:00Z".parse().unwrap(),
			workspaces: BTreeSet::new(),
			refused: BTreeSet::new(),
			administers: false,
			bypass: false,
			agents: BTreeMap::from([(
				"reporter".to_owned(),
				vec!["1.0.0".to_owned(), "1.2.0".to_owned()],
			)]),
			threads: BTreeSet::new(),
			people: ["alice", "bob", "carol"].map(str::to_owned).into(),
			limits: Limits::default(),
			ceilings: Ceilings::default(),
			schedules: BTreeMap::new(),
			ticks: vec![],
		}
	}

	fn as_person(mut self, subject: &str) -> Self {
		self.subject = subject.into();
		self.credential = Uuid::new_v4();
		self
	}
}

#[async_trait]
impl ScheduleScope for Fake {
	fn subject(&self) -> &str {
		&self.subject
	}
	fn credential(&self) -> Uuid {
		self.credential
	}
	fn now(&self) -> DateTime<Utc> {
		self.now
	}
	async fn require_workspace(&mut self, workspace: Uuid, action: &str) -> Result<()> {
		if self.workspaces.contains(&workspace) && !self.refused.contains(action) {
			Ok(())
		} else {
			Err(Error::Forbidden)
		}
	}
	async fn sees_workspace(&mut self, workspace: Uuid) -> Result<bool> {
		Ok(self.workspaces.contains(&workspace))
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
	async fn thread_in_workspace(&mut self, workspace: Uuid, thread: Uuid) -> Result<bool> {
		Ok(self.threads.contains(&(workspace, thread)))
	}
	async fn is_person(&mut self, subject: &str) -> Result<bool> {
		Ok(self.people.contains(subject))
	}
	async fn limits(&mut self) -> Result<(Limits, Ceilings, Option<u32>)> {
		Ok((self.limits, self.ceilings, None))
	}
	async fn save_limits(&mut self, limits: &Limits) -> Result<()> {
		self.limits = *limits;
		Ok(())
	}
	async fn counts(&mut self, registrant: &str) -> Result<(i64, i64)> {
		Ok((
			self.schedules.len() as i64,
			self.schedules
				.values()
				.filter(|schedule| schedule.registrant == registrant)
				.count() as i64,
		))
	}
	async fn insert(&mut self, schedule: &Schedule) -> Result<()> {
		self.schedules.insert(schedule.id, schedule.clone());
		Ok(())
	}
	async fn load_for_update(&mut self, id: Uuid) -> Result<Schedule> {
		self.load(id).await
	}
	async fn load(&mut self, id: Uuid) -> Result<Schedule> {
		self.schedules
			.get(&id)
			.cloned()
			.ok_or_else(|| Error::NotFound("schedule not found".into()))
	}
	async fn save(&mut self, schedule: &Schedule) -> Result<()> {
		let mut saved = schedule.clone();
		saved.revision += 1;
		self.schedules.insert(schedule.id, saved);
		Ok(())
	}
	async fn delete(&mut self, id: Uuid) -> Result<()> {
		self.schedules.remove(&id);
		Ok(())
	}
	async fn list(&mut self, listing: Listing) -> Result<Vec<Schedule>> {
		Ok(self
			.schedules
			.values()
			.filter(|schedule| match listing {
				Listing::Workspace(workspace) => schedule.workspace_id == workspace,
				Listing::Mine => schedule.registrant == self.subject,
				Listing::Tenant => true,
			})
			.cloned()
			.collect())
	}
	async fn ticks(&mut self, id: Uuid, _: u32) -> Result<Vec<Tick>> {
		Ok(self
			.ticks
			.iter()
			.filter(|tick| tick.schedule_id == id)
			.cloned()
			.collect())
	}
}

fn definition() -> Definition {
	Definition {
		title: "Weekly dependency check".into(),
		request: "List outdated dependencies.".into(),
		agent: EntityRef {
			id: "reporter".into(),
			version: "1.0.0".into(),
		},
		trigger: Trigger::Cron {
			expression: "0 9 * * mon".into(),
			time_zone: "Asia/Tokyo".into(),
		},
		thread: ThreadMode::New,
		approval_mode: None,
		max_lateness_seconds: 3600,
	}
}

fn alice_in(workspace: Uuid) -> Fake {
	let mut fake = Fake::new("alice");
	fake.workspaces.insert(workspace);
	fake
}

#[rstest]
#[tokio::test]
async fn a_registered_schedule_acts_as_its_registrant_and_knows_its_next_time() {
	// Arrange
	let workspace = Uuid::new_v4();
	let mut scope = alice_in(workspace);
	// Act
	let schedule = register(&mut scope, workspace, definition(), Origin::Person)
		.await
		.unwrap();
	// Assert: Monday 09:00 in Tokyo after Friday 2026-11-13 00:00 UTC.
	assert_eq!(schedule.registrant, "alice");
	assert_eq!(schedule.credential_id, scope.credential);
	assert_eq!(
		schedule.next_scheduled_at,
		Some("2026-11-16T00:00:00Z".parse().unwrap())
	);
	assert_eq!(schedule.halt, None);
	assert!(scope.schedules.contains_key(&schedule.id));
}

#[rstest]
#[tokio::test]
async fn registering_needs_schedule_create_and_task_create_in_the_workspace() {
	for action in [schedules::CREATE, "task.create", "workspace.read"] {
		// Arrange
		let workspace = Uuid::new_v4();
		let mut scope = alice_in(workspace);
		scope.refused.insert(action);
		// Act
		let result = register(&mut scope, workspace, definition(), Origin::Person).await;
		// Assert
		assert!(matches!(result, Err(Error::Forbidden)), "{action}");
		assert!(scope.schedules.is_empty());
	}
}

#[rstest]
#[tokio::test]
async fn a_schedule_names_only_what_its_registrant_may_use() {
	let workspace = Uuid::new_v4();
	let mut unknown_agent = definition();
	unknown_agent.agent.version = "9.9.9".into();
	let mut foreign_thread = definition();
	foreign_thread.thread = ThreadMode::Fixed {
		thread_id: Uuid::new_v4(),
	};
	let mut bypass = definition();
	bypass.approval_mode = Some(ApprovalMode::Bypass);
	let mut frequent = definition();
	frequent.trigger = Trigger::Interval { every_seconds: 60 };
	for (case, expect_forbidden) in [
		(unknown_agent, true),
		(foreign_thread, false),
		(bypass, false),
		(frequent, false),
	] {
		// Arrange
		let mut scope = alice_in(workspace);
		// Act
		let result = register(&mut scope, workspace, case, Origin::Person).await;
		// Assert
		match result {
			Err(Error::Forbidden) => assert!(expect_forbidden),
			Err(Error::Invalid(_) | Error::Domain(_)) => assert!(!expect_forbidden),
			other => panic!("unexpected {other:?}"),
		}
		assert!(scope.schedules.is_empty());
	}
}

#[rstest]
#[tokio::test]
async fn the_tenant_raises_the_minimum_interval_and_bounds_the_counts() {
	// Arrange
	let workspace = Uuid::new_v4();
	let mut scope = alice_in(workspace);
	scope.limits = Limits {
		max_schedules: 50,
		max_per_person: 1,
		min_interval_seconds: 7200,
	};
	let mut hourly = definition();
	hourly.trigger = Trigger::Interval {
		every_seconds: 3600,
	};
	// Act
	let too_frequent = register(&mut scope, workspace, hourly, Origin::Person).await;
	register(&mut scope, workspace, definition(), Origin::Person)
		.await
		.unwrap();
	let over_limit = register(&mut scope, workspace, definition(), Origin::Person).await;
	// Assert
	assert!(too_frequent.is_err());
	assert!(
		matches!(over_limit, Err(Error::Domain(aidash_domain::Error::Conflict(message))) if message.starts_with("SCHEDULE_LIMIT"))
	);
}

#[rstest]
#[tokio::test]
async fn limits_stay_within_the_deployment_ceilings_and_need_administer() {
	// Arrange
	let mut scope = Fake::new("admin");
	let wanted = Limits {
		max_schedules: 20,
		max_per_person: 2,
		min_interval_seconds: 3600,
	};
	// Act + Assert
	assert!(matches!(
		set_limits(&mut scope, wanted).await,
		Err(Error::Forbidden)
	));
	scope.administers = true;
	assert_eq!(set_limits(&mut scope, wanted).await.unwrap(), wanted);
	assert!(
		set_limits(
			&mut scope,
			Limits {
				min_interval_seconds: 60,
				..wanted
			}
		)
		.await
		.is_err()
	);
}

#[rstest]
#[tokio::test]
async fn an_agent_registers_for_the_person_at_its_root_and_the_run_is_kept() {
	// Arrange
	let workspace = Uuid::new_v4();
	let run_id = Uuid::new_v4();
	let mut scope = alice_in(workspace);
	// Act
	let schedule = register(
		&mut scope,
		workspace,
		definition(),
		Origin::Agent { run_id },
	)
	.await
	.unwrap();
	// Assert
	assert_eq!(schedule.registrant, "alice");
	assert_eq!(schedule.created_by_run_id, Some(run_id));
}

#[rstest]
#[tokio::test]
async fn only_the_registrant_changes_it_and_a_new_trigger_counts_from_now() {
	// Arrange
	let workspace = Uuid::new_v4();
	let mut alice = alice_in(workspace);
	let schedule = register(&mut alice, workspace, definition(), Origin::Person)
		.await
		.unwrap();
	let mut bob = alice_in(workspace).as_person("bob");
	bob.schedules = alice.schedules.clone();
	bob.administers = true;
	let patch = Patch {
		trigger: Some(Trigger::Interval {
			every_seconds: 86_400,
		}),
		approval_mode: Some(ModeChoice::DontAsk),
		..Patch::default()
	};
	// Act
	let refused = update(&mut bob, schedule.id, patch.clone()).await;
	let changed = update(&mut alice, schedule.id, patch).await.unwrap();
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
async fn an_administrator_pauses_but_only_the_registrant_resumes() {
	// Arrange
	let workspace = Uuid::new_v4();
	let mut alice = alice_in(workspace);
	let schedule = register(&mut alice, workspace, definition(), Origin::Person)
		.await
		.unwrap();
	let mut admin = Fake::new("admin");
	admin.administers = true;
	admin.schedules = alice.schedules.clone();
	// Act
	let paused = pause(&mut admin, schedule.id).await.unwrap();
	let refused = resume(&mut admin, schedule.id).await;
	alice.schedules = admin.schedules.clone();
	let resumed = resume(&mut alice, schedule.id).await.unwrap();
	// Assert
	assert_eq!(paused.halt, Some(Halt::Paused));
	assert!(refused.is_err());
	assert_eq!(resumed.halt, None);
	assert_eq!(resumed.registrant, "alice");
}

#[rstest]
#[tokio::test]
async fn a_successor_resumes_a_departed_registrants_schedule_as_their_own() {
	// Arrange: Alice left; the Schedule stopped.
	let workspace = Uuid::new_v4();
	let mut alice = alice_in(workspace);
	let mut schedule = register(&mut alice, workspace, definition(), Origin::Person)
		.await
		.unwrap();
	schedule.stop(Halt::RegistrantLeft, alice.now);
	let mut admin = Fake::new("admin");
	admin.administers = true;
	admin.schedules.insert(schedule.id, schedule.clone());
	// Act
	let named = name_successor(&mut admin, schedule.id, "carol")
		.await
		.unwrap();
	let mut carol = alice_in(workspace).as_person("carol");
	carol.schedules = admin.schedules.clone();
	let mut bob = alice_in(workspace).as_person("bob");
	bob.schedules = admin.schedules.clone();
	let refused = resume(&mut bob, schedule.id).await;
	let taken = resume(&mut carol, schedule.id).await.unwrap();
	// Assert
	assert_eq!(named.successor.as_deref(), Some("carol"));
	assert!(refused.is_err());
	assert_eq!(taken.registrant, "carol");
	assert_eq!(taken.credential_id, carol.credential);
	assert_eq!(taken.halt, None);
	assert_eq!(taken.successor, None);
	assert!(
		name_successor(&mut admin, schedule.id, "mallory")
			.await
			.is_err()
	);
}

#[rstest]
#[tokio::test]
async fn others_see_a_schedule_only_through_its_workspace() {
	// Arrange
	let workspace = Uuid::new_v4();
	let mut alice = alice_in(workspace);
	let schedule = register(&mut alice, workspace, definition(), Origin::Person)
		.await
		.unwrap();
	let mut outsider = Fake::new("bob");
	outsider.schedules = alice.schedules.clone();
	// Act + Assert
	assert!(matches!(
		detail(&mut outsider, schedule.id).await,
		Err(Error::NotFound(_))
	));
	let shown = detail(&mut alice, schedule.id).await.unwrap();
	assert!(shown.may_edit);
	assert_eq!(shown.newer_agent_version.as_deref(), Some("1.2.0"));
	assert!(matches!(
		list(&mut outsider, Listing::Tenant).await,
		Err(Error::Forbidden)
	));
}
