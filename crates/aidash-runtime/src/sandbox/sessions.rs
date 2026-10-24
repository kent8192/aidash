//! Own all sandbox jobs and pending admissions through process shutdown and bounded drain.
use aidash_application::{Error, Result, registry::workbench::sandbox::execution::SessionId};
use futures_util::future::poll_fn;
use std::{
	future::Future,
	sync::{Arc, Mutex},
	task::Poll,
};
use tokio::{
	sync::{Notify, watch},
	task::JoinSet,
};
#[derive(Clone)]
pub struct Sessions {
	inner: Arc<Inner>,
}
struct Inner {
	state: Mutex<State>,
	changed: Notify,
}
struct State {
	accepting: bool,
	cancelled: bool,
	driving: bool,
	pending: usize,
	jobs: JoinSet<(SessionId, Result<()>)>,
}
impl Default for Sessions {
	fn default() -> Self {
		Self {
			inner: Arc::new(Inner {
				state: Mutex::new(State {
					accepting: true,
					cancelled: false,
					driving: false,
					pending: 0,
					jobs: JoinSet::new(),
				}),
				changed: Notify::new(),
			}),
		}
	}
}
/// A permit keeps an admission in the drain set until it launches or rolls back.
pub struct Permit {
	sessions: Sessions,
}
impl Sessions {
	pub fn permit(&self) -> Result<Permit> {
		let mut state = self.inner.state.lock().unwrap();
		while let Some(result) = state.jobs.try_join_next() {
			observe(result);
		}
		if !state.accepting {
			return Err(Error::Conflict("sandbox runtime is stopping".into()));
		}
		state.pending += 1;
		Ok(Permit {
			sessions: self.clone(),
		})
	}
	/// Close admission before draining accepted work; this is synchronous with listener shutdown.
	pub fn close(&self) {
		self.inner.state.lock().unwrap().accepting = false;
		self.inner.changed.notify_one();
	}
	fn drained(&self) -> bool {
		let state = self.inner.state.lock().unwrap();
		state.pending == 0 && state.jobs.is_empty()
	}
	async fn next(&self) -> std::result::Result<(SessionId, Result<()>), tokio::task::JoinError> {
		poll_fn(|cx| {
			let mut state = self.inner.state.lock().unwrap();
			if state.jobs.is_empty() {
				Poll::Pending
			} else {
				match state.jobs.poll_join_next(cx) {
					Poll::Ready(Some(result)) => Poll::Ready(result),
					_ => Poll::Pending,
				}
			}
		})
		.await
	}
	/// The outer Supervisor applies its shared drain deadline and cancels this owned driver.
	pub async fn run(self, mut stop: watch::Receiver<bool>) -> Result<()> {
		{
			let mut state = self.inner.state.lock().unwrap();
			if state.driving {
				return Err(Error::Conflict(
					"sandbox supervisor is already running".into(),
				));
			}
			state.driving = true;
		}
		let _driver = Driver(self.clone());
		let mut stopping = *stop.borrow();
		if stopping {
			self.close();
		}
		loop {
			if stopping && self.drained() {
				return Ok(());
			}
			tokio::select! {
			 _=stop.changed(),if !stopping=>{self.close();stopping=true;},
			 result=self.next()=>observe(result),
			 _=self.inner.changed.notified()=>{},
			}
		}
	}
}
impl Permit {
	/// The job owns application adapters, never this Sessions owner or a server runtime clone.
	pub fn spawn(
		self,
		id: SessionId,
		job: impl Future<Output = Result<()>> + Send + 'static,
	) -> Result<()> {
		{
			let mut state = self.sessions.inner.state.lock().unwrap();
			if state.cancelled {
				return Err(Error::Conflict(
					"sandbox runtime stopped before execution".into(),
				));
			}
			state.jobs.spawn(async move { (id, job.await) });
		}
		self.sessions.inner.changed.notify_one();
		Ok(())
	}
}
impl Drop for Permit {
	fn drop(&mut self) {
		self.sessions.inner.state.lock().unwrap().pending -= 1;
		self.sessions.inner.changed.notify_one();
	}
}
/// Dropping the driver aborts owned jobs even when another server handle remains alive.
struct Driver(Sessions);
impl Drop for Driver {
	fn drop(&mut self) {
		let mut state = self.0.inner.state.lock().unwrap();
		state.accepting = false;
		state.cancelled = true;
		state.driving = false;
		state.jobs.abort_all();
		self.0.inner.changed.notify_one();
	}
}
fn observe(result: std::result::Result<(SessionId, Result<()>), tokio::task::JoinError>) {
	match result {
		Ok((session_id, Err(error))) => {
			tracing::error!(%session_id,%error,"sandbox session completion failed")
		}
		Err(error) => tracing::error!(%error,"sandbox session task failed"),
		Ok((_, Ok(()))) => {}
	}
}
#[cfg(test)]
mod tests;
