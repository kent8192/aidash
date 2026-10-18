//! The Enterprise build's server. It composes the Community server library
//! and runs only the Cloud and Dedicated Offerings.
use aidash_domain::offering::Offering;
use aidash_server::bootstrap::build::Build;

pub const BUILD: Build = Build {
	name: "Enterprise",
	offerings: &[Offering::Cloud, Offering::Dedicated],
	manifest_dir: env!("CARGO_MANIFEST_DIR"),
	package: env!("CARGO_PKG_NAME"),
};
