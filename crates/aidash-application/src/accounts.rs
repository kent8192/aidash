//! Human Account use cases; no Tenant, Mapping or service credential authority.
use crate::ports::accounts::{Repository, Scope};
use aidash_domain::accounts::*;
use chrono::{DateTime, Utc};
use uuid::Uuid;
#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error(transparent)]
	Rule(#[from] AccountError),
	#[error(transparent)]
	Store(#[from] crate::Error),
}
pub type Result<T> = std::result::Result<T, Error>;
pub struct Service<R> {
	repository: R,
}
impl<R: Repository> Service<R> {
	pub fn new(repository: R) -> Self {
		Self { repository }
	}

	/// Explicit first assignment, including a legacy Identity's choice of a
	/// new Account. No email, product provisioning, or local reference mutation.
	pub async fn register(&self, account: Uuid, proof: Uuid, now: DateTime<Utc>) -> Result<()> {
		let mut tx = self.repository.begin().await?;
		if tx.account(account).await?.is_some() {
			return Err(AccountError::AlreadyAssigned.into());
		}
		let mut identity = proven_identity(tx.as_mut(), proof, now).await?;
		if identity.first_account.is_some() {
			return Err(AccountError::AlreadyAssigned.into());
		}
		identity.attach(account)?;
		tx.save_account(&Account {
			id: account,
			status: AccountStatus::Active,
			session_epoch: 0,
			suspension_epoch: 0,
		})
		.await?;
		tx.save_identity(&identity).await?;
		tx.commit().await?;
		Ok(())
	}

	/// Both proofs come from explicit reauthentication and are consumed together.
	pub async fn link(
		&self,
		account: Uuid,
		account_proof: Uuid,
		target_proof: Uuid,
		now: DateTime<Utc>,
	) -> Result<()> {
		let mut tx = self.repository.begin().await?;
		let source = proven_account(tx.as_mut(), account, account_proof, now).await?;
		let mut target = proven_identity(tx.as_mut(), target_proof, now).await?;
		if source.id == target.id {
			return Err(AccountError::FreshProofRequired.into());
		}
		target.attach(account)?;
		tx.save_identity(&target).await?;
		tx.commit().await?;
		Ok(())
	}

	/// Prove a different usable login method. Its successful fresh proof also
	/// establishes that removing the target cannot remove the last usable one.
	pub async fn unlink(
		&self,
		account: Uuid,
		target: Uuid,
		other_proof: Uuid,
		now: DateTime<Utc>,
	) -> Result<()> {
		let mut tx = self.repository.begin().await?;
		let other = proven_account(tx.as_mut(), account, other_proof, now).await?;
		if other.id == target {
			return Err(AccountError::LastLoginMethod.into());
		}
		let mut target = tx.identity(target).await?.ok_or(AccountError::NotFound)?;
		if target.first_account != Some(account) || !target.linked {
			return Err(AccountError::ProvenanceMismatch.into());
		}
		target.linked = false;
		tx.save_identity(&target).await?;
		tx.revoke_identity_sessions(target.id, now).await?;
		tx.commit().await?;
		Ok(())
	}

	/// The adapter authenticates the administrator; a product service credential
	/// or Deployment/Tenant Administrator must map to `Product`, not this role.
	pub async fn suspend(
		&self,
		administrator: Administrator,
		account: Uuid,
		now: DateTime<Utc>,
	) -> Result<()> {
		require_administrator(administrator)?;
		let mut tx = self.repository.begin().await?;
		let mut account = tx.account(account).await?.ok_or(AccountError::NotFound)?;
		account.status = AccountStatus::Suspended;
		account.suspension_epoch = account
			.suspension_epoch
			.checked_add(1)
			.ok_or(AccountError::EpochExhausted)?;
		account.session_epoch = account
			.session_epoch
			.checked_add(1)
			.ok_or(AccountError::EpochExhausted)?;
		tx.save_account(&account).await?;
		tx.revoke_account_sessions(account.id, now).await?;
		tx.commit().await?;
		Ok(())
	}

	/// Restores eligibility for fresh login/manual authorized resume. It does
	/// not revive sessions, reset epochs, or emit an automatic Run resume.
	pub async fn restore(&self, administrator: Administrator, account: Uuid) -> Result<()> {
		require_administrator(administrator)?;
		let mut tx = self.repository.begin().await?;
		let mut account = tx.account(account).await?.ok_or(AccountError::NotFound)?;
		account.status = AccountStatus::Active;
		tx.save_account(&account).await?;
		tx.commit().await?;
		Ok(())
	}

	/// Caller must already authenticate ownership of this product/device's
	/// opaque session. This revokes only that session, never durable work.
	pub async fn logout(&self, session: Uuid, now: DateTime<Utc>) -> Result<()> {
		let mut tx = self.repository.begin().await?;
		let mut session = tx.session(session).await?.ok_or(AccountError::NotFound)?;
		session.revoked_at = Some(now);
		tx.save_session(&session).await?;
		tx.commit().await?;
		Ok(())
	}

	/// The caller verifies service/client ownership of the opaque Session first.
	pub async fn logout_all_authenticated(&self, session: Uuid, now: DateTime<Utc>) -> Result<()> {
		let mut tx = self.repository.begin().await?;
		let session = tx.session(session).await?.ok_or(AccountError::NotFound)?;
		let mut account = tx
			.account(session.account_id)
			.await?
			.ok_or(AccountError::NotFound)?;
		let identity = tx
			.identity(session.identity_id)
			.await?
			.ok_or(AccountError::NotFound)?;
		if interactive_validity(&session, &account, &identity, now) != Validity::Active {
			return Err(AccountError::InvalidAuthentication.into());
		}
		account.session_epoch = account
			.session_epoch
			.checked_add(1)
			.ok_or(AccountError::EpochExhausted)?;
		tx.save_account(&account).await?;
		tx.revoke_account_sessions(account.id, now).await?;
		tx.commit().await?;
		Ok(())
	}

	/// Successful admission touches idle time, never the absolute deadline.
	/// The product still checks its own permissions for this original Identity.
	pub async fn check_session(&self, session: Uuid, now: DateTime<Utc>) -> Result<Validity> {
		let mut tx = self.repository.begin().await?;
		let Some(mut session) = tx.session(session).await? else {
			return Ok(Validity::Denied(Denial::SessionInvalid));
		};
		let Some(account) = tx.account(session.account_id).await? else {
			return Ok(Validity::Unavailable);
		};
		let Some(identity) = tx.identity(session.identity_id).await? else {
			return Ok(Validity::Denied(Denial::ProvenanceMismatch));
		};
		let validity = interactive_validity(&session, &account, &identity, now);
		if validity == Validity::Active {
			session.last_seen_at = now;
			tx.save_session(&session).await?;
			tx.commit().await?;
		}
		Ok(validity)
	}

	/// Legacy work remains checkable without allocating an Account. Once first
	/// assignment exists, its retained Account is checked even after unlinking.
	pub async fn check_origin(
		&self,
		origin: &DurableOrigin,
		now: DateTime<Utc>,
	) -> Result<Validity> {
		let mut tx = self.repository.begin().await?;
		let Some(identity) = tx.identity(origin.identity_id).await? else {
			return Ok(Validity::Denied(Denial::ProvenanceMismatch));
		};
		let account = match identity.first_account {
			Some(id) => tx.account(id).await?,
			None => None,
		};
		Ok(durable_validity(origin, account.as_ref(), &identity, now))
	}
}

async fn proven_identity(
	tx: &mut dyn Scope,
	proof: Uuid,
	now: DateTime<Utc>,
) -> Result<ExternalIdentity> {
	let proof_record = tx
		.consume_proof(proof)
		.await?
		.ok_or(AccountError::FreshProofRequired)?;
	if proof_record.id != proof || !proof_record.valid_at(now) {
		return Err(AccountError::FreshProofRequired.into());
	}
	let identity = tx
		.identity(proof_record.identity_id)
		.await?
		.ok_or(AccountError::NotFound)?;
	if identity.validity(now) != Validity::Active {
		return Err(AccountError::InvalidAuthentication.into());
	}
	Ok(identity)
}
async fn proven_account(
	tx: &mut dyn Scope,
	account: Uuid,
	proof: Uuid,
	now: DateTime<Utc>,
) -> Result<ExternalIdentity> {
	let source = proven_identity(tx, proof, now).await?;
	let account_record = tx.account(account).await?.ok_or(AccountError::NotFound)?;
	if account_record.status != AccountStatus::Active
		|| !source.linked
		|| source.first_account != Some(account)
	{
		return Err(AccountError::InvalidAuthentication.into());
	}
	Ok(source)
}
fn require_administrator(administrator: Administrator) -> Result<()> {
	if administrator != Administrator::Authentication {
		return Err(AccountError::AdministratorRequired.into());
	}
	Ok(())
}
#[cfg(test)]
#[path = "accounts/tests.rs"]
mod tests;
