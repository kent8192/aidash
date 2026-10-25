//! Lifecycle tests own authentication rules; the transactional store only stores
//! records and commits/rolls back. It does not implement any asserted policy.
use super::*;
use async_trait::async_trait;
use chrono::{Duration, TimeZone};
use rstest::fixture;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, OwnedMutexGuard};

#[derive(Clone, Default)]
struct State {
	accounts: HashMap<Uuid, Account>,
	identities: HashMap<Uuid, ExternalIdentity>,
	sessions: HashMap<Uuid, ProductSession>,
	proofs: HashMap<Uuid, FreshProof>,
}
#[derive(Clone, Default)]
struct Memory(Arc<Mutex<State>>);
struct Transaction {
	guard: OwnedMutexGuard<State>,
	pending: State,
}
#[async_trait]
impl Repository for Memory {
	async fn begin(&self) -> crate::Result<Box<dyn Scope>> {
		let guard = self.0.clone().lock_owned().await;
		let pending = guard.clone();
		Ok(Box::new(Transaction { guard, pending }))
	}
}
#[async_trait]
impl Scope for Transaction {
	async fn account(&mut self, id: Uuid) -> crate::Result<Option<Account>> {
		Ok(self.pending.accounts.get(&id).cloned())
	}
	async fn identity(&mut self, id: Uuid) -> crate::Result<Option<ExternalIdentity>> {
		Ok(self.pending.identities.get(&id).cloned())
	}
	async fn session(&mut self, id: Uuid) -> crate::Result<Option<ProductSession>> {
		Ok(self.pending.sessions.get(&id).cloned())
	}
	async fn save_account(&mut self, value: &Account) -> crate::Result<()> {
		self.pending.accounts.insert(value.id, value.clone());
		Ok(())
	}
	async fn save_identity(&mut self, value: &ExternalIdentity) -> crate::Result<()> {
		self.pending.identities.insert(value.id, value.clone());
		Ok(())
	}
	async fn save_session(&mut self, value: &ProductSession) -> crate::Result<()> {
		self.pending.sessions.insert(value.id, value.clone());
		Ok(())
	}
	async fn consume_proof(&mut self, id: Uuid) -> crate::Result<Option<FreshProof>> {
		Ok(self.pending.proofs.remove(&id))
	}
	async fn revoke_identity_sessions(
		&mut self,
		id: Uuid,
		now: DateTime<Utc>,
	) -> crate::Result<()> {
		for session in self
			.pending
			.sessions
			.values_mut()
			.filter(|session| session.identity_id == id)
		{
			session.revoked_at = Some(now);
		}
		Ok(())
	}
	async fn revoke_account_sessions(&mut self, id: Uuid, now: DateTime<Utc>) -> crate::Result<()> {
		for session in self
			.pending
			.sessions
			.values_mut()
			.filter(|session| session.account_id == id)
		{
			session.revoked_at = Some(now);
		}
		Ok(())
	}
	async fn commit(mut self: Box<Self>) -> crate::Result<()> {
		*self.guard = self.pending.clone();
		Ok(())
	}
}
fn id(value: u128) -> Uuid {
	Uuid::from_u128(value)
}
#[fixture]
fn now() -> DateTime<Utc> {
	Utc.timestamp_opt(1_800_000_000, 0).unwrap()
}
struct Fixture {
	store: Memory,
	now: DateTime<Utc>,
}
impl Fixture {
	fn service(&self) -> Service<Memory> {
		Service::new(self.store.clone())
	}
	async fn proof(&self, proof: u128, identity: u128) {
		self.store.0.lock().await.proofs.insert(
			id(proof),
			FreshProof {
				id: id(proof),
				identity_id: id(identity),
				authenticated_at: self.now,
				expires_at: self.now + Duration::minutes(3),
			},
		);
	}
	async fn session(&self, number: u128, identity: u128) {
		self.store.0.lock().await.sessions.insert(
			id(number),
			ProductSession {
				id: id(number),
				account_id: id(10),
				identity_id: id(identity),
				client_id: "cloud".into(),
				device_id: number.to_string(),
				issued_at: self.now,
				last_seen_at: self.now,
				absolute_expires_at: self.now + Duration::hours(12),
				idle_seconds: 1800,
				account_epoch: 0,
				revoked_at: None,
			},
		);
	}
}
#[fixture]
fn fixture(now: DateTime<Utc>) -> Fixture {
	let mut state = State::default();
	state.accounts.insert(
		id(10),
		Account {
			id: id(10),
			status: AccountStatus::Active,
			session_epoch: 0,
			suspension_epoch: 0,
		},
	);
	state.accounts.insert(
		id(20),
		Account {
			id: id(20),
			status: AccountStatus::Active,
			session_epoch: 0,
			suspension_epoch: 0,
		},
	);
	for (number, first_account, linked) in [
		(1, Some(id(10)), true),
		(2, None, false),
		(3, Some(id(20)), true),
	] {
		state.identities.insert(
			id(number),
			ExternalIdentity {
				id: id(number),
				issuer: format!("pool/{number}"),
				subject: "same-human".into(),
				first_account,
				linked,
				status: IdentityStatus::Enabled,
				disable_epoch: 0,
				last_valid_at: Some(now),
			},
		);
	}
	Fixture {
		store: Memory(Arc::new(Mutex::new(state))),
		now,
	}
}

// Catches accidental reassignment after unlink and omission of session revoke.
#[rstest::rstest]
#[tokio::test]
async fn link_unlink_retains_history_and_source_session_scope(fixture: Fixture) {
	let f = fixture;
	f.proof(100, 1).await;
	f.proof(101, 2).await;
	f.service()
		.link(id(10), id(100), id(101), f.now)
		.await
		.unwrap();
	f.session(30, 1).await;
	f.session(31, 2).await;
	f.proof(102, 1).await;
	f.service()
		.unlink(id(10), id(2), id(102), f.now)
		.await
		.unwrap();
	assert_eq!(
		f.service().check_session(id(30), f.now).await.unwrap(),
		Validity::Active
	);
	assert_eq!(
		f.service().check_session(id(31), f.now).await.unwrap(),
		Validity::Denied(Denial::SessionInvalid)
	);
	assert_eq!(
		f.service()
			.check_origin(
				&DurableOrigin {
					source_disable_epoch: None,
					suspension_epoch: None,
					identity_id: id(2),
					account_id: None
				},
				f.now
			)
			.await
			.unwrap(),
		Validity::Active
	);
	f.proof(103, 3).await;
	f.proof(104, 2).await;
	assert!(matches!(
		f.service().link(id(20), id(103), id(104), f.now).await,
		Err(Error::Rule(AccountError::AlreadyAssigned))
	));
	let state = f.store.0.lock().await;
	assert_eq!(state.identities[&id(2)].first_account, Some(id(10)));
	assert!(!state.identities[&id(2)].linked);
	assert!(
		state.proofs.contains_key(&id(103)),
		"failed linking rolls back proof consumption"
	);
	drop(state);
	f.proof(105, 1).await;
	f.proof(106, 2).await;
	f.service()
		.link(id(10), id(105), id(106), f.now)
		.await
		.unwrap();
	assert_eq!(
		f.service().check_session(id(31), f.now).await.unwrap(),
		Validity::Denied(Denial::SessionInvalid)
	);
}

// Catches trusting SSO, same-source unlink, or consuming only one side's proof.
#[rstest::rstest]
#[tokio::test]
async fn fresh_proofs_are_required_and_cannot_be_replayed(fixture: Fixture) {
	let f = fixture;
	f.proof(100, 1).await;
	assert!(
		f.service()
			.link(id(10), id(100), id(999), f.now)
			.await
			.is_err()
	);
	assert!(f.store.0.lock().await.proofs.contains_key(&id(100)));
	f.proof(101, 2).await;
	f.service()
		.link(id(10), id(100), id(101), f.now)
		.await
		.unwrap();
	assert!(
		f.service()
			.link(id(10), id(100), id(101), f.now)
			.await
			.is_err()
	);
	f.proof(102, 1).await;
	assert!(matches!(
		f.service().unlink(id(10), id(1), id(102), f.now).await,
		Err(Error::Rule(AccountError::LastLoginMethod))
	));
	f.proof(103, 2).await;
	assert!(
		f.service()
			.unlink(id(10), id(1), id(103), f.now + Duration::minutes(4))
			.await
			.is_err()
	);
}

// Catches product-admin escalation, resurrected sessions, and cross-source stop.
#[rstest::rstest]
#[tokio::test]
async fn account_stops_need_auth_admin_and_restore_does_not_revive_sessions(fixture: Fixture) {
	let f = fixture;
	f.session(30, 1).await;
	assert!(matches!(
		f.service()
			.suspend(Administrator::Product, id(10), f.now)
			.await,
		Err(Error::Rule(AccountError::AdministratorRequired))
	));
	assert_eq!(
		f.service().check_session(id(30), f.now).await.unwrap(),
		Validity::Active
	);
	f.service()
		.suspend(Administrator::Authentication, id(10), f.now)
		.await
		.unwrap();
	let mut origin = DurableOrigin {
		source_disable_epoch: None,
		suspension_epoch: None,
		identity_id: id(1),
		account_id: Some(id(10)),
	};
	assert_eq!(
		f.service().check_origin(&origin, f.now).await.unwrap(),
		Validity::Denied(Denial::AccountSuspended)
	);
	f.service()
		.restore(Administrator::Authentication, id(10))
		.await
		.unwrap();
	assert_eq!(
		f.service().check_session(id(30), f.now).await.unwrap(),
		Validity::Denied(Denial::SessionInvalid)
	);
	assert_eq!(
		f.service().check_origin(&origin, f.now).await.unwrap(),
		Validity::Denied(Denial::AccountSuspended)
	);
	origin.suspension_epoch = Some(1);
	assert_eq!(
		f.service().check_origin(&origin, f.now).await.unwrap(),
		Validity::Active
	);
	f.store
		.0
		.lock()
		.await
		.identities
		.get_mut(&id(1))
		.unwrap()
		.status = IdentityStatus::Disabled;
	assert_eq!(
		f.service().check_origin(&origin, f.now).await.unwrap(),
		Validity::Denied(Denial::IdentityDisabled)
	);
	assert_eq!(
		f.service()
			.check_origin(
				&DurableOrigin {
					source_disable_epoch: None,
					suspension_epoch: None,
					identity_id: id(3),
					account_id: Some(id(20))
				},
				f.now
			)
			.await
			.unwrap(),
		Validity::Active
	);
}

// Catches eager legacy account allocation and conflating logout with work stop.
#[rstest::rstest]
#[tokio::test]
async fn legacy_work_and_logout_keep_original_authority(fixture: Fixture) {
	let f = fixture;
	let legacy = DurableOrigin {
		source_disable_epoch: None,
		suspension_epoch: None,
		identity_id: id(2),
		account_id: None,
	};
	assert_eq!(
		f.service().check_origin(&legacy, f.now).await.unwrap(),
		Validity::Active
	);
	assert_eq!(
		f.store.0.lock().await.identities[&id(2)].first_account,
		None
	);
	f.session(30, 1).await;
	f.session(31, 1).await;
	f.service().logout(id(30), f.now).await.unwrap();
	assert_eq!(
		f.service().check_session(id(31), f.now).await.unwrap(),
		Validity::Active
	);
	assert_eq!(
		f.service()
			.check_origin(
				&DurableOrigin {
					source_disable_epoch: None,
					suspension_epoch: None,
					identity_id: id(1),
					account_id: Some(id(10))
				},
				f.now
			)
			.await
			.unwrap(),
		Validity::Active
	);
	f.proof(101, 2).await;
	f.service().register(id(40), id(101), f.now).await.unwrap();
	assert_eq!(
		f.store.0.lock().await.identities[&id(2)].first_account,
		Some(id(40))
	);
}

// Explicit all-products logout needs a live authenticated session, not a fresh
// link proof. An expired session cannot revoke another device's sessions.
#[rstest::rstest]
#[tokio::test]
async fn authenticated_global_logout_requires_live_session_without_new_proof(fixture: Fixture) {
	let f = fixture;
	f.session(30, 1).await;
	f.session(31, 1).await;
	assert!(
		f.service()
			.logout_all_authenticated(id(30), f.now + Duration::hours(13))
			.await
			.is_err()
	);
	assert_eq!(
		f.service().check_session(id(31), f.now).await.unwrap(),
		Validity::Active
	);
	f.service()
		.logout_all_authenticated(id(30), f.now)
		.await
		.unwrap();
	assert_eq!(
		f.service().check_session(id(31), f.now).await.unwrap(),
		Validity::Denied(Denial::SessionInvalid)
	);
	assert_eq!(
		f.service()
			.check_origin(
				&DurableOrigin {
					source_disable_epoch: None,
					suspension_epoch: None,
					identity_id: id(1),
					account_id: Some(id(10))
				},
				f.now
			)
			.await
			.unwrap(),
		Validity::Active
	);
}
