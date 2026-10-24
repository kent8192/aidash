use super::*;
use rstest::rstest;

fn at(seconds: i64) -> DateTime<Utc> {
	DateTime::from_timestamp(1_800_000_000 + seconds, 0).unwrap()
}

fn deletion(deleted_by: Actor) -> Deletion {
	Deletion {
		deleted_by,
		purge_after: purge_after(at(0), DEFAULT_GRACE_DAYS),
		restored: false,
		purged: false,
	}
}

#[test]
fn the_grace_period_defaults_to_thirty_days() {
	assert_eq!(
		purge_after(at(0), DEFAULT_GRACE_DAYS),
		at(30 * 24 * 60 * 60)
	);
}

#[rstest]
#[case::zero(0, false)]
#[case::one(1, true)]
#[case::default(30, true)]
#[case::longest(90, true)]
#[case::too_long(91, false)]
fn the_grace_period_is_finite(#[case] days: u32, #[case] valid: bool) {
	assert_eq!(grace_days(days).is_ok(), valid);
}

#[rstest]
#[case::exact("brave-otter-k3m9q2xa", true)]
#[case::surrounding_space(" brave-otter-k3m9q2xa ", true)]
#[case::another("calm-heron-p2s8d1qe", false)]
#[case::empty("", false)]
fn deletion_is_confirmed_by_the_tenant_name(#[case] typed: &str, #[case] confirmed: bool) {
	assert_eq!(confirm("brave-otter-k3m9q2xa", typed).is_ok(), confirmed);
}

#[rstest]
#[case::by_tenant_administrator(Actor::TenantAdministrator, Actor::TenantAdministrator, true)]
#[case::by_deployment_administrator_for_tenant(
	Actor::TenantAdministrator,
	Actor::DeploymentAdministrator,
	true
)]
#[case::deployment_measure_by_tenant(
	Actor::DeploymentAdministrator,
	Actor::TenantAdministrator,
	false
)]
#[case::deployment_measure_by_deployment(
	Actor::DeploymentAdministrator,
	Actor::DeploymentAdministrator,
	true
)]
fn only_a_deployment_administrator_undoes_a_deployment_measure(
	#[case] deleted_by: Actor,
	#[case] restorer: Actor,
	#[case] allowed: bool,
) {
	assert_eq!(restorable(&deletion(deleted_by), restorer, at(60)), allowed);
}

#[test]
fn restore_ends_with_the_grace_period() {
	let deletion = deletion(Actor::TenantAdministrator);
	let last = deletion.purge_after - Duration::seconds(1);
	assert!(check_restore(&deletion, Actor::TenantAdministrator, last).is_ok());
	assert!(matches!(
		check_restore(&deletion, Actor::DeploymentAdministrator, deletion.purge_after),
		Err(Error::Conflict(reason)) if reason == "tenant_grace_period_ended"
	));
}

#[test]
fn restored_and_purged_deletions_cannot_be_restored() {
	let mut restored = deletion(Actor::TenantAdministrator);
	restored.restored = true;
	assert!(!restorable(
		&restored,
		Actor::DeploymentAdministrator,
		at(60)
	));
	let mut purged = deletion(Actor::TenantAdministrator);
	purged.purged = true;
	assert!(matches!(
		check_restore(&purged, Actor::DeploymentAdministrator, at(60)),
		Err(Error::Conflict(reason)) if reason == "tenant_purged"
	));
}

#[rstest]
#[case::tenant_administrator(Actor::TenantAdministrator)]
#[case::deployment_administrator(Actor::DeploymentAdministrator)]
fn actors_round_trip(#[case] actor: Actor) {
	assert_eq!(Actor::parse(actor.as_str()).unwrap(), actor);
}

#[test]
fn a_purge_runs_its_steps_in_order_and_ends_after_the_last() {
	let mut order = vec![PurgeStep::FIRST];
	while let Some(next) = order.last().unwrap().next() {
		order.push(next);
	}
	assert_eq!(order, PurgeStep::ALL);
	assert!(PurgeStep::ALL.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn a_purge_step_round_trips_its_stored_name() {
	for step in PurgeStep::ALL {
		assert_eq!(PurgeStep::parse(step.as_str()).unwrap(), step);
	}
	assert!(PurgeStep::parse("sweep").is_err());
}

#[rstest]
#[case::once(1, false)]
#[case::twice(2, false)]
#[case::three_times(3, true)]
fn a_purge_that_keeps_failing_is_stalled(#[case] failures: u32, #[case] stalled_now: bool) {
	assert_eq!(stalled(failures), stalled_now);
}

#[rstest]
#[case::zero(0, false)]
#[case::one(1, true)]
#[case::default(365, true)]
#[case::longest(3_650, true)]
#[case::too_long(3_651, false)]
fn audit_history_is_kept_for_a_finite_period(#[case] days: u32, #[case] valid: bool) {
	assert_eq!(audit_retention_days(days).is_ok(), valid);
}

#[test]
fn audit_history_is_kept_a_year_by_default() {
	assert_eq!(
		audit_erase_after(at(0), DEFAULT_AUDIT_RETENTION_DAYS),
		at(365 * 24 * 60 * 60)
	);
}
