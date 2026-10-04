//! Lease supervision and terminal-outbox polling share the listener-free worker driver.
use aidash_application::{
	Result,
	execution::{terminal, worker},
	ports::{
		activation::ActivationRepository,
		execution::{
			terminal::TerminalRepository,
			worker::{WorkerLeases, WorkerStep},
		},
	},
};
use std::{future::Future, sync::Arc, time::Duration};
use tokio::{sync::watch, task::JoinSet};
use uuid::Uuid;

#[derive(Clone, Copy)]
enum LeaseTarget {
	Token,
	Run(Uuid),
}
enum Completion<T> {
	Completed(Result<T>),
	Lost,
}

async fn renew(leases: &dyn WorkerLeases, run: Uuid, token: Uuid, seconds: i32) -> Result<bool> {
	let mut delay = Duration::from_millis(250);
	loop {
		match leases.renew(run, token, seconds).await {
			Ok(renewed) => return Ok(renewed),
			Err(error) if leases.transient(&error) => {
				tracing::warn!(%error, run_id = %run, "retrying worker lease renewal after transient database error");
				tokio::time::sleep(delay).await;
				delay = delay.saturating_mul(2).min(Duration::from_secs(2));
			}
			Err(error) => return Err(error),
		}
	}
}

/// Losing the lease cancels the in-flight operation before any recovery or finish write.
async fn keepalive<T>(
	work: impl Future<Output = Result<T>>,
	leases: &dyn WorkerLeases,
	target: LeaseTarget,
	token: Uuid,
	seconds: i32,
) -> Result<Completion<T>> {
	let mut work = Box::pin(work);
	let mut heartbeat = tokio::time::interval(Duration::from_secs((seconds / 3).max(1) as u64));
	heartbeat.tick().await;
	loop {
		tokio::select! {
			result = &mut work => return Ok(Completion::Completed(result)),
			_ = heartbeat.tick() => {
				let run = match target {
					LeaseTarget::Token => leases.current_id(token).await?,
					LeaseTarget::Run(run) => run,
				};
				if !renew(leases, run, token, seconds).await? {
					return Ok(Completion::Lost);
				}
			}
		}
	}
}

/// Visibility must resume before recovery can read and update the committed run.
pub async fn advance(
	mut scope: Box<dyn WorkerStep>,
	leases: &dyn WorkerLeases,
	token: Uuid,
	seconds: i32,
) -> Result<()> {
	let Completion::Completed(result) = keepalive(
		worker::advance(scope.as_mut(), token),
		leases,
		LeaseTarget::Token,
		token,
		seconds,
	)
	.await?
	else {
		return Ok(());
	};
	scope.resume_visibility().await?;
	metrics::counter!("aidash_worker_steps_total", "outcome" => if result.is_ok() { "success" } else { "error" }).increment(1);
	if let Err(error) = result {
		let metadata = scope.metadata();
		tracing::warn!(run = %metadata.id, phase = metadata.phase().as_str(), error = ?error, "worker step requires recovery");
		let failure = scope.classify_failure(error);
		if aidash_application::recovery::recover(
			scope.recovery_store(),
			token,
			failure,
			chrono::Utc::now(),
		)
		.await?
		{
			metrics::counter!("aidash_worker_retries_total").increment(1);
		}
	}
	Ok(())
}

pub async fn failure_once(
	repository: &dyn TerminalRepository,
	leases: &dyn WorkerLeases,
	seconds: i32,
) -> Result<bool> {
	let token = Uuid::new_v4();
	let Some(mut scope) = repository.claim_failure(token, seconds).await? else {
		return Ok(false);
	};
	if terminal::finish_cancelled(scope.as_mut()).await? {
		return Ok(true);
	}
	let run = scope.metadata().id;
	if let Completion::Completed(result) = keepalive(
		terminal::deliver(scope.as_mut()),
		leases,
		LeaseTarget::Run(run),
		token,
		seconds,
	)
	.await?
	{
		terminal::settle(scope.as_mut(), result).await?;
	}
	Ok(true)
}

async fn terminal_once(
	repository: &dyn TerminalRepository,
	leases: &dyn WorkerLeases,
	seconds: i32,
) -> Result<bool> {
	if failure_once(repository, leases, seconds).await? {
		return Ok(true);
	}
	terminal::pending(repository).await
}

pub async fn deliver_terminal_until(
	repository: Arc<dyn TerminalRepository>,
	leases: Arc<dyn WorkerLeases>,
	seconds: i32,
	mut stopping: watch::Receiver<bool>,
) -> Result<()> {
	while !*stopping.borrow() && stopping.has_changed().is_ok() {
		match terminal_once(repository.as_ref(), leases.as_ref(), seconds).await {
			Ok(true) => continue,
			Ok(false) => {}
			Err(error) => tracing::warn!(%error, "terminal remote input delivery deferred"),
		}
		tokio::select! {
			_ = tokio::time::sleep(Duration::from_millis(250)) => {},
			_ = stopped(&mut stopping) => break,
		}
	}
	Ok(())
}

/// A failed committed-state observation never cancels an inference request.
pub async fn wait_for_cancellation(leases: &dyn WorkerLeases, run: Uuid) -> Result<()> {
	loop {
		match worker::cancellation_requested(leases, run).await {
			Ok(true) => return Ok(()),
			Ok(false) => {}
			Err(error) => {
				tracing::warn!(run_id = %run, %error, "inference cancellation poll failed; retrying")
			}
		}
		tokio::time::sleep(Duration::from_millis(250)).await;
	}
}

/// Own all background tasks while the current committed step drains on shutdown.
pub async fn run_worker(
	activation: Arc<crate::activation::Runtime>,
	repository: Arc<dyn ActivationRepository>,
	terminal: Arc<dyn TerminalRepository>,
	leases: Arc<dyn WorkerLeases>,
	seconds: i32,
	stopping: watch::Receiver<bool>,
) -> Result<()> {
	let mut background = JoinSet::new();
	background.spawn(activation.clone().run(stopping.clone()));
	background.spawn(deliver_terminal_until(
		terminal,
		leases,
		seconds,
		stopping.clone(),
	));
	let result = activation.worker(repository, stopping).await;
	background.abort_all();
	while background.join_next().await.is_some() {}
	result
}

async fn stopped(receiver: &mut watch::Receiver<bool>) {
	while !*receiver.borrow_and_update() {
		if receiver.changed().await.is_err() {
			return;
		}
	}
}

#[cfg(test)]
mod tests;
