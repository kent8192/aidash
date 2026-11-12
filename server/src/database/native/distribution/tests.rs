use super::*;
use rstest::rstest;

#[rstest]
#[tokio::test]
async fn a_peer_is_covered_only_while_its_caller_holds_the_lock() {
	let hold = Hold::default();
	assert!(!caller_holds());
	assert!(!serving(&hold, async { caller_holds() }).await);
	hold.take();
	assert!(serving(&hold, async { caller_holds() }).await);
	assert!(
		!caller_holds(),
		"the cover ends with the call it was made for"
	);
	hold.release();
	assert!(!serving(&hold, async { caller_holds() }).await);
}

#[rstest]
#[tokio::test]
async fn a_peer_that_asks_another_node_stays_covered() {
	let holder = Hold::default();
	holder.take();
	// The peer's own transaction holds nothing, yet what it asks inherits the cover.
	let peer = Hold::default();
	let covered = serving(&holder, async {
		serving(&peer, async { caller_holds() }).await
	})
	.await;
	assert!(covered);
}

#[rstest]
#[tokio::test]
async fn an_authority_under_a_lease_holds_what_the_lease_holds() {
	let lease = Hold::default();
	let mut inner = Hold::default();
	inner.inherit(lease.clone());
	assert!(!inner.held());
	lease.take();
	assert!(
		inner.held(),
		"the lease is read when asked, not when linked"
	);
	assert!(serving(&inner, async { caller_holds() }).await);
	// Releasing the inner transaction's own hold never gives up the lease's.
	inner.release();
	assert!(inner.held());
	lease.release();
	assert!(!inner.held());
}

#[rstest]
#[tokio::test]
async fn clones_of_a_hold_see_the_same_lock() {
	let hold = Hold::default();
	let seen = hold.clone();
	hold.take();
	assert!(seen.held());
	seen.release();
	assert!(!hold.held());
}
