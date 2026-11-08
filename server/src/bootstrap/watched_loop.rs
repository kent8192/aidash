//! A timed loop whose current tick is watched, so a stalled one shows (ADR 0113).
use crate::Result;
use aidash_runtime::background::Passes;
use std::{future::Future, time::Duration};

/// Run `tick` every `interval`, numbering ticks from zero, until the Supervisor
/// cancels the loop.
///
/// Each tick is one watched pass of `service`. One that stalls, such as a purge
/// on a connection whose server vanished, shows in
/// `aidash_background_service_pass_age_seconds{service}` and repeats a log line,
/// instead of holding the loop silent until restart. A tick handles its own
/// failures, so none of them ends the loop or changes the wait.
pub(super) async fn run<Tick, Pass>(
	service: &'static str,
	interval: Duration,
	mut tick: Tick,
) -> Result<()>
where
	Tick: FnMut(u32) -> Pass,
	Pass: Future<Output = ()>,
{
	let mut passes = Passes::new(service, interval);
	let mut ticks = 0_u32;
	loop {
		passes
			.run(async {
				tick(ticks).await;
				Ok::<_, aidash_application::Error>(())
			})
			.await;
		ticks = ticks.wrapping_add(1);
		passes.wait().await;
	}
}

#[cfg(test)]
mod tests;
