//! Personal CLI inference behind Aidash's existing Keyless contract.
//! Authentication never enters Provider Credentials or Plan Connections.
mod anthropic;
pub mod config;
pub mod protocol;
pub mod provider;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure(pub &'static str);
impl std::fmt::Display for Failure {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(self.0)
	}
}
impl std::error::Error for Failure {}
pub type Result<T> = std::result::Result<T, Failure>;

impl From<aidash_runtime::cli_child::Failure> for Failure {
	fn from(value: aidash_runtime::cli_child::Failure) -> Self {
		use aidash_runtime::cli_child::Failure as Child;
		Self(match value {
			Child::Start => "cli_start_failed",
			Child::Io => "cli_io_failed",
			Child::Timeout => "cli_timeout",
			Child::OutputLimit => "cli_output_limit",
			Child::Exit => "cli_failed",
		})
	}
}
