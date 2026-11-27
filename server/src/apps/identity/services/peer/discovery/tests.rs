use super::{StreamExt, peer_responses};
use futures_util::poll;
use std::cell::Cell;
use tokio::sync::Semaphore;

#[tokio::test]
async fn one_page_of_peer_requests_can_run_while_the_rest_wait() {
	let started = Cell::new(0);
	let finish = Semaphore::new(0);
	let targets = (0..26).map(|node| node.to_string()).collect();
	let responses = peer_responses(targets, |node| {
		let started = &started;
		let finish = &finish;
		async move {
			started.set(started.get() + 1);
			finish.acquire().await.unwrap().forget();
			node
		}
	});
	tokio::pin!(responses);

	assert!(poll!(responses.next()).is_pending());
	assert_eq!(started.get(), 25);
	finish.add_permits(26);
	assert_eq!(responses.collect::<Vec<_>>().await.len(), 26);
}
