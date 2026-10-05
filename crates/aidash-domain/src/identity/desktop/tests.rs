use super::*;
use rstest::rstest;

#[rstest]
#[case(None, false, Rotation::Issue)]
#[case(Some(b"next".as_slice()), true, Rotation::Recover)]
#[case(Some(b"next".as_slice()), false, Rotation::Revoke)]
#[case(Some(b"another".as_slice()), true, Rotation::Revoke)]
fn refresh_rotation_requires_the_exact_current_successor(
	#[case] previous: Option<&[u8]>,
	#[case] current: bool,
	#[case] expected: Rotation,
) {
	assert_eq!(rotation(previous, b"next", current), expected);
}

#[rstest]
#[case(-1, false, true)]
#[case(-1, true, false)]
#[case(1, true, true)]
#[case(0, true, false)]
fn renewal_can_recover_an_expired_access_token_without_authenticating_it(
	#[case] access_seconds: i64,
	#[case] require_access: bool,
	#[case] expected: bool,
) {
	let now = Utc::now();
	let session = Session {
		desktop: true,
		revoked_at: None,
		expires_at: now + Duration::hours(1),
		last_activity_at: now,
		idle_seconds: Some(60),
		access_expires_at: Some(now + Duration::seconds(access_seconds)),
	};
	assert_eq!(session.current(now, require_access), expected);
}
