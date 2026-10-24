use aidash_domain::nodes::{DeploymentKeyId, keys::*};
use chrono::{Duration, TimeZone, Utc};

fn key(byte: u8) -> PublicDeploymentKey {
	PublicDeploymentKey::new(DeploymentKeyId::of_public_key(&[byte; 32]), [byte; 32]).unwrap()
}

#[test]
fn exact_schedule_and_receiver_overlap_boundaries() {
	let published = Utc.timestamp_opt(1_000, 0).unwrap();
	let schedule = RolloverSchedule::new(published).unwrap();
	assert_eq!(schedule.activates_at, published + Duration::hours(24));
	assert_eq!(schedule.retires_at, published + Duration::days(8));
	for (now, accepted) in [
		(schedule.activates_at - Duration::seconds(1), false),
		(schedule.activates_at, true),
		(schedule.retires_at - Duration::seconds(1), true),
		(schedule.retires_at, false),
	] {
		let mut trust = TrustedDeploymentKeys::new(key(1)).unwrap();
		assert_eq!(
			trust
				.accept_successor(&key(1).key_id, key(2), schedule.clone(), now)
				.is_ok(),
			accepted
		);
		if accepted {
			assert_eq!(trust.current, key(2));
			assert!(
				trust
					.trusted_key(&key(1).key_id, schedule.retires_at - Duration::seconds(1))
					.is_some()
			);
			assert!(
				trust
					.trusted_key(&key(1).key_id, schedule.retires_at)
					.is_none()
			);
		}
	}
}

#[test]
fn trust_is_monotonic_and_quarantine_requires_manual_repin() {
	let schedule = RolloverSchedule::new(Utc.timestamp_opt(1_000, 0).unwrap()).unwrap();
	let mut trust = TrustedDeploymentKeys::new(key(1)).unwrap();
	trust
		.accept_successor(
			&key(1).key_id,
			key(2),
			schedule.clone(),
			schedule.activates_at,
		)
		.unwrap();
	let accepted = trust.clone();
	assert!(
		trust
			.accept_successor(
				&key(1).key_id,
				key(3),
				schedule.clone(),
				schedule.activates_at
			)
			.is_err()
	);
	assert!(
		trust
			.accept_successor(
				&key(2).key_id,
				key(3),
				schedule.clone(),
				schedule.activates_at
			)
			.is_err()
	);
	let next = RolloverSchedule::new(schedule.retires_at).unwrap();
	assert!(
		trust
			.accept_successor(&key(2).key_id, key(1), next.clone(), next.activates_at)
			.is_err()
	);
	assert_eq!(trust, accepted);
	let mut next_trust = trust.clone();
	next_trust
		.accept_successor(&key(2).key_id, key(3), next.clone(), next.activates_at)
		.unwrap();
	assert_eq!(next_trust.current, key(3));
	assert!(
		next_trust
			.trusted_key(&key(1).key_id, next.activates_at)
			.is_none()
	);
	trust.quarantine();
	assert!(
		trust
			.trusted_key(&key(2).key_id, next.activates_at)
			.is_none()
	);
	assert!(
		trust
			.accept_successor(&key(2).key_id, key(3), next.clone(), next.activates_at)
			.is_err()
	);
	trust.manual_repin(key(3)).unwrap();
	assert!(!trust.quarantined);
	assert_eq!(trust.current, key(3));
	assert!(trust.retiring.is_none());
}

#[test]
fn invalid_digest_and_schedule_cannot_advance_trust() {
	assert!(PublicDeploymentKey::new(key(1).key_id, [2; 32]).is_err());
	let mut schedule = RolloverSchedule::new(Utc.timestamp_opt(1_000, 0).unwrap()).unwrap();
	schedule.activates_at -= Duration::seconds(1);
	let mut trust = TrustedDeploymentKeys::new(key(1)).unwrap();
	assert!(
		trust
			.accept_successor(
				&key(1).key_id,
				key(2),
				schedule.clone(),
				schedule.activates_at
			)
			.is_err()
	);
	assert_eq!(trust.current, key(1));
}

#[test]
fn registration_refresh_preserves_authorization_and_cannot_cross_deployments() {
	use aidash_domain::{
		nodes::{DeploymentId, NodeAddress, NodeName, registration::*},
		tenancy::TenantName,
	};
	let published = Utc.timestamp_opt(1_000, 0).unwrap();
	let schedule = RolloverSchedule::new(published).unwrap();
	let registration = NodeRegistration {
		tenant: TenantName::new("tenant").unwrap(),
		node: NodeName::derived("owner"),
		source_node: NodeAddress::derived("aidash://sender", "source"),
		counterpart: Counterpart::OtherDeployment {
			endpoint: "https://peer.example".into(),
			key: PinnedKey {
				key_id: key(1).key_id,
				public_key_pem: "adapter-owned-pem".into(),
			},
		},
		atomic_transactions: false,
		enabled: false,
		revision: 8,
		actor: "admin".into(),
		created_at: published,
		updated_at: published,
	};
	let before = registration.clone();
	let mut trust = TrustedDeploymentKeys::new(key(1)).unwrap();
	assert!(
		registration
			.accept_key_transition(
				&mut trust,
				&DeploymentId::parse("aidash://other").unwrap(),
				&key(1).key_id,
				key(2),
				schedule.clone(),
				schedule.activates_at
			)
			.is_err()
	);
	assert_eq!(trust.current, key(1));
	registration
		.accept_key_transition(
			&mut trust,
			&DeploymentId::parse("aidash://sender").unwrap(),
			&key(1).key_id,
			key(2),
			schedule.clone(),
			schedule.activates_at,
		)
		.unwrap();
	assert_eq!(trust.current, key(2));
	assert_eq!(registration, before);
}
