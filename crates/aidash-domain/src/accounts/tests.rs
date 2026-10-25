use super::*;
use chrono::{Duration, TimeZone};
use rstest::{fixture, rstest};

#[fixture]
fn now() -> DateTime<Utc> {
	Utc.timestamp_opt(1_800_000_000, 0).unwrap()
}

#[fixture]
fn identity(now: DateTime<Utc>) -> ExternalIdentity {
	ExternalIdentity {
		id: Uuid::from_u128(1),
		issuer: "pool/enterprise".into(),
		subject: "human".into(),
		first_account: Some(Uuid::from_u128(2)),
		linked: true,
		status: IdentityStatus::Enabled,
		disable_epoch: 0,
		last_valid_at: Some(now),
	}
}

// Protects source and historical Account provenance even after login unlinking.
#[rstest]
fn unlink_keeps_durable_origin_and_cannot_transfer(
	mut identity: ExternalIdentity,
	now: DateTime<Utc>,
) {
	identity.linked = false;
	let account = Account {
		id: Uuid::from_u128(2),
		status: AccountStatus::Active,
		session_epoch: 4,
		suspension_epoch: 0,
	};
	let origin = DurableOrigin {
		source_disable_epoch: None,
		suspension_epoch: None,
		identity_id: identity.id,
		account_id: None,
	};
	assert_eq!(
		durable_validity(&origin, Some(&account), &identity, now),
		Validity::Active
	);
	assert_eq!(
		identity.attach(Uuid::from_u128(3)),
		Err(AccountError::AlreadyAssigned)
	);
	assert!(!identity.linked);
	identity.attach(account.id).unwrap();
	assert!(identity.linked);
}

// A foreign/current-login Identity may never replace the admitted source.
#[rstest]
fn durable_checks_reject_provenance_substitution(identity: ExternalIdentity, now: DateTime<Utc>) {
	let origin = DurableOrigin {
		source_disable_epoch: None,
		suspension_epoch: None,
		identity_id: Uuid::from_u128(99),
		account_id: identity.first_account,
	};
	assert_eq!(
		durable_validity(&origin, None, &identity, now),
		Validity::Denied(Denial::ProvenanceMismatch)
	);
}

// A legacy unassigned origin must be checkable without allocating an Account.
#[rstest]
fn never_assigned_legacy_work_needs_no_account(mut identity: ExternalIdentity, now: DateTime<Utc>) {
	identity.first_account = None;
	identity.linked = false;
	assert_eq!(
		durable_validity(
			&DurableOrigin {
				source_disable_epoch: None,
				suspension_epoch: None,
				identity_id: identity.id,
				account_id: None
			},
			None,
			&identity,
			now
		),
		Validity::Active
	);
}

// Explicit stops override a positive upstream observation; stale status pauses.
#[rstest]
#[case(IdentityStatus::Enabled, 899, Validity::Active)]
#[case(IdentityStatus::Enabled, 900, Validity::Unavailable)]
#[case(
	IdentityStatus::Disabled,
	0,
	Validity::Denied(Denial::IdentityDisabled)
)]
fn source_status_boundary(
	mut identity: ExternalIdentity,
	now: DateTime<Utc>,
	#[case] status: IdentityStatus,
	#[case] age: i64,
	#[case] expected: Validity,
) {
	identity.first_account = None;
	identity.status = status;
	identity.last_valid_at = Some(now - Duration::seconds(age));
	assert_eq!(
		durable_validity(
			&DurableOrigin {
				source_disable_epoch: None,
				suspension_epoch: None,
				identity_id: identity.id,
				account_id: None
			},
			None,
			&identity,
			now
		),
		expected
	);
}

// A restore or unlink must not resurrect a previously issued interactive session.
#[rstest]
#[case(
	false,
	AccountStatus::Active,
	4,
	4,
	Validity::Denied(Denial::SessionInvalid)
)]
#[case(
	true,
	AccountStatus::Suspended,
	4,
	4,
	Validity::Denied(Denial::AccountSuspended)
)]
#[case(
	true,
	AccountStatus::Active,
	5,
	4,
	Validity::Denied(Denial::SessionInvalid)
)]
#[case(true, AccountStatus::Active, 4, 4, Validity::Active)]
fn interactive_session_uses_source_and_epoch(
	mut identity: ExternalIdentity,
	now: DateTime<Utc>,
	#[case] linked: bool,
	#[case] status: AccountStatus,
	#[case] epoch: u64,
	#[case] issued_epoch: u64,
	#[case] expected: Validity,
) {
	identity.linked = linked;
	let account = Account {
		id: Uuid::from_u128(2),
		status,
		session_epoch: epoch,
		suspension_epoch: 0,
	};
	let session = ProductSession {
		id: Uuid::from_u128(5),
		account_id: account.id,
		identity_id: identity.id,
		client_id: "cloud".into(),
		device_id: "browser".into(),
		issued_at: now,
		last_seen_at: now,
		absolute_expires_at: now + Duration::hours(12),
		idle_seconds: 1800,
		account_epoch: issued_epoch,
		revoked_at: None,
	};
	assert_eq!(
		interactive_validity(&session, &account, &identity, now),
		expected
	);
}

// A source stop followed by restore while disconnected cannot auto-resume an
// old origin; only an explicitly refreshed original binding can proceed.
#[rstest]
#[case(None, Validity::Denied(Denial::IdentityDisabled))]
#[case(Some(0), Validity::Denied(Denial::IdentityDisabled))]
#[case(Some(1), Validity::Active)]
fn restored_source_retains_its_stop_epoch(
	mut identity: ExternalIdentity,
	now: DateTime<Utc>,
	#[case] captured: Option<u64>,
	#[case] expected: Validity,
) {
	identity.first_account = None;
	identity.disable_epoch = 1;
	assert_eq!(
		durable_validity(
			&DurableOrigin {
				identity_id: identity.id,
				account_id: None,
				suspension_epoch: None,
				source_disable_epoch: captured
			},
			None,
			&identity,
			now
		),
		expected
	);
}
