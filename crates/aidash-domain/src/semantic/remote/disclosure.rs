//! A disclosed receipt budgets the complete wire representation and exact source provenance.
use super::{ContractError, Receipt, Result, SourceRead};
use crate::semantic::Failure;
impl Receipt {
	/// Drop only trailing matches. A result with candidates must never become an empty success.
	pub fn fit_budget(&mut self, budget: usize) -> Result<()> {
		let receipt = self;
		let had_matches = !receipt.result.matches.is_empty() || receipt.result.truncated;
		loop {
			receipt.result.estimated_tokens =
				crate::semantic::retrieval::result_tokens(&receipt.result)?;
			receipt.sources = receipt
				.result
				.matches
				.iter()
				.map(|m| SourceRead {
					entry_id: m.entry_id,
					revision: m.revision,
					content_digest: crate::semantic::indexing::content_digest(&m.text),
				})
				.collect();
			receipt.estimated_tokens =
				crate::context::estimated_tokens(&serde_json::to_string(receipt)?) + 16;
			if receipt.estimated_tokens <= budget {
				break;
			}
			if receipt.result.matches.pop().is_none() {
				return Err(ContractError::Semantic(Failure::ContextBudget));
			}
			receipt.result.truncated = true;
		}
		if had_matches && receipt.result.matches.is_empty() {
			return Err(ContractError::Semantic(Failure::ContextBudget));
		}
		Ok(())
	}
}
#[cfg(test)]
mod tests;
