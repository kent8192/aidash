use super::*;
use rstest::rstest;
#[rstest]
#[case::unknown_old(None,json!({"instance":"new"}),false)]
#[case::same(Some("old"),json!({"instance":"old"}),false)]
#[case::changed(Some("old"),json!({"instance":"new"}),true)]
#[case::missing_current(Some("old"),json!({}),false)]
#[case::invalid_current(Some("old"),json!({"instance":42}),false)]
fn only_a_known_changed_identity_proves_the_old_runner_journal_lost(
	#[case] old: Option<&str>,
	#[case] health: Value,
	#[case] lost: bool,
) {
	assert_eq!(journal_lost(old, &health), lost);
}
#[rstest]
#[case::storage("STORAGE_QUOTA", true)]
#[case::storage_detail("STORAGE_QUOTA: tenant", true)]
#[case::files("runner file quota", true)]
#[case::output("runner output quota", true)]
#[case::other_conflict("AREA_BUSY", false)]
#[case::similar_text("runner file quota extra", false)]
fn blocked_publication_uses_only_the_existing_retryable_quota_messages(
	#[case] message: &str,
	#[case] blocked: bool,
) {
	let result = storage_blockage(message);
	assert_eq!(result.is_some(), blocked);
	if blocked {
		assert_eq!(
			result.unwrap(),
			json!({"code":"STORAGE_QUOTA","message":"Storage quota prevents saving this result. Restore capacity to reconcile the same operation; prior files remain intact.","retryable":true})
		);
	}
}
