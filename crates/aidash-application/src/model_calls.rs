//! Classify a provider result independently of later use of the response.
use crate::{Error, Result};
use aidash_domain::{
	context::recovery::Failure,
	model_call::Outcome,
	provider::{ModelResponse, usage::UsageOutcome},
};

pub fn outcome(result: &Result<ModelResponse>) -> Outcome {
	match result {
		Ok(_) => Outcome::Accepted,
		Err(
			Error::Forbidden
			| Error::Unauthorized
			| Error::Refused(_)
			| Error::ProviderRejected { .. }
			| Error::Context(Failure::Refused)
			| Error::TerminalResponse(Failure::Refused, _),
		) => Outcome::Refused,
		Err(_) => Outcome::Failed,
	}
}

pub fn usage(result: &Result<ModelResponse>) -> UsageOutcome {
	match result {
		Ok(response) => UsageOutcome::Completed(response.reported.clone()),
		Err(error) => match error.terminal_usage() {
			Some(response) => UsageOutcome::Completed(response.reported.clone()),
			None if outcome(result) == Outcome::Refused => UsageOutcome::Rejected {
				class: "provider_refused".into(),
			},
			None => UsageOutcome::Unknown,
		},
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_billed_refusal_keeps_provider_usage() {
		let response = ModelResponse {
			reported: aidash_domain::provider::usage::ReportedUsage {
				input_tokens: Some(15),
				..Default::default()
			},
			..Default::default()
		};
		let reported = response.reported.clone();
		let result = Err(Error::TerminalResponse(
			Failure::Refused,
			Box::new(response),
		));
		assert_eq!(outcome(&result), Outcome::Refused);
		assert_eq!(usage(&result), UsageOutcome::Completed(reported));
	}
}
