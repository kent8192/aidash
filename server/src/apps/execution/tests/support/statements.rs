//! Counts the SQL statements a future issues on this thread, through sqlx's own
//! statement log. A `#[tokio::test]` runs on one thread, so other tests running at
//! the same time are not counted. Each event asks this thread's recorder: tracing
//! caches a callsite's interest across threads, and a test that hit sqlx's
//! callsite while no recorder was set would otherwise switch it off for all.
use std::{
	fmt::{Debug, Write as _},
	sync::{Arc, Mutex},
};
use tracing::{
	Dispatch, Event, Metadata, Subscriber,
	field::{Field, Visit},
	level_filters::LevelFilter,
	span::{Attributes, Id, Record},
	subscriber::Interest,
};

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<String>>>);

struct Text(String);

impl Visit for Text {
	fn record_debug(&mut self, _: &Field, value: &dyn Debug) {
		let _ = write!(self.0, "{value:?} ");
	}
}

impl Subscriber for Recorder {
	fn register_callsite(&self, _: &'static Metadata<'static>) -> Interest {
		Interest::sometimes()
	}
	fn max_level_hint(&self) -> Option<LevelFilter> {
		Some(LevelFilter::TRACE)
	}
	fn enabled(&self, metadata: &Metadata<'_>) -> bool {
		metadata.target() == "sqlx::query"
	}
	fn new_span(&self, _: &Attributes<'_>) -> Id {
		Id::from_u64(1)
	}
	fn record(&self, _: &Id, _: &Record<'_>) {}
	fn record_follows_from(&self, _: &Id, _: &Id) {}
	fn event(&self, event: &Event<'_>) {
		let mut text = Text(String::new());
		event.record(&mut text);
		self.0.lock().unwrap().push(text.0);
	}
	fn enter(&self, _: &Id) {}
	fn exit(&self, _: &Id) {}
}

/// The value of `future`, with the text of every statement it issued.
#[allow(dead_code)]
pub(crate) async fn recorded<T>(future: impl Future<Output = T>) -> (T, Vec<String>) {
	let recorder = Recorder::default();
	let value = {
		let _guard = tracing::dispatcher::set_default(&Dispatch::new(recorder.clone()));
		future.await
	};
	let statements = recorder.0.lock().unwrap().clone();
	(value, statements)
}
