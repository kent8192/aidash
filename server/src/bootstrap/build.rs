//! Which build a binary belongs to, and the Offerings that build runs.
//!
//! The Community build and the Enterprise build compose the same server
//! library (ADR 0032). Each binary installs its `Build` before it runs a
//! command, so that every settings load refuses another build's Offering
//! before it connects to anything.
use aidash_domain::offering::Offering;
use reinhardt::conf::settings::builder::BuildError;
use reinhardt::conf::settings::scoped::ScopedSettings;
use std::sync::OnceLock;

static INSTALLED: OnceLock<&'static Build> = OnceLock::new();

/// Make `build` the build of this process. Only the first call counts.
pub fn install(build: &'static Build) {
	let _ = INSTALLED.set(build);
}

/// The installed build. A process that installed none, such as a test of
/// this library, is the Community build.
pub fn current() -> &'static Build {
	INSTALLED.get().copied().unwrap_or(&Build::COMMUNITY)
}

#[derive(Debug, Clone, Copy)]
pub struct Build {
	pub name: &'static str,
	pub offerings: &'static [Offering],
	/// The launcher package, for Reinhardt's Cargo checks.
	pub manifest_dir: &'static str,
	pub package: &'static str,
}

impl Build {
	pub const COMMUNITY: Self = Self {
		name: "Community",
		offerings: &[Offering::SelfHosted],
		manifest_dir: env!("CARGO_MANIFEST_DIR"),
		package: env!("CARGO_PKG_NAME"),
	};

	pub fn admit(&self, offering: Offering) -> Result<(), BuildError> {
		if self.offerings.contains(&offering) {
			return Ok(());
		}
		let accepted = self
			.offerings
			.iter()
			.map(|offering| format!("\"{offering}\""))
			.collect::<Vec<_>>()
			.join(" or ");
		Err(BuildError::Validation(format!(
			"the {} build does not run the {offering} Offering; set [deployment] offering to {accepted}",
			self.name
		)))
	}

	/// Refuse settings whose `[deployment] offering`, or its default, is not
	/// this build's.
	pub fn check(&self, settings: &ScopedSettings) -> Result<(), BuildError> {
		let offering = settings
			.optional_path::<Offering>(&["deployment", "offering"])?
			.unwrap_or_default();
		self.admit(offering)
	}
}

#[cfg(test)]
mod tests {
	use super::Build;
	use aidash_domain::offering::Offering::{self, Cloud, Dedicated, SelfHosted};
	use rstest::rstest;

	const ENTERPRISE: Build = Build {
		name: "Enterprise",
		offerings: &[Cloud, Dedicated],
		..Build::COMMUNITY
	};

	#[rstest]
	#[case(Build::COMMUNITY, SelfHosted, None)]
	#[case(
		Build::COMMUNITY,
		Cloud,
		Some(
			"the Community build does not run the cloud Offering; set [deployment] offering to \"self-hosted\""
		)
	)]
	#[case(
		Build::COMMUNITY,
		Dedicated,
		Some(
			"the Community build does not run the dedicated Offering; set [deployment] offering to \"self-hosted\""
		)
	)]
	#[case(
		ENTERPRISE,
		SelfHosted,
		Some(
			"the Enterprise build does not run the self-hosted Offering; set [deployment] offering to \"cloud\" or \"dedicated\""
		)
	)]
	#[case(ENTERPRISE, Cloud, None)]
	#[case(ENTERPRISE, Dedicated, None)]
	fn each_build_runs_only_its_offerings(
		#[case] build: Build,
		#[case] offering: Offering,
		#[case] refusal: Option<&str>,
	) {
		let actual = build.admit(offering).err().map(|error| error.to_string());
		assert_eq!(
			actual,
			refusal.map(|message| format!("Validation error: {message}"))
		);
	}
}
