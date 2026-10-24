//! Read-only query documents consumed by deployment acceptance runners.
mod acceptance;
mod calibration;
mod evaluation;
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
					.value_parser(["acceptance", "calibration", "evaluation", "remote-memory"])
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
			Some("calibration") => calibration::document(),
			Some("evaluation") => evaluation::document(),
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

#[cfg(test)]
mod tests {
	#[test]
	fn every_cluster_profile_includes_the_generated_pgroonga_flush_query() {
		let query = super::acceptance::pgroonga_ready();
		assert!(query.contains("pgroonga_command"));
		assert!(query.contains("io_flush"));
		for document in [
			super::acceptance::document(),
			super::remote_memory::document(),
		] {
			assert_eq!(document["pgroonga_ready"], query);
		}
	}

	#[test]
	fn calibration_pools_select_only_aggregates() {
		// Act
		let query = super::calibration::pools();
		// Assert
		assert!(query.starts_with("SELECT row_to_json(\"e\")"), "{query}");
		assert!(
			query.contains("FROM \"estimate_calibration_samples\""),
			"{query}"
		);
		assert!(query.contains("PERCENTILE_DISC(0.99)"), "{query}");
		assert!(
			query.contains("GROUP BY \"upstream_model\", \"projection_version\""),
			"{query}"
		);
		for identifier in ["run_id", "tenant", "attempt_id", "\"id\""] {
			assert!(!query.contains(identifier), "{identifier} in {query}");
		}
		assert_eq!(
			super::calibration::document()["calibration_pools"],
			serde_json::json!(query)
		);
	}

	#[test]
	fn evaluation_usage_query_is_parameterless_and_excludes_lease_tokens() {
		// Act
		let query = super::evaluation::inference_usage();
		// Assert
		assert!(query.starts_with("SELECT row_to_json(\"e\")"), "{query}");
		assert!(query.contains("FROM \"inference_usage\""), "{query}");
		assert!(!query.contains("WHERE"), "{query}");
		assert!(!query.contains(":'"), "{query}");
		assert!(
			query.contains("ORDER BY \"created_at\" ASC, \"attempt_id\" ASC"),
			"{query}"
		);
		assert!(!query.contains("lease_token"), "{query}");
		assert_eq!(
			super::evaluation::document()["inference_usage"],
			serde_json::json!(query)
		);
	}
}
