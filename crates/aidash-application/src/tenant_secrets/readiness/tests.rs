//! Readiness classification: every status and repair, and the matching rules.
use super::{Availability, Fact, Kind, Repair, Status, Target, facts};
use crate::tenant_secrets::Metadata;
use chrono::Utc;
use uuid::Uuid;

fn secret(name: &str, hosts: &[&str]) -> Metadata {
	Metadata {
		owner: crate::tenant_secrets::Ownership::Tenant,
		id: Uuid::now_v7(),
		tenant: "acme".into(),
		name: name.into(),
		hosts: hosts.iter().map(|host| host.to_string()).collect(),
		fingerprint: "f".repeat(16),
		last4: "1234".into(),
		created_at: Utc::now(),
		updated_at: Utc::now(),
		revision: 2,
	}
}

fn target(name: Option<&str>, endpoint: Option<&str>) -> Target {
	Target {
		item: "crm@1.0.0".into(),
		kind: Kind::Tool,
		tenant_secret_name: name.map(str::to_owned),
		endpoint: endpoint.map(str::to_owned),
	}
}

fn one(availability: Availability, can_manage: bool, target: Target, secrets: &[Metadata]) -> Fact {
	facts(availability, can_manage, [target], secrets)
		.pop()
		.unwrap()
}

#[test]
fn a_tool_that_names_no_secret_is_ready() {
	let fact = one(
		Availability::Available,
		false,
		target(None, None),
		&[secret("crm-key", &["api.example.test"])],
	);
	assert_eq!(fact.status, Status::Ready);
	assert_eq!(fact.repair, Repair::None);
	assert_eq!(fact.tenant_secret_name, None);
}

#[test]
fn a_missing_or_pending_named_secret_is_secret_missing() {
	for can_manage in [true, false] {
		let fact = one(
			Availability::Available,
			can_manage,
			target(Some("crm-key"), Some("https://api.example.test/v1/call")),
			&[],
		);
		assert_eq!(fact.status, Status::SecretMissing);
		assert_eq!(
			fact.repair,
			if can_manage {
				Repair::ManageSecret
			} else {
				Repair::AskAdministrator
			}
		);
		assert_eq!(fact.tenant_secret_name.as_deref(), Some("crm-key"));
	}
}

#[test]
fn a_secret_that_does_not_record_the_host_is_secret_hosts_mismatch() {
	let secrets = [secret("crm-key", &["other.example.test"])];
	for can_manage in [true, false] {
		let fact = one(
			Availability::Available,
			can_manage,
			target(Some("crm-key"), Some("https://api.example.test/v1/call")),
			&secrets,
		);
		assert_eq!(fact.status, Status::SecretHostsMismatch);
		assert_eq!(
			fact.repair,
			if can_manage {
				Repair::ManageSecret
			} else {
				Repair::AskAdministrator
			}
		);
	}
}

#[test]
fn a_secret_that_records_the_host_is_ready() {
	let secrets = [secret("crm-key", &["api.example.test"])];
	let fact = one(
		Availability::Available,
		false,
		target(Some("crm-key"), Some("https://api.example.test/v1/call")),
		&secrets,
	);
	assert_eq!(fact.status, Status::Ready);
	assert_eq!(fact.repair, Repair::None);
}

#[test]
fn deployment_wide_problems_are_deployment_administrator_repairs() {
	let secrets = [secret("crm-key", &["api.example.test"])];
	for (availability, status) in [
		(
			Availability::Unavailable,
			Status::SecretsUnavailableOnDeployment,
		),
		(
			Availability::BrokerNotConfigured,
			Status::BrokerNotConfigured,
		),
	] {
		for can_manage in [true, false] {
			let fact = one(
				availability,
				can_manage,
				target(Some("crm-key"), Some("https://api.example.test/")),
				&secrets,
			);
			assert_eq!(fact.status, status);
			assert_eq!(fact.repair, Repair::DeploymentAdministrator);
		}
	}
}

#[test]
fn a_slot_is_ready_when_any_active_secret_records_the_package_host() {
	let slot = Target {
		item: "search-key".into(),
		kind: Kind::Slot,
		tenant_secret_name: None,
		endpoint: Some("https://api.example.test/v1/".into()),
	};
	for can_manage in [true, false] {
		let missing = one(Availability::Available, can_manage, slot.clone(), &[]);
		assert_eq!(missing.status, Status::SecretMissing);
		assert_eq!(
			missing.repair,
			if can_manage {
				Repair::ManageSecret
			} else {
				Repair::AskAdministrator
			}
		);
		let mismatch = one(
			Availability::Available,
			can_manage,
			slot.clone(),
			&[secret("other-key", &["elsewhere.example.test"])],
		);
		assert_eq!(mismatch.status, Status::SecretHostsMismatch);
		let ready = one(
			Availability::Available,
			can_manage,
			slot.clone(),
			&[secret("crm-key", &["api.example.test"])],
		);
		assert_eq!(ready.status, Status::Ready);
		assert_eq!(ready.repair, Repair::None);
		assert_eq!(ready.tenant_secret_name, None);
	}
}

#[test]
fn facts_keep_the_requested_order_and_kind() {
	let targets = [
		target(None, None),
		Target {
			item: "search-key".into(),
			kind: Kind::Slot,
			tenant_secret_name: None,
			endpoint: Some("https://api.example.test/".into()),
		},
	];
	let facts = facts(
		Availability::Available,
		false,
		targets,
		&[secret("crm-key", &["api.example.test"])],
	);
	assert_eq!(facts.len(), 2);
	assert_eq!(facts[0].item, "crm@1.0.0");
	assert_eq!(facts[0].kind, Kind::Tool);
	assert_eq!(facts[1].item, "search-key");
	assert_eq!(facts[1].kind, Kind::Slot);
}
