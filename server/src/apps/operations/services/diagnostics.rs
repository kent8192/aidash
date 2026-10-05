//! Read-only query documents consumed by deployment acceptance runners.
mod acceptance;
mod remote_memory;
use async_trait::async_trait;
use reinhardt::commands::{
	CapabilityCommand, CapabilityContext, CapabilityRequirement, CommandResult,
};

pub struct Diagnostics;
#[async_trait]
impl CapabilityCommand for Diagnostics {
	fn cli(&self) -> clap::Command {
		clap::Command::new("diagnostics")
			.about("Export read-only Reinhardt Query diagnostics without connecting to services")
			.arg(
				clap::Arg::new("profile")
					.value_parser(["acceptance", "remote-memory"])
					.required(true),
			)
	}
	fn requirements(&self, _: &clap::ArgMatches) -> Vec<CapabilityRequirement> {
		Vec::new()
	}
	async fn execute(
		&self,
		matches: &clap::ArgMatches,
		_: &CapabilityContext,
	) -> CommandResult<()> {
		let document = match matches.get_one::<String>("profile").map(String::as_str) {
			Some("acceptance") => acceptance::document(),
			Some("remote-memory") => remote_memory::document(),
			_ => {
				return Err(reinhardt::commands::CommandError::InvalidArguments(
					"a diagnostics profile is required".into(),
				));
			}
		};
		println!("{document}");
		Ok(())
	}
}
