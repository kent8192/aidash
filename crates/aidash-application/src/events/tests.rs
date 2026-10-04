use super::*;
use crate::Error;
use aidash_domain::Event;
use async_trait::async_trait;
use rstest::{fixture, rstest};
use serde_json::json;
use std::sync::{
	Arc, Mutex,
	atomic::{AtomicBool, Ordering},
};
use uuid::Uuid;

struct Outbox {
	events: Vec<Event>,
	state: Arc<Publication>,
}
#[derive(Default)]
struct Publication {
	held: AtomicBool,
	finished: Mutex<Vec<(Uuid, Option<String>)>>,
	lose_claim: bool,
}
struct Batch {
	events: Vec<Event>,
	state: Arc<Publication>,
}
impl Drop for Batch {
	fn drop(&mut self) {
		self.state.held.store(false, Ordering::SeqCst);
	}
}
#[async_trait]
impl EventOutbox for Outbox {
	async fn claim(&self) -> Result<Box<dyn EventOutboxBatch>> {
		self.state.held.store(true, Ordering::SeqCst);
		Ok(Box::new(Batch {
			events: self.events.clone(),
			state: self.state.clone(),
		}))
	}
}
#[async_trait]
impl EventOutboxBatch for Batch {
	fn events(&self) -> Vec<Event> {
		self.events.clone()
	}
	async fn finish(&self, id: Uuid, error: Option<String>) -> Result<bool> {
		assert!(self.state.held.load(Ordering::SeqCst));
		self.state.finished.lock().unwrap().push((id, error));
		Ok(!self.state.lose_claim)
	}
}
struct Publisher {
	failing: Option<Uuid>,
	pending: bool,
}
#[async_trait]
impl EventPublisher for Publisher {
	async fn publish(&self, event: &Event) -> Result<()> {
		if self.pending {
			return std::future::pending().await;
		}
		if self.failing == Some(event.id) {
			Err(Error::External("broker unavailable".into()))
		} else {
			Ok(())
		}
	}
}
#[fixture]
fn outbox() -> Outbox {
	Outbox {
		events: (1..=2)
			.map(|sequence| Event {
				sequence,
				id: Uuid::new_v4(),
				node_id: "aidash://fixture".into(),
				workspace_id: None,
				kind: "fixture".into(),
				data: json!({"sequence":sequence}),
				created_at: "2026-10-02T00:00:00Z".parse().unwrap(),
			})
			.collect(),
		state: Arc::new(Publication::default()),
	}
}
#[rstest]
#[tokio::test]
async fn one_failed_event_is_retained_while_later_events_are_published(outbox: Outbox) {
	// Arrange
	let failed = outbox.events[0].id;
	let publisher = Publisher {
		failing: Some(failed),
		pending: false,
	};
	// Act
	let count = publish(&outbox, &publisher).await.unwrap();
	// Assert
	assert_eq!(count, 1);
	let finished = outbox.state.finished.lock().unwrap();
	assert!(finished.contains(&(failed, Some("broker unavailable".into()))));
	assert!(finished.contains(&(outbox.events[1].id, None)));
	assert_eq!(finished.len(), 2);
	assert!(!outbox.state.held.load(Ordering::SeqCst));
}
#[rstest]
#[tokio::test]
async fn stale_claim_acknowledgements_are_not_counted_as_publication(mut outbox: Outbox) {
	Arc::get_mut(&mut outbox.state).unwrap().lose_claim = true;
	let count = publish(
		&outbox,
		&Publisher {
			failing: None,
			pending: false,
		},
	)
	.await
	.unwrap();
	assert_eq!(count, 0);
	assert_eq!(outbox.state.finished.lock().unwrap().len(), 2);
}
#[rstest]
#[tokio::test]
async fn cancelled_publication_releases_the_owned_visibility_lease(outbox: Outbox) {
	let publisher = Publisher {
		failing: None,
		pending: true,
	};
	let mut work = Box::pin(publish(&outbox, &publisher));
	assert!(futures_util::poll!(work.as_mut()).is_pending());
	assert!(outbox.state.held.load(Ordering::SeqCst));
	drop(work);
	assert!(!outbox.state.held.load(Ordering::SeqCst));
	assert!(outbox.state.finished.lock().unwrap().is_empty());
}

struct Receiver {
	calls: Arc<Mutex<Vec<&'static str>>>,
	inserted: bool,
	fail: bool,
}
struct Delivery {
	id: Option<Uuid>,
	calls: Arc<Mutex<Vec<&'static str>>>,
}
#[async_trait]
impl EventDelivery for Delivery {
	fn event_id(&self) -> Option<Uuid> {
		self.id
	}
	async fn acknowledge(self: Box<Self>) -> Result<()> {
		self.calls.lock().unwrap().push("ack");
		Ok(())
	}
	async fn discard(self: Box<Self>) -> Result<()> {
		self.calls.lock().unwrap().push("discard");
		Ok(())
	}
}
struct Subscription(Option<Delivery>);
#[async_trait]
impl EventSubscription for Subscription {
	async fn next(&mut self) -> Result<Box<dyn EventDelivery>> {
		Ok(Box::new(self.0.take().expect("one delivery")))
	}
}
#[async_trait]
impl EventInbox for Receiver {
	async fn receive(&self, _id: Uuid) -> Result<bool> {
		self.calls.lock().unwrap().push("commit");
		if self.fail {
			Err(Error::External("inbox unavailable".into()))
		} else {
			Ok(self.inserted)
		}
	}
}
impl EventWakeup for Receiver {
	fn wake(&self) {
		self.calls.lock().unwrap().push("wake");
	}
}
#[rstest]
#[case::new_event(true,vec!["commit","wake","ack"])]
#[case::duplicate(false,vec!["commit","ack"])]
#[tokio::test]
async fn inbox_commit_precedes_wakeup_and_acknowledgement(
	#[case] inserted: bool,
	#[case] expected: Vec<&str>,
) {
	let receiver = Receiver {
		calls: Arc::new(Mutex::new(vec![])),
		inserted,
		fail: false,
	};
	let mut subscription = Subscription(Some(Delivery {
		id: Some(Uuid::new_v4()),
		calls: receiver.calls.clone(),
	}));
	receive(&receiver, &mut subscription, &receiver)
		.await
		.unwrap();
	assert_eq!(*receiver.calls.lock().unwrap(), expected);
}
#[rstest]
#[tokio::test]
async fn malformed_envelopes_are_discarded_without_an_inbox_write() {
	let receiver = Receiver {
		calls: Arc::new(Mutex::new(vec![])),
		inserted: true,
		fail: false,
	};
	let mut subscription = Subscription(Some(Delivery {
		id: None,
		calls: receiver.calls.clone(),
	}));
	receive(&receiver, &mut subscription, &receiver)
		.await
		.unwrap();
	assert_eq!(*receiver.calls.lock().unwrap(), ["discard"]);
}
#[rstest]
#[tokio::test]
async fn an_uncommitted_inbox_never_wakes_or_acknowledges() {
	let receiver = Receiver {
		calls: Arc::new(Mutex::new(vec![])),
		inserted: true,
		fail: true,
	};
	let mut subscription = Subscription(Some(Delivery {
		id: Some(Uuid::new_v4()),
		calls: receiver.calls.clone(),
	}));
	assert!(matches!(
		receive(&receiver, &mut subscription, &receiver).await,
		Err(Error::External(_))
	));
	assert_eq!(*receiver.calls.lock().unwrap(), ["commit"]);
}
