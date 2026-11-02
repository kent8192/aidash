use super::*;
use chrono::{Duration, TimeZone};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Default)]
struct Memory {
	person: String,
	spaces: BTreeMap<Uuid, ScratchSpace>,
	binding: Option<(Uuid, String)>,
	cleared: Vec<Uuid>,
}

#[async_trait]
impl ScratchScope for Memory {
	fn person(&self) -> &str {
		&self.person
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
	async fn run_binding(&mut self, _: &Run) -> Result<(Uuid, String)> {
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
	assert!(scope.spaces[&id].settings.external_services.is_empty());
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
}
