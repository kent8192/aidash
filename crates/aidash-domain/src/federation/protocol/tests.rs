use super::*;
use rstest::rstest;

#[rstest]
#[case::same(CURRENT, CURRENT, true)]
#[case::previous("0.3", "0.2", true)]
#[case::older_node_does_not_accept_newer("0.2", "0.3", false)]
#[case::equal_unlisted_versions("0.1", "0.1", true)]
#[case::unlisted("0.3", "0.1", false)]
#[case::malformed("0.3", "current", false)]
fn a_node_accepts_its_own_and_listed_older_versions(
	#[case] local: &str,
	#[case] peer: &str,
	#[case] accepted: bool,
) {
	assert_eq!(compatible(local, peer), accepted);
}

#[rstest]
#[case("0.2", false)]
#[case("0.3", true)]
#[case("0.10", true)]
#[case("1.0", true)]
#[case("0.1", false)]
#[case("", false)]
#[case("0.x", false)]
#[case("3", false)]
fn only_peers_at_or_after_the_corpus_digest_version_receive_it(
	#[case] peer: &str,
	#[case] expected: bool,
) {
	assert_eq!(carries_corpus_digest(peer), expected);
}

#[test]
fn the_advertised_version_carries_the_corpus_digest() {
	assert!(carries_corpus_digest(CURRENT));
}
