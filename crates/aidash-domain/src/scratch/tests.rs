use super::*;
use std::collections::BTreeSet;

#[test]
fn settings_cannot_expand_tenant_hosts_or_enable_disallowed_docker() {
	let ceiling = ScratchCeilings::default();
	let tenant = HostAllowlist::Hosts(BTreeSet::from(["example.com".to_owned()]));
	let mut settings = ScratchSettings::default();
	settings.allowed_hosts = HostAllowlist::Hosts(BTreeSet::from(["other.com".to_owned()]));
	assert!(settings.validate(&ceiling, &tenant).is_err());
	settings.allowed_hosts = tenant.clone();
	settings.docker_enabled = true;
	assert!(settings.validate(&ceiling, &tenant).is_err());
	settings.docker_enabled = false;
	assert!(settings.validate(&ceiling, &tenant).is_ok());
}

#[test]
fn membership_and_expiry_follow_conversation_person_and_last_use() {
	use chrono::{Duration, TimeZone, Utc};
	let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
	let mut space = ScratchSpace {
		id: uuid::Uuid::nil(),
		node: crate::nodes::NodeName::derived("scratch-test"),
		name: "Example".into(),
		creator: "alice".into(),
		members: BTreeSet::from(["bob".into()]),
		state: ScratchState::Active,
		settings: ScratchSettings::default(),
		created_at: now,
		last_used_at: now,
	};
	assert!(space.agent_may_use("bob"));
	assert!(!space.agent_may_use("eve"));
	assert!(!space.is_expired(now + Duration::days(13)));
	assert!(space.is_expired(now + Duration::days(14)));
	space.settings.idle_expiry_days = None;
	assert!(!space.is_expired(now + Duration::days(100)));
	space.clear();
	assert!(!space.may_use("alice"));
}
