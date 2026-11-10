//! Durable background extraction adapter for compaction's offloaded ranges.
use super::*;
use aidash_application::ports::memory::offloads::{BackgroundExtraction, JournalRange};
use async_trait::async_trait;
use std::sync::Arc;

/// Queue a persisted batch outside the foreground Run step.
pub(crate) fn schedule(
	queue: Arc<dyn BackgroundExtraction>,
	run: Uuid,
	ranges: Vec<aidash_application::context::offload::JournalRange>,
) -> tokio::task::JoinHandle<()> {
	tokio::spawn(async move {
		for range in ranges {
			let proof = JournalRange {
				run,
				from: range.from_seq,
				through: range.through_seq,
			};
			if let Err(error) = queue.enqueue(proof).await {
				tracing::warn!(run_id = %run, from_seq = range.from_seq, through_seq = range.through_seq, %error, "background memory extraction enqueue failed");
			}
		}
	})
}

pub(crate) struct OffloadQueue {
	pub(crate) store: Store,
}

#[async_trait]
impl BackgroundExtraction for OffloadQueue {
	async fn enqueue(&self, range: JournalRange) -> aidash_application::Result<()> {
		let proof = range.evidence()?;
		let store = &self.store;
		let result: Result<()> = async {
			let run: aidash_domain::Run = crate::database::query_as(
				&Query::select()
					.column(ColumnRef::Asterisk)
					.from(Alias::new("runs"))
					.and_where(Expr::col("id").eq(Expr::value(range.run)))
					.to_string(PostgresQueryBuilder),
			)
			.fetch_one(store.pool.in_scope(&store.run_tenancy(range.run).await?))
			.await?;
			if !run.is_local(store.deployment.as_str()) {
				return Err(Error::Forbidden);
			}
			let mut lease =
				match crate::authorization::execution::access_for_run(store, &run.metadata(), true)
					.await?
				{
					Some(access) => Lease::Scoped(Box::new(access)),
					None => {
						Lease::within(store, &store.workspace_tenancy(run.workspace_id).await?)
							.await?
					}
				};
			let result = async {
				repository::lock_workspace(&mut lease, run.workspace_id, true).await?;
				let binding = super::super::bindings::for_run(store, &mut lease, &run)
					.await?
					.ok_or(Error::Forbidden)?;
				if binding.provider.id
					!= aidash_domain::registry::bindings::memory_defaults::MEMORY_ID
					|| binding.provider.version
						!= aidash_domain::registry::bindings::memory_defaults::VERSION
				{
					return Err(Error::Forbidden);
				}
				let policy = crate::semantic::native_memory::policy(
					&mut lease,
					&binding.bank.tenant,
					&binding.provider,
				)
				.await?;
				units::current(
					&mut lease,
					binding.bank.workspace,
					std::slice::from_ref(&proof),
					policy.bounds.max_graph_visits,
				)
				.await?;
				let id = crate::semantic::native_memory::request_id(
					range.run,
					&format!("offloaded:{}:{}", range.from, range.through),
				)?;
				super::enqueue(
					&mut lease,
					&binding.bank,
					&binding.provider,
					&policy,
					id,
					Input::OffloadedRange {
						run: range.run,
						from: range.from,
						through: range.through,
					},
					"pending",
					None,
				)
				.await?;
				Ok(())
			}
			.await;
			lease.finish(result).await
		}
		.await;
		result.map_err(Into::into)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::Mutex;

	#[derive(Default)]
	struct Queue(Mutex<Vec<JournalRange>>);
	#[async_trait]
	impl BackgroundExtraction for Queue {
		async fn enqueue(&self, range: JournalRange) -> aidash_application::Result<()> {
			self.0.lock().unwrap().push(range);
			if range.from == 1 {
				Err(aidash_application::Error::Forbidden)
			} else {
				Ok(())
			}
		}
	}

	#[tokio::test]
	async fn persisted_offload_batch_is_enqueued_and_failures_do_not_stop_later_ranges() {
		let queue = Arc::new(Queue::default());
		let run = Uuid::now_v7();
		let ranges = vec![
			aidash_application::context::offload::JournalRange {
				from_seq: 1,
				through_seq: 2,
			},
			aidash_application::context::offload::JournalRange {
				from_seq: 5,
				through_seq: 7,
			},
		];
		schedule(queue.clone(), run, ranges).await.unwrap();
		assert_eq!(
			*queue.0.lock().unwrap(),
			vec![
				JournalRange {
					run,
					from: 1,
					through: 2
				},
				JournalRange {
					run,
					from: 5,
					through: 7
				}
			]
		);
	}
}
