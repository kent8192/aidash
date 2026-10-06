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
			let encoded = serde_json::to_string(receipt)?;
			receipt.estimated_tokens = if receipt.binding.native().is_some() {
				encoded.len() + 16
			} else {
				crate::context::estimated_tokens(&encoded) + 16
			};
			if receipt.estimated_tokens <= budget {
				break;
			}
			if receipt.result.matches.pop().is_none() {
				let mut removed = false;
				if let Some(memory) = receipt.memory.as_mut() {
					for bank in memory.banks.iter_mut().rev() {
						if let crate::memory::Recall::Ready { units } = &mut bank.recall {
							removed = units.pop().is_some();
							if units.is_empty() {
								bank.recall = crate::memory::Recall::NoSpace;
							}
							if removed {
								break;
							}
						}
					}
				}
				if !removed {
					return Err(ContractError::Semantic(Failure::ContextBudget));
				}
			} else {
				receipt.result.truncated = true;
			}
		}
		if had_matches && receipt.result.matches.is_empty() {
			return Err(ContractError::Semantic(Failure::ContextBudget));
		}
		Ok(())
	}

	pub fn validate_native(&self) -> Result<()> {
		let Some(binding) = self.binding.native() else {
			return if self.memory.is_none() {
				Ok(())
			} else {
				Err(ContractError::Semantic(Failure::ProviderContract))
			};
		};
		let memory = self
			.memory
			.as_ref()
			.ok_or(ContractError::Semantic(Failure::ProviderContract))?;
		if binding.banks.is_empty()
			|| binding.banks.len() > 33
			|| binding.banks.len() != memory.banks.len()
		{
			return Err(ContractError::Semantic(Failure::ProviderContract));
		}
		let mut ids = std::collections::BTreeSet::new();
		for (declared, actual) in binding.banks.iter().zip(&memory.banks) {
			if declared.cache_max_age_seconds == 0
				|| declared.cache_max_age_seconds > 315_360_000
				|| declared.cache_max_attempts == 0
				|| declared.cache_max_attempts > i32::MAX as usize
				|| declared.bank != actual.bank
				|| declared.provider.entry != actual.provider
				|| actual.bank.home != self.home_node
				|| actual.bank.tenant != self.tenant
				|| actual.bank.workspace != self.workspace_id
			{
				return Err(ContractError::Semantic(Failure::ProviderContract));
			}
			match &actual.recall {
				crate::memory::Recall::Disabled => {
					return Err(ContractError::Semantic(Failure::ProviderContract));
				}
				crate::memory::Recall::Ready { units } => {
					if units.is_empty() {
						return Err(ContractError::Semantic(Failure::ProviderContract));
					}
					for unit in units {
						if unit.bank != actual.bank
							|| !unit.visible() || unit.revision < 1
							|| !ids.insert(unit.id)
							|| ids.len() > 1024
						{
							return Err(ContractError::Semantic(Failure::ProviderContract));
						}
					}
				}
				_ => {}
			}
		}
		Ok(())
	}
}
#[cfg(test)]
mod tests;
