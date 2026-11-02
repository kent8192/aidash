use super::*;
use chrono::{Duration, TimeZone};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Default)]
struct Memory {
	person: String,
	deny_node: bool,
	node_people: Vec<String>,
	spaces: BTreeMap<Uuid, ScratchSpace>,
	binding: Option<(Uuid, String)>,
	creator: Option<(Uuid, Run)>,
	bound_run: Option<Uuid>,
	cleared: Vec<Uuid>,
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
		Ok(())
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
		if settings.external_services.iter().any(|id| id != "approved") {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	async fn validate_members(&mut self, members: &BTreeSet<String>) -> Result<()> {
		if members.iter().any(|member| member != "bob") {
			return Err(Error::Forbidden);
		}
		Ok(())
	}
	async fn insert(&mut self, space: &ScratchSpace) -> Result<()> {
		self.spaces.insert(space.id, space.clone());
		Ok(())
	}
	async fn load_for_update(&mut self, id: Uuid) -> Result<ScratchSpace> {
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

fn space(now: DateTime<Utc>) -> ScratchSpace {
	ScratchSpace {
		id: Uuid::new_v4(),
		node: "brave-otter-k3m9q2xa".parse().unwrap(),
		name: "Project".into(),
		creator: "alice".into(),
		members: BTreeSet::new(),
		state: ScratchState::Active,
		settings: ScratchSettings::default(),
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
	update_settings(&mut scope, id, now, approved)
		.await
		.unwrap();
	assert_eq!(
		scope.spaces[&id].settings.external_services,
		BTreeSet::from(["approved".into()])
	);
	let mut settings = space.settings.clone();
	settings.allowed_hosts = HostAllowlist::Hosts(BTreeSet::from(["outside.example".into()]));
	assert!(
		update_settings(&mut scope, id, now, settings)
			.await
			.is_err()
	);
	let mut settings = space.settings.clone();
	settings.external_services.insert("unapproved".into());
	assert!(matches!(
		update_settings(&mut scope, id, now, settings).await,
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
	assert!(matches!(
		read(&mut scope, id, now).await,
		Err(Error::Forbidden)
	));
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
