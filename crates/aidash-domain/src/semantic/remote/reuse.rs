//! A remote Ordered Run reuses one semantic retrieval across steps (ADR 0020).
//! A reuse step creates no semantic operation, so its usage admission cites the
//! operation whose retrieval it reuses. Each node judges the citation against
//! its own journal; neither ever trusts the citing peer's account of it.
use uuid::Uuid;

/// What a node's journal holds for a cited semantic operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CitedOperation {
	pub admission_id: Uuid,
	pub grant_id: Uuid,
	pub ready: bool,
	/// The Run step whose boundary the operation was retrieved at.
	pub step: i32,
	/// The Home corpus digest read before the retrieval. Only the Home records
	/// one; an operation without it can never prove that the corpus is unchanged.
	pub corpus_digest: Option<String>,
}

/// Why a node refuses a citation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
	/// The operation belongs to another Run or grant, is not READY or was
	/// retrieved at a later step. Such a citation never authorizes anything.
	Foreign,
	/// The Home corpus changed after the retrieval. The Run retrieves again.
	Stale,
}

impl CitedOperation {
	/// Both nodes: the operation is READY and was retrieved for this Run and
	/// grant at this step or an earlier one.
	pub fn authorizes(&self, admission: Uuid, grant: Uuid, step: i32) -> Result<(), Refusal> {
		if self.admission_id != admission
			|| self.grant_id != grant
			|| !self.ready
			|| self.step > step
		{
			return Err(Refusal::Foreign);
		}
		Ok(())
	}

	/// The Home: the Workspace corpus still has the digest recorded before the
	/// retrieval, so the reused result is the one a new retrieval would draw from.
	pub fn current(&self, corpus_digest: &str) -> Result<(), Refusal> {
		if self.corpus_digest.as_deref() != Some(corpus_digest) {
			return Err(Refusal::Stale);
		}
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use rstest::rstest;

	const RUN: Uuid = Uuid::from_u128(1);
	const GRANT: Uuid = Uuid::from_u128(2);

	fn cited() -> CitedOperation {
		CitedOperation {
			admission_id: RUN,
			grant_id: GRANT,
			ready: true,
			step: 1,
			corpus_digest: Some("corpus-1".into()),
		}
	}

	#[rstest]
	#[case::same_step(1)]
	#[case::later_step(4)]
	fn a_ready_operation_of_this_run_authorizes_its_step_and_later_ones(#[case] step: i32) {
		assert_eq!(cited().authorizes(RUN, GRANT, step), Ok(()));
	}

	#[rstest]
	#[case::another_run(|c: &mut CitedOperation| c.admission_id = Uuid::from_u128(9))]
	#[case::another_grant(|c: &mut CitedOperation| c.grant_id = Uuid::from_u128(9))]
	#[case::not_ready(|c: &mut CitedOperation| c.ready = false)]
	#[case::future_step(|c: &mut CitedOperation| c.step = 3)]
	fn a_foreign_operation_never_authorizes(#[case] change: fn(&mut CitedOperation)) {
		let mut operation = cited();
		change(&mut operation);
		assert_eq!(operation.authorizes(RUN, GRANT, 2), Err(Refusal::Foreign));
	}

	#[rstest]
	#[case::unchanged("corpus-1", Some("corpus-1"), Ok(()))]
	#[case::changed("corpus-2", Some("corpus-1"), Err(Refusal::Stale))]
	#[case::never_recorded("corpus-1", None, Err(Refusal::Stale))]
	fn only_an_unchanged_corpus_keeps_the_retrieval_current(
		#[case] current: &str,
		#[case] recorded: Option<&str>,
		#[case] expected: Result<(), Refusal>,
	) {
		let operation = CitedOperation {
			corpus_digest: recorded.map(str::to_owned),
			..cited()
		};
		assert_eq!(operation.current(current), expected);
	}
}
