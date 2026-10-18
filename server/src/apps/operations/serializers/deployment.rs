//! `[deployment]`: settings of the whole Deployment rather than of one Node.
use aidash_domain::offering::Offering;
use reinhardt::settings;

#[settings(fragment = true, section = "deployment")]
pub struct DeploymentSettings {
	/// The Offering this Deployment runs. Each build accepts only its own
	/// Offerings, so Cloud and Dedicated must state theirs.
	#[setting(default = "Offering::SelfHosted", leaf)]
	pub offering: Offering,
}
