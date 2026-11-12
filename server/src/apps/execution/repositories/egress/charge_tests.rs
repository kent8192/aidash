//! The gateway charges again after anything but a decision, so only a decision
//! may commit the bytes it reports on. This is the rule on its own; the
//! database test `native_gateway_serializes_cumulative_limits_and_refuses_revoked_bindings`
//! shows a real debit keeping it, both for a decision and for a failure.
use super::*;

#[test]
fn only_a_decision_about_the_operation_commits_its_bytes() {
	for decision in [
		Error::Forbidden,
		Error::Unauthorized,
		Error::TenantDeleted,
		Error::AuthorityRefused(Box::new(aidash_domain::tool::AuthorityRefusal {
			tool: "fixture".into(),
			check: aidash_domain::tool::RefusalCheck::Policy,
			action: None,
			subject: None,
			call: None,
		})),
		Error::Refused("egress_budget_exhausted".into()),
		Error::Refused("egress_suspended".into()),
		Error::Conflict(aidash_domain::egress::OPERATION_EXPIRED.into()),
		Error::Conflict(aidash_domain::egress::OPERATION_INACTIVE.into()),
		Error::Conflict(aidash_domain::egress::LIMIT_EXCEEDED.into()),
	] {
		let decision = aidash_application::Error::from(decision);
		assert!(DebitRefusal::of(&decision).is_some(), "{decision}");
	}
	for failure in [
		Error::Database(sqlx::Error::PoolTimedOut),
		Error::External("runner unreachable".into()),
		Error::NotFound("run".into()),
		Error::Conflict("something_else".into()),
	] {
		let failure = aidash_application::Error::from(failure);
		assert!(DebitRefusal::of(&failure).is_none(), "{failure}");
	}
}
