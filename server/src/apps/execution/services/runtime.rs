use crate::{
	Error, Result,
	authorization::execution::{self, Guard},
	domain::*,
	federation::{Federation, Home},
};
use std::time::Duration;
use uuid::Uuid;

#[derive(Clone)]
pub struct Harness {
	pub federation: Federation,
}
impl Harness {
	pub async fn worker_once(&self) -> Result<bool> {
		let store = &self.federation.store;
		if self.deliver_failure_once().await? {
			return Ok(true);
		}
		// Terminal runs are no longer leased, but their accepted remote inputs
		// remain in the durable outbox until home delivery is acknowledged.
		if let Some(run) = store.pending_terminal_run_message().await? {
			match self.federation.deliver_run_message_metadata(&run).await {
				Ok(()) => return Ok(true),
				Err(error) => {
					tracing::warn!(run_id=%run.id, %error, "terminal run message delivery deferred");
					store.defer_run_message_delivery(run.id).await?;
				}
			}
		}
		let visibility = crate::transactions::gate::ReadLease::begin(store).await?;
		let token = Uuid::new_v4();
		let Some(run) = store
			.lease_run(token, self.federation.config.lease_seconds)
			.await?
		else {
			return Ok(false);
		};
		self.advance_leased(run, token, visibility).await
	}

	/// The caller has committed the lease and, for notifications, its disposition.
	pub(crate) async fn advance_leased(
		&self,
		mut run: Run,
		token: Uuid,
		mut visibility: crate::transactions::gate::ReadLease,
	) -> Result<bool> {
		let store = &self.federation.store;
		let _active = crate::http::ActiveExecution::begin();
		let result = {
			let mut work = Box::pin(self.advance(&mut run, token, &mut visibility));
			let mut heartbeat = tokio::time::interval(Duration::from_secs(
				(self.federation.config.lease_seconds / 3).max(1) as u64,
			));
			heartbeat.tick().await;
			loop {
				tokio::select! {
					result=&mut work=>break result,
					_=heartbeat.tick()=>{
						if !renew_worker_lease(store, run_id(store, token).await?, token, self.federation.config.lease_seconds).await? {
							return Ok(true);
						}
					}
				}
			}
		};
		visibility.resume(store).await?;
		metrics::counter!("aidash_worker_steps_total", "outcome" => if result.is_ok() { "success" } else { "error" }).increment(1);
		if let Err(error) = result {
			tracing::warn!(run = %run.id, phase = run.phase().as_str(), error = ?error, "worker step requires recovery");
			let failure = classify_execution_failure(&error);
			let repository = crate::bootstrap::recovery_store(store);
			if aidash_application::recovery::recover(
				&repository,
				token,
				failure,
				chrono::Utc::now(),
			)
			.await?
			{
				metrics::counter!("aidash_worker_retries_total").increment(1);
			}
		}
		Ok(true)
	}
	/// Terminal input delivery remains independent of runnable-Run activation.
	pub async fn deliver_terminal_messages_until(
		&self,
		mut stopping: tokio::sync::watch::Receiver<bool>,
	) -> Result<()> {
		while !*stopping.borrow() && stopping.has_changed().is_ok() {
			let delivery = async {
				if self.deliver_failure_once().await? {
					return Ok(true);
				}
				if let Some(run) = self.federation.store.pending_terminal_run_message().await? {
					if self
						.federation
						.deliver_run_message_metadata(&run)
						.await
						.is_ok()
					{
						return Ok(true);
					}
					self.federation
						.store
						.defer_run_message_delivery(run.id)
						.await?;
				}
				Ok::<bool, Error>(false)
			}
			.await;
			match delivery {
				Ok(true) => continue,
				Ok(false) => {}
				Err(error) => tracing::warn!(%error, "terminal remote input delivery deferred"),
			}
			tokio::select! {
				_ = tokio::time::sleep(Duration::from_millis(250)) => {},
				_ = crate::lifecycle::stopped(&mut stopping) => break,
			}
		}
		Ok(())
	}
	pub async fn run_worker(&self) -> Result<()> {
		let (_sender, receiver) = tokio::sync::watch::channel(false);
		self.run_worker_until(receiver).await
	}
	/// Finish the current durable step, then stop claiming work on shutdown.
	pub async fn run_worker_until(
		&self,
		stopping: tokio::sync::watch::Receiver<bool>,
	) -> Result<()> {
		let runtime = crate::activation::Runtime::new(
			self.federation.clone(),
			crate::activation::Settings::from_env()?,
			true,
		);
		let mut background = tokio::task::JoinSet::new();
		background.spawn(runtime.clone().run(stopping.clone()));
		let delivery = self.clone();
		let delivery_stopping = stopping.clone();
		background.spawn(async move {
			delivery
				.deliver_terminal_messages_until(delivery_stopping)
				.await
		});
		let result = runtime.worker(self.clone(), stopping).await;
		background.abort_all();
		while background.join_next().await.is_some() {}
		result
	}

	async fn advance(
		&self,
		run: &mut Run,
		token: Uuid,
		visibility: &mut crate::transactions::gate::ReadLease,
	) -> Result<()> {
		if execution::cancel_if_scoped(&self.federation.store, run, token).await? {
			return Ok(());
		}
		let guard = Guard::begin(&self.federation, run).await?;
		let environment =
			crate::bootstrap::execution_environment(&self.federation, guard.as_ref(), run);
		let mut visibility = crate::apps::execution::repositories::agent::Visibility {
			visibility,
			store: &self.federation.store,
		};
		let result = aidash_application::agent::Executor::new(&environment)
			.advance(run, token, &mut visibility)
			.await
			.map_err(Error::from);
		if let Some(guard) = guard {
			guard.finish(result).await
		} else {
			result
		}
	}
}

pub(crate) async fn wait_for_inference_cancellation(
	store: &crate::store::Store,
	id: Uuid,
) -> Result<()> {
	use reinhardt::query::{Alias, Expr, PostgresQueryBuilder, Query};

	// Read committed control outside the step's authority lease. Notify is
	// process-local, and a long worker heartbeat must not delay cancellation.
	// Fetch only control rather than repeatedly copying the run's context.
	let query = Query::select()
		.column(Alias::new("control"))
		.from(Alias::new("runs"))
		.and_where(
			reinhardt::query::SimpleExpr::from(Expr::col(Alias::new("id"))).eq(Expr::cust("$1")),
		)
		.to_string(PostgresQueryBuilder);
	loop {
		let control = crate::database::query_as::<RunControl>(&query)
			.bind(id)
			.fetch_one(&store.pool)
			.await;
		match control {
			Ok(RunControl::Cancelled) => return Ok(()),
			Ok(RunControl::Active | RunControl::Paused) => {}
			Err(error) => {
				// A failed observation is not cancellation. Keep the in-flight
				// request alive; the existing heartbeat still fences the lease.
				tracing::warn!(
					run_id = %id,
					error = %error,
					"inference cancellation poll failed; retrying"
				);
			}
		}
		tokio::time::sleep(Duration::from_millis(250)).await;
	}
}

async fn renew_worker_lease(
	store: &crate::store::Store,
	run_id: Uuid,
	token: Uuid,
	lease_seconds: i32,
) -> Result<bool> {
	let mut delay = Duration::from_millis(250);
	loop {
		match store.renew_lease(run_id, token, lease_seconds).await {
			Ok(renewed) => return Ok(renewed),
			Err(error)
				if matches!(&error, Error::TransactionPending) || error.is_transient_database() =>
			{
				tracing::warn!(%error, run_id = %run_id, "retrying worker lease renewal after transient database error");
				tokio::time::sleep(delay).await;
				delay = delay.saturating_mul(2).min(Duration::from_secs(2));
			}
			Err(error) => return Err(error),
		}
	}
}

// Reserve the suffix before truncating, including at a UTF-8 boundary.

#[cfg(test)]
fn retryable_inference_error(error: &Error) -> bool {
	classify_execution_failure(error).retryable()
}

#[cfg(test)]
#[path = "../tests/services_runtime_review_tests.rs"]
mod review_tests;

impl Harness {
	async fn deliver_failure_once(&self) -> Result<bool> {
		let store = &self.federation.store;
		let token = Uuid::new_v4();
		let _visibility = crate::transactions::gate::ReadLease::begin(store).await?;
		let Some(delivery) = store
			.claim_failure_delivery(token, self.federation.config.lease_seconds)
			.await?
		else {
			return Ok(false);
		};
		if delivery.metadata.control == RunControl::Cancelled
			&& crate::authorization::peer::admission::run_grant(store, &delivery.metadata)
				.await?
				.is_some()
		{
			store
				.finish_failure_delivery(&delivery, token, Ok(TaskStatus::Cancelled))
				.await?;
			return Ok(true);
		}
		let work = async {
			let guard =
				execution::DeliveryGuard::begin(&self.federation, &delivery.metadata).await?;
			let result = async {
				self.federation
					.deliver_run_message_metadata(&delivery.metadata)
					.await?;
				let home = Home::for_delivery(self.federation.clone(), delivery.metadata.clone())
					.with_authority(
						guard
							.as_ref()
							.and_then(execution::DeliveryGuard::local_authority),
					);
				let task = home.task().await?;
				if task.status.is_terminal() {
					return Ok(task.status);
				}
				Ok(self
					.federation
					.transition_terminal_metadata(
						&delivery.metadata,
						delivery.target.task_status(),
						guard
							.as_ref()
							.and_then(execution::DeliveryGuard::local_authority),
					)
					.await?
					.status)
			}
			.await;
			if let Some(guard) = guard {
				guard.finish(result).await
			} else {
				result
			}
		};
		let result = {
			let mut work = Box::pin(work);
			let mut heartbeat = tokio::time::interval(Duration::from_secs(
				(self.federation.config.lease_seconds / 3).max(1) as u64,
			));
			heartbeat.tick().await;
			loop {
				tokio::select! {
					result = &mut work => break result,
					_ = heartbeat.tick() => {
						if !renew_worker_lease(store, delivery.metadata.id, token, self.federation.config.lease_seconds).await? { return Ok(true); }
					}
				}
			}
		};
		if let Err(
			error @ (Error::Forbidden | Error::Unauthorized | Error::IdentityStatusUnavailable),
		) = &result
		{
			store
				.pause_for_execution(
					&delivery.metadata,
					token,
					if matches!(error, Error::IdentityStatusUnavailable) {
						"identity status unavailable"
					} else {
						"execution authority denied"
					},
					"run.authorization_blocked",
				)
				.await?;
			return Ok(true);
		}
		store
			.finish_failure_delivery(&delivery, token, result)
			.await?;
		Ok(true)
	}
}
use reinhardt::query::{ExprTrait as _, QueryStatementBuilder as _};

pub(crate) async fn run_id(store: &crate::store::Store, token: Uuid) -> Result<Uuid> {
	{
		let query_bind_1 = token;
		sqlx::query_scalar(
			&reinhardt::query::Query::select()
				.expr(reinhardt::query::SimpleExpr::from(
					reinhardt::query::Expr::col(reinhardt::query::Alias::new("id")),
				))
				.from(reinhardt::query::Alias::new("runs"))
				.and_where(SimpleExpr::CustomWithExpr(
					"(lease_owner = ?)".to_owned(),
					vec![Expr::value(query_bind_1.to_owned()).into()],
				))
				.to_string(reinhardt::query::PostgresQueryBuilder),
		)
		.fetch_optional(&store.pool)
		.await?
	}
	.ok_or_else(|| Error::Conflict("worker lease lost".into()))
}

use reinhardt::query::SimpleExpr;

use reinhardt::query::Expr;

// Convert use-case errors at the native adapter boundary.

fn classify_execution_failure(error: &Error) -> aidash_application::recovery::ExecutionFailure {
	use aidash_application::recovery::ExecutionFailure as Failure;
	match error {
		Error::RemoteSemantic(reason) => Failure::Semantic(*reason),
		Error::Forbidden | Error::Unauthorized => Failure::Authority {
			identity_unavailable: false,
		},
		Error::IdentityStatusUnavailable => Failure::Authority {
			identity_unavailable: true,
		},
		Error::MediaRouteUnavailable(_) => Failure::MediaRoute(error.to_string()),
		Error::TransactionPending | Error::StaleInference => Failure::Deferred,
		Error::External(_) => Failure::Inference {
			transport: true,
			status: None,
			message: error.to_string(),
		},
		Error::ProviderRejected { status, .. } => Failure::Inference {
			transport: false,
			status: Some(*status),
			message: error.to_string(),
		},
		_ => Failure::Other(error.to_string()),
	}
}
