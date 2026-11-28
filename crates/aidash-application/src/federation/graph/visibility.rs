//! Thread permission precedes record-specific disclosure for mapped Subjects.
use crate::{Result, ports::graph::GraphVisibility};
use aidash_domain::federation::graph::Candidate;

pub async fn visible(scope: &mut dyn GraphVisibility, candidate: &Candidate) -> Result<bool> {
	let thread = match candidate {
		Candidate::Thread(row) => Some(row.id),
		Candidate::Task(row) => Some(row.thread_id),
		Candidate::Run(row) => Some(row.thread_id),
		Candidate::Conversation(row) => Some(row.thread_id),
		Candidate::Registry(_) => None,
	};
	if let Candidate::Registry(entry) = candidate {
		return scope.registry(entry, "registry.read").await;
	}
	if !scope
		.thread(
			thread.expect("non-catalog candidate has a thread"),
			"thread.read",
		)
		.await?
	{
		return Ok(false);
	}
	match candidate {
		Candidate::Thread(_) => Ok(true),
		Candidate::Conversation(row) => scope.conversation(row, "conversation.read").await,
		_ => scope.record(candidate).await,
	}
}

#[cfg(test)]
mod tests;
