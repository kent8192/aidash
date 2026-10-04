//! Delegated identity and provider freshness rules are independent of transport.
use chrono::{DateTime, Duration, Utc};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DashboardStatusFailure {
	Forbidden,
	Unavailable,
}
pub type IdentityValidity = (String, Option<DateTime<Utc>>, Option<DateTime<Utc>>);
pub fn dashboard_status(
	validity: Option<IdentityValidity>,
	now: DateTime<Utc>,
	google_issuer: &str,
) -> Result<(), DashboardStatusFailure> {
	let Some((issuer, Some(last_valid_at), None)) = validity else {
		return Err(DashboardStatusFailure::Forbidden);
	};
	if issuer != google_issuer && last_valid_at <= now - Duration::minutes(15) {
		return Err(DashboardStatusFailure::Unavailable);
	}
	Ok(())
}
pub fn enabled(bundle: &crate::policy::PolicyBundle, subject: &str) -> bool {
	if bundle.validate().is_err() {
		return false;
	}
	let mut current = Some(subject);
	while let Some(id) = current {
		let Some(subject) = bundle.subjects.get(id) else {
			return false;
		};
		if !subject.enabled {
			return false;
		}
		current = subject.delegated_by.as_deref();
	}
	true
}
#[cfg(test)]
mod tests;
