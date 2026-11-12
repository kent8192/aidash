//! Who holds the Deployment's distribution lock, for an in-process peer (#751).
//!
//! One advisory lock orders Marketplace distribution changes against Catalog
//! admissions. A reader that holds it shared and then asks another Node of this
//! Deployment in process must not make that Node's own transaction request it
//! again. With a writer queued in between, Postgres puts the new shared request
//! behind the writer, the writer waits for the reader, and the reader waits for
//! the peer until its 10 second limit. The peer runs inside the reader's task
//! and the reader keeps its transaction open meanwhile, so the reader's lock
//! covers what the peer reads and the peer takes none of its own.
use std::sync::{
	Arc,
	atomic::{AtomicBool, Ordering},
};

/// Whether a transaction holds the lock, as far as this process knows. It is
/// false whenever that is uncertain, because a peer that asks again only waits
/// for its limit, while one that skips a lock nobody holds reads unordered.
#[derive(Clone, Default)]
pub(crate) struct Hold {
	taken: Arc<AtomicBool>,
	/// The hold of the authority this one runs under (`Access::under_lease`),
	/// whose lock lasts as long as this transaction does.
	inherited: Option<Box<Hold>>,
}

impl Hold {
	/// The transaction has taken the lock, shared or exclusive.
	pub(crate) fn take(&self) {
		self.taken.store(true, Ordering::SeqCst);
	}

	/// A savepoint rollback may have released the lock taken after its savepoint.
	pub(crate) fn release(&self) {
		self.taken.store(false, Ordering::SeqCst);
	}

	/// Count the hold of the authority this one runs under as its own.
	pub(crate) fn inherit(&mut self, parent: Hold) {
		self.inherited = Some(Box::new(parent));
	}

	pub(crate) fn held(&self) -> bool {
		self.taken.load(Ordering::SeqCst) || self.inherited.as_deref().is_some_and(Hold::held)
	}
}

tokio::task_local! {
	/// Set while the work of an in-process peer runs for a caller that holds the lock.
	static CALLER_HOLDS: ();
}

/// Whether the work running now serves a caller that holds the lock.
pub(crate) fn caller_holds() -> bool {
	CALLER_HOLDS.try_with(|()| ()).is_ok()
}

/// Run `work` of an in-process peer for a caller whose transaction holds `hold`.
/// Whatever the peer asks of the lock inside is covered by the caller's. A peer
/// that is itself served for a holder stays covered when it asks another Node.
pub(crate) async fn serving<F: Future>(hold: &Hold, work: F) -> F::Output {
	if hold.held() && !caller_holds() {
		CALLER_HOLDS.scope((), work).await
	} else {
		work.await
	}
}

#[cfg(test)]
mod tests;
