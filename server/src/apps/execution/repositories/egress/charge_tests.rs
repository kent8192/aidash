//! The gateway charges again after anything but a decision, so only a decision
//! may commit the bytes it reports on.
use super::*;

#[test]
fn only_a_decision_about_the_operation_commits_its_bytes() {
	for decision in [
		Error::Forbidden,
		Error::Unauthorized,
		Error::TenantDeleted,
		Error::Conflict(aidash_domain::egress::OPERATION_EXPIRED.into()),
		Error::Conflict(aidash_domain::egress::LIMIT_EXCEEDED.into()),
	] {
		assert!(stops_stream(&decision), "{decision}");
	}
	for failure in [
		Error::Database(sqlx::Error::PoolTimedOut),
		Error::External("runner unreachable".into()),
		Error::NotFound("run".into()),
		Error::Conflict("something_else".into()),
	] {
		assert!(!stops_stream(&failure), "{failure}");
	}
}
