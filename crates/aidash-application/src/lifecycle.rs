//! Cooperative cancellation shared by use cases and their process supervisor.
use tokio::sync::watch;

#[derive(Clone)]
pub struct StopToken {
	receiver: watch::Receiver<bool>,
}

impl StopToken {
	pub fn new(receiver: watch::Receiver<bool>) -> Self {
		Self { receiver }
	}

	pub fn is_stopping(&self) -> bool {
		*self.receiver.borrow()
	}

	/// A closed owner channel also means that work must stop.
	pub async fn stopped(&mut self) {
		while !*self.receiver.borrow_and_update() {
			if self.receiver.changed().await.is_err() {
				break;
			}
		}
	}
}
