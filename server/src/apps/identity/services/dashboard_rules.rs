//! Dashboard authority rules without HTTP or persistence I/O.

use super::{Snapshot, identity, policy::SubjectKind};
use crate::apps::identity::serializers::oidc::Registration;
use crate::{Error, Result};
use chrono::{DateTime, Duration, Utc};

pub(crate) fn reuse_registration(
	previous: &Registration,
	now: DateTime<Utc>,
	active_mapping: bool,
) -> Result<bool> {
	if previous.status == "pending" {
		return Ok(true);
	}
	if previous.status == "rejected"
		&& previous
			.decided_at
			.is_some_and(|time| time > now - Duration::hours(24))
	{
		return Err(Error::Conflict(
			"registration can be resubmitted after 24 hours".into(),
		));
	}
	if previous.status == "approved" && active_mapping {
		return Err(Error::Conflict("registration was already approved".into()));
	}
	Ok(false)
}

pub(crate) fn require_pending(
	status: &str,
	expires_at: DateTime<Utc>,
	now: DateTime<Utc>,
) -> Result<()> {
	if status != "pending" || expires_at <= now {
		return Err(Error::Conflict("registration is no longer pending".into()));
	}
	Ok(())
}

pub(crate) fn require_user_mapping(snapshot: &Snapshot, subject: &str) -> Result<()> {
	if !identity::enabled(snapshot, subject)
		|| snapshot
			.bundle
			.subjects
			.get(subject)
			.is_none_or(|subject| subject.kind != SubjectKind::User)
	{
		return Err(Error::Invalid(
			"mapping requires an enabled user subject".into(),
		));
	}
	Ok(())
}

pub(crate) fn next_revision(revision: i64) -> Result<i64> {
	revision
		.checked_add(1)
		.ok_or_else(|| Error::Conflict("revision exhausted".into()))
}
