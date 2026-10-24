use super::*;
use rstest::rstest;

#[rstest]
#[case::same(CURRENT, CURRENT, true)]
#[case::the_version_before_tokens("0.3", CURRENT, false)]
#[case::the_oldest("0.2", CURRENT, false)]
#[case::a_newer_peer(CURRENT, "0.5", false)]
#[case::malformed(CURRENT, "current", false)]
#[case::empty(CURRENT, "", false)]
fn a_node_serves_only_its_own_version(
	#[case] local: &str,
	#[case] peer: &str,
	#[case] accepted: bool,
) {
	assert_eq!(compatible(local, peer), accepted);
}

#[test]
fn the_current_version_is_0_4() {
	assert_eq!(CURRENT, "0.4");
}
