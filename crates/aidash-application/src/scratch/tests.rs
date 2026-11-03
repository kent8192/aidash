use super::*;
use chrono::{Duration, TimeZone};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Default)]
struct Memory {
	person: String,
	deny_node: bool,
	deny_create: bool,
	deny_references: bool,
	node_people: Vec<String>,
	spaces: BTreeMap<Uuid, ScratchSpace>,
	binding: Option<(Uuid, String)>,
	creator: Option<(Uuid, Run)>,
	bound_run: Option<Uuid>,
	cleared: Vec<Uuid>,
	read_loads: usize,
	locked_loads: usize,
}

#[async_trait]
impl ScratchScope for Memory {
	fn person(&self) -> &str {
		&self.person
	}
	async fn require_node_use(&mut self, _: &NodeName, person: &str) -> Result<()> {
		self.node_people.push(person.to_owned());
		if self.deny_node {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
	async fn require_create(&mut self, _: &NodeName) -> Result<()> {
		if self.deny_create {
			Err(Error::Forbidden)
		} else {
			Ok(())
		}
	}
	async fn limits(&mut self, _: &NodeName) -> Result<(ScratchCeilings, HostAllowlist)> {
		Ok((
			ScratchCeilings::default(),
			HostAllowlist::Hosts(BTreeSet::from(["example.com".into()])),
		))
	}
	async fn validate_references(
		&mut self,
		_: &NodeName,
		settings: &ScratchSettings,
	) -> Result<()> {
		if (self.deny_references && !settings.external_services.is_empty())
			|| settings.external_services.iter().any(|id| id != "approved")
		{
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	async fn validate_runtime(&mut self, _: &NodeName, _: &ScratchSettings) -> Result<()> {
		Ok(())
	}
	async fn validate_members(&mut self, members: &BTreeSet<String>) -> Result<()> {
		if members
			.iter()
			.any(|member| member != "alice" && member != "bob")
		{
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	async fn insert(&mut self, space: &ScratchSpace) -> Result<()> {
		self.spaces.insert(space.id, space.clone());
		Ok(())
	}
	async fn load_for_update(&mut self, id: Uuid) -> Result<ScratchSpace> {
		self.locked_loads += 1;
		self.spaces
			.get(&id)
			.cloned()
			.ok_or_else(|| Error::NotFound("space".into()))
	}
	async fn load(&mut self, id: Uuid) -> Result<ScratchSpace> {
		self.read_loads += 1;
		self.spaces
			.get(&id)
			.cloned()
			.ok_or_else(|| Error::NotFound("space".into()))
	}
	async fn list(&mut self, node: &NodeName) -> Result<Vec<ScratchSpace>> {
		Ok(self
			.spaces
			.values()
			.filter(|space| &space.node == node)
			.cloned()
			.collect())
	}
	async fn save(&mut self, space: &ScratchSpace) -> Result<()> {
		self.spaces.insert(space.id, space.clone());
		Ok(())
	}
	async fn delete(&mut self, id: Uuid) -> Result<()> {
		self.spaces.remove(&id);
		Ok(())
	}
	async fn clear_contents(&mut self, id: Uuid) -> Result<()> {
		self.cleared.push(id);
		self.spaces.get_mut(&id).unwrap().wipe_generation += 1;
		Ok(())
	}
	async fn subnode_creator_run(&mut self, workspace: Uuid) -> Result<Option<Run>> {
		Ok(self
			.creator
			.as_ref()
			.and_then(|(id, run)| (*id == workspace).then(|| run.clone())))
	}
	async fn run_binding(&mut self, run: &Run) -> Result<(Uuid, String)> {
		self.bound_run = Some(run.id);
		self.binding
			.clone()
			.ok_or_else(|| Error::NotFound("binding".into()))
	}
}

#[tokio::test]
async fn reads_do_not_lock_but_settings_updates_do() {
	let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
	let mut scope = Memory {
		person: "alice".into(),
		..Default::default()
	};
	let space = space(now);
	create(&mut scope, &space).await.unwrap();
	read(&mut scope, space.id, now).await.unwrap();
	assert_eq!((scope.read_loads, scope.locked_loads), (1, 0));
	update_settings(&mut scope, space.id, now, 0, space.settings)
		.await
		.unwrap();
	assert_eq!((scope.read_loads, scope.locked_loads), (1, 1));
}

fn space(now: DateTime<Utc>) -> ScratchSpace {
	ScratchSpace {
		id: Uuid::new_v4(),
		node: "brave-otter-k3m9q2xa".parse().unwrap(),
		name: "Project".into(),
		creator: "alice".into(),
		members: BTreeSet::new(),
		state: ScratchState::Active,
		revision: 0,
		wipe_generation: 0,
		settings: ScratchSettings {
			allowed_hosts: HostAllowlist::Hosts(BTreeSet::new()),
			..ScratchSettings::default()
		},
		created_at: now,
		last_used_at: now,
	}
}

#[tokio::test]
async fn membership_and_policy_bound_settings_control_mutations() {
	let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
	let mut scope = Memory {
		person: "alice".into(),
		..Default::default()
	};
	let space = space(now);
	let id = space.id;
	scope.person = "mallory".into();
	assert!(create(&mut scope, &space).await.is_err());
	scope.person = "alice".into();
	create(&mut scope, &space).await.unwrap();
	scope.deny_node = true;
	assert!(matches!(
		read(&mut scope, id, now).await,
		Err(Error::Forbidden)
	));
	scope.deny_node = false;
	scope.person = "bob".into();
	assert!(matches!(
		read(&mut scope, id, now).await,
		Err(Error::Forbidden)
	));
	scope.person = "alice".into();
	assert!(matches!(
		add_member(&mut scope, id, now, "outsider").await,
		Err(Error::Forbidden)
	));
	add_member(&mut scope, id, now, "bob").await.unwrap();
	scope.person = "bob".into();
	let mut approved = space.settings.clone();
	approved.allowed_hosts = HostAllowlist::Hosts(BTreeSet::from(["example.com".into()]));
	approved.external_services.insert("approved".into());
	update_settings(&mut scope, id, now, 0, approved)
		.await
		.unwrap();
	assert_eq!(
		scope.spaces[&id].settings.external_services,
		BTreeSet::from(["approved".into()])
	);
	let mut settings = space.settings.clone();
	settings.allowed_hosts = HostAllowlist::Hosts(BTreeSet::from(["outside.example".into()]));
	assert!(
		update_settings(&mut scope, id, now, 1, settings)
			.await
			.is_err()
	);
	let mut settings = space.settings.clone();
	settings.external_services.insert("unapproved".into());
	assert!(matches!(
		update_settings(&mut scope, id, now, 1, settings).await,
		Err(Error::Forbidden)
	));
	assert_eq!(
		scope.spaces[&id].settings.external_services,
		BTreeSet::from(["approved".into()])
	);
	scope.person = "alice".into();
	remove_member(&mut scope, id, now, "bob").await.unwrap();
	scope.person = "bob".into();
	assert!(matches!(
		read(&mut scope, id, now).await,
		Err(Error::Forbidden)
	));
	scope.person = "alice".into();
	clear(&mut scope, id, now).await.unwrap();
	assert_eq!(scope.cleared, vec![id]);
	assert_eq!(scope.spaces[&id].wipe_generation, 1);
	assert!(read(&mut scope, id, now).await.is_ok());
}

#[tokio::test]
async fn expired_space_can_be_cleared_or_deleted_by_a_member() {
	let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
	let mut scope = Memory {
		person: "alice".into(),
		..Default::default()
	};
	let space = space(now);
	let id = space.id;
	create(&mut scope, &space).await.unwrap();
	add_member(&mut scope, id, now, "bob").await.unwrap();
	scope.person = "bob".into();
	let expired = now + Duration::days(15);
	clear(&mut scope, id, expired).await.unwrap();
	assert!(read(&mut scope, id, expired).await.is_ok());
	assert_eq!(scope.cleared, vec![id]);
	delete(&mut scope, id, expired + Duration::days(15))
		.await
		.unwrap();
	assert!(
		read(&mut scope, id, expired + Duration::days(15))
			.await
			.is_err()
	);
	assert_eq!(scope.cleared, vec![id, id]);
}

#[tokio::test]
async fn stale_settings_revision_does_not_overwrite_another_members_edit() {
	let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
	let mut scope = Memory {
		person: "alice".into(),
		..Default::default()
	};
	let space = space(now);
	create(&mut scope, &space).await.unwrap();
	let mut first = space.settings.clone();
	first.idle_expiry_days = Some(30);
	update_settings(&mut scope, space.id, now, 0, first)
		.await
		.unwrap();
	assert!(matches!(
		update_settings(&mut scope, space.id, now, 0, space.settings).await,
		Err(Error::Conflict(_))
	));
	assert_eq!(scope.spaces[&space.id].settings.idle_expiry_days, Some(30));
}

#[tokio::test]
async fn run_requires_starter_authority_and_receiving_node() {
	let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
	let mut scope = Memory {
		person: "alice".into(),
		..Default::default()
	};
	let space = space(now);
	let id = space.id;
	create(&mut scope, &space).await.unwrap();
	let mut run =
		crate::test_support::run_of(crate::test_support::snapshot("aidash://local", "agent"));
	scope.person = "agent:worker".into();
	scope.binding = Some((id, "mallory".into()));
	assert!(matches!(
		scratch_space_for_run(&mut scope, &run, now).await,
		Err(Error::Forbidden)
	));
	scope.binding = Some((id, "alice".into()));
	run.node = NodeName::derived("remote");
	assert!(matches!(
		scratch_space_for_run(&mut scope, &run, now).await,
		Err(Error::Forbidden)
	));
	run.node = space.node;
	assert_eq!(
		scratch_space_for_run(&mut scope, &run, now + Duration::days(1))
			.await
			.unwrap(),
		id
	);
	assert_eq!(scope.spaces[&id].last_used_at, now + Duration::days(1));
	assert_eq!(scope.node_people.last().map(String::as_str), Some("alice"));
	// A later operator revocation is checked again before the next Run.
	scope.spaces.get_mut(&id).unwrap().settings.docker_enabled = true;
	assert!(
		scratch_space_for_run(&mut scope, &run, now + Duration::days(1))
			.await
			.is_err()
	);
}

#[tokio::test]
async fn removing_a_conversation_starter_revokes_their_runs() {
	let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
	let mut scope = Memory {
		person: "alice".into(),
		..Default::default()
	};
	let space = space(now);
	create(&mut scope, &space).await.unwrap();
	add_member(&mut scope, space.id, now, "bob").await.unwrap();
	let mut run =
		crate::test_support::run_of(crate::test_support::snapshot("aidash://local", "agent"));
	run.node = space.node.clone();
	scope.binding = Some((space.id, "bob".into()));
	assert_eq!(
		scratch_space_for_run(&mut scope, &run, now).await.unwrap(),
		space.id
	);
	remove_member(&mut scope, space.id, now, "bob")
		.await
		.unwrap();
	assert!(matches!(
		scratch_space_for_run(&mut scope, &run, now).await,
		Err(Error::Forbidden)
	));
}

#[tokio::test]
async fn listing_shows_only_spaces_available_to_the_person_on_that_node() {
	let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
	let mut scope = Memory {
		person: "alice".into(),
		..Default::default()
	};
	let shared = space(now);
	let mut private = space(now);
	private.name = "Private".into();
	let mut remote = space(now);
	remote.node = NodeName::derived("remote");
	create(&mut scope, &shared).await.unwrap();
	create(&mut scope, &private).await.unwrap();
	create(&mut scope, &remote).await.unwrap();
	add_member(&mut scope, shared.id, now, "bob").await.unwrap();
	add_member(&mut scope, remote.id, now, "bob").await.unwrap();
	scope.person = "bob".into();
	let visible = list(&mut scope, &shared.node, now).await.unwrap();
	assert_eq!(
		visible.iter().map(|value| value.id).collect::<Vec<_>>(),
		vec![shared.id]
	);
}

#[tokio::test]
async fn subnode_run_uses_the_creator_runs_scratch_space() {
	let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
	let mut scope = Memory {
		person: "alice".into(),
		..Default::default()
	};
	let space = space(now);
	create(&mut scope, &space).await.unwrap();
	let creator =
		crate::test_support::run_of(crate::test_support::snapshot("aidash://local", "agent"));
	let mut child = creator.clone();
	child.id = Uuid::new_v4();
	child.workspace_id = Uuid::new_v4();
	scope.creator = Some((child.workspace_id, creator.clone()));
	scope.binding = Some((space.id, "alice".into()));
	assert_eq!(
		scratch_space_for_run(&mut scope, &child, now)
			.await
			.unwrap(),
		space.id
	);
	assert_eq!(scope.bound_run, Some(creator.id));
}

#[tokio::test]
async fn conversation_selects_an_accessible_space_or_creates_a_dedicated_one() {
	let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
	let mut scope = Memory {
		person: "alice".into(),
		..Default::default()
	};
	let node = NodeName::derived("conversation");
	let id = select_for_workspace(&mut scope, &node, "New conversation", None, now)
		.await
		.unwrap();
	assert_eq!(scope.spaces[&id].creator, "alice");
	assert_eq!(scope.spaces[&id].node, node);
	assert_eq!(
		scope.spaces[&id].settings.allowed_hosts,
		HostAllowlist::Hosts(BTreeSet::new())
	);
	assert_eq!(
		select_for_workspace(
			&mut scope,
			&node,
			"Other",
			Some(id),
			now + Duration::days(1)
		)
		.await
		.unwrap(),
		id
	);
	assert_eq!(scope.spaces[&id].last_used_at, now + Duration::days(1));
	let other_node = NodeName::derived("other");
	assert!(matches!(
		select_for_workspace(&mut scope, &other_node, "Other", Some(id), now).await,
		Err(Error::Forbidden)
	));
	scope.person = "bob".into();
	assert!(matches!(
		select_for_workspace(&mut scope, &node, "Other", Some(id), now).await,
		Err(Error::Forbidden)
	));
}

#[tokio::test]
async fn agents_cannot_create_a_space_as_the_owner() {
	let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
	let mut scope = Memory {
		person: "agent:worker".into(),
		..Default::default()
	};
	let mut space = space(now);
	space.creator = scope.person.clone();
	assert!(matches!(
		create(&mut scope, &space).await,
		Err(Error::Forbidden)
	));
	assert!(scope.spaces.is_empty());
}

#[tokio::test]
async fn members_can_use_services_already_shared_with_the_space() {
	let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
	let mut scope = Memory {
		person: "alice".into(),
		..Default::default()
	};
	let mut space = space(now);
	space.settings.external_services.insert("approved".into());
	create(&mut scope, &space).await.unwrap();
	add_member(&mut scope, space.id, now, "bob").await.unwrap();
	scope.person = "bob".into();
	scope.deny_references = true;
	assert_eq!(
		select_for_workspace(&mut scope, &space.node, "reuse", Some(space.id), now)
			.await
			.unwrap(),
		space.id
	);
	let mut run =
		crate::test_support::run_of(crate::test_support::snapshot("aidash://local", "agent"));
	run.node = space.node.clone();
	scope.binding = Some((space.id, "bob".into()));
	assert_eq!(
		scratch_space_for_run(&mut scope, &run, now).await.unwrap(),
		space.id
	);
	update_settings(&mut scope, space.id, now, 0, space.settings.clone())
		.await
		.unwrap();
	let mut expanded = space.settings;
	expanded.external_services.insert("another-service".into());
	assert!(matches!(
		update_settings(&mut scope, space.id, now, 1, expanded).await,
		Err(Error::Forbidden)
	));
}

#[tokio::test]
async fn a_workspace_gets_its_dedicated_space_without_scratch_create() {
	let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
	let node: NodeName = "brave-otter-k3m9q2xa".parse().unwrap();
	let mut scope = Memory {
		person: "alice".into(),
		deny_create: true,
		..Default::default()
	};
	let id = select_for_workspace(&mut scope, &node, "Project", None, now)
		.await
		.unwrap();
	assert_eq!(scope.spaces[&id].creator, "alice");
	assert!(matches!(
		create(&mut scope, &space(now)).await,
		Err(Error::Forbidden)
	));
}
