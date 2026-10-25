//! Human authentication provenance. An Account or Link grants no product authority.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccountStatus {
	Active,
	Suspended,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdentityStatus {
	Enabled,
	Disabled,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
	pub id: Uuid,
	pub status: AccountStatus,
	pub session_epoch: u64,
	#[serde(default)]
	pub suspension_epoch: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalIdentity {
	pub id: Uuid,
	pub issuer: String,
	pub subject: String,
	/// Immutable after first assignment, including after unlinking.
	pub first_account: Option<Uuid>,
	pub linked: bool,
	pub status: IdentityStatus,
	#[serde(default)]
	pub disable_epoch: u64,
	pub last_valid_at: Option<DateTime<Utc>>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductSession {
	pub id: Uuid,
	pub account_id: Uuid,
	pub identity_id: Uuid,
	pub client_id: String,
	pub device_id: String,
	pub issued_at: DateTime<Utc>,
	pub last_seen_at: DateTime<Utc>,
	pub absolute_expires_at: DateTime<Utc>,
	pub idle_seconds: i64,
	pub account_epoch: u64,
	pub revoked_at: Option<DateTime<Utc>>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurableOrigin {
	pub identity_id: Uuid,
	pub account_id: Option<Uuid>,
	#[serde(default)]
	pub suspension_epoch: Option<u64>,
	#[serde(default)]
	pub source_disable_epoch: Option<u64>,
}
/// Created only after explicit upstream reauthentication, never from an SSO cookie.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreshProof {
	pub id: Uuid,
	pub identity_id: Uuid,
	pub authenticated_at: DateTime<Utc>,
	pub expires_at: DateTime<Utc>,
}
/// The adapter derives this from authenticated service/administrator authorization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Administrator {
	Authentication,
	Product,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Denial {
	IdentityDisabled,
	AccountSuspended,
	SessionInvalid,
	ProvenanceMismatch,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Validity {
	Active,
	Denied(Denial),
	Unavailable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AccountError {
	#[error("external identity already assigned to another account")]
	AlreadyAssigned,
	#[error("fresh authentication proof required")]
	FreshProofRequired,
	#[error("another usable linked identity is required")]
	LastLoginMethod,
	#[error("authentication administrator required")]
	AdministratorRequired,
	#[error("authentication is not valid")]
	InvalidAuthentication,
	#[error("authentication record does not exist")]
	NotFound,
	#[error("authentication provenance differs")]
	ProvenanceMismatch,
	#[error("authentication epoch exhausted")]
	EpochExhausted,
}
impl ExternalIdentity {
	/// Links are login methods; historical ownership is never cleared by unlink.
	pub fn attach(&mut self, account: Uuid) -> Result<(), AccountError> {
		if self.first_account.is_some_and(|first| first != account) {
			return Err(AccountError::AlreadyAssigned);
		}
		self.first_account = Some(account);
		self.linked = true;
		Ok(())
	}
	/// Adapters refresh after five minutes; only a failed upstream lookup may
	/// use the remaining grace. A known disablement always wins.
	pub fn needs_status_refresh(&self, now: DateTime<Utc>) -> bool {
		self.last_valid_at
			.is_none_or(|last| last > now || (now - last).num_seconds() >= 300)
	}
	pub fn validity(&self, now: DateTime<Utc>) -> Validity {
		if self.status == IdentityStatus::Disabled {
			return Validity::Denied(Denial::IdentityDisabled);
		}
		match self.last_valid_at {
			Some(last) if last <= now && (now - last).num_seconds() < 900 => Validity::Active,
			_ => Validity::Unavailable,
		}
	}
}
impl FreshProof {
	pub fn valid_at(&self, now: DateTime<Utc>) -> bool {
		self.authenticated_at <= now
			&& now < self.expires_at
			&& self.expires_at <= self.authenticated_at + chrono::Duration::minutes(5)
	}
}
/// Unassigned legacy origins acquire historical Account provenance lazily from
/// the source record. An absent login Link is deliberately irrelevant here.
pub fn durable_validity(
	origin: &DurableOrigin,
	account: Option<&Account>,
	identity: &ExternalIdentity,
	now: DateTime<Utc>,
) -> Validity {
	if origin.identity_id != identity.id
		|| origin
			.account_id
			.is_some_and(|id| Some(id) != identity.first_account)
	{
		return Validity::Denied(Denial::ProvenanceMismatch);
	}
	if origin.source_disable_epoch.unwrap_or(0) != identity.disable_epoch {
		return Validity::Denied(Denial::IdentityDisabled);
	}
	if let Some(id) = identity.first_account {
		let Some(account) = account else {
			return Validity::Unavailable;
		};
		if account.id != id {
			return Validity::Denied(Denial::ProvenanceMismatch);
		}
		if account.status == AccountStatus::Suspended
			|| origin.suspension_epoch.unwrap_or(0) != account.suspension_epoch
		{
			return Validity::Denied(Denial::AccountSuspended);
		}
	} else if account.is_some() {
		return Validity::Denied(Denial::ProvenanceMismatch);
	}
	identity.validity(now)
}
pub fn interactive_validity(
	session: &ProductSession,
	account: &Account,
	identity: &ExternalIdentity,
	now: DateTime<Utc>,
) -> Validity {
	if session.account_id != account.id
		|| session.identity_id != identity.id
		|| identity.first_account != Some(account.id)
	{
		return Validity::Denied(Denial::ProvenanceMismatch);
	}
	if account.status == AccountStatus::Suspended {
		return Validity::Denied(Denial::AccountSuspended);
	}
	if !identity.linked
		|| session.revoked_at.is_some()
		|| session.account_epoch != account.session_epoch
		|| session.issued_at > now
		|| session.last_seen_at > now
		|| session.last_seen_at < session.issued_at
		|| session.absolute_expires_at <= now
		|| session.idle_seconds <= 0
		|| now - session.last_seen_at >= chrono::Duration::seconds(session.idle_seconds)
	{
		return Validity::Denied(Denial::SessionInvalid);
	}
	identity.validity(now)
}
#[cfg(test)]
mod tests;
