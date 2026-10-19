//! Which build a binary belongs to, and the Offerings that build runs.
//!
//! The Community build and the Enterprise build compose the same server
//! library (ADR 0032). Each binary installs its `Build` before it runs a
//! command, so that every settings load refuses another build's Offering
//! before it connects to anything.
use aidash_application::federation::key::DeploymentKey;
use aidash_domain::offering::Offering;
use reinhardt::conf::settings::builder::BuildError;
use reinhardt::conf::settings::scoped::ScopedSettings;
use std::sync::{Arc, OnceLock};

static INSTALLED: OnceLock<&'static Build> = OnceLock::new();
static OFFERING: OnceLock<Offering> = OnceLock::new();

/// Make `build` the build of this process. Only the first call counts.
pub fn install(build: &'static Build) {
	let _ = INSTALLED.set(build);
}

/// The installed build. A process that installed none, such as a test of
/// this library, is the Community build.
pub fn current() -> &'static Build {
	INSTALLED.get().copied().unwrap_or(&Build::COMMUNITY)
}

/// The Offering the settings check admitted, or the build's first one before
/// any settings load, which decides the endpoints this process mounts.
pub fn offering() -> Offering {
	OFFERING
		.get()
		.copied()
		.unwrap_or_else(|| current().offerings[0])
}

#[derive(Debug, Clone, Copy)]
pub struct Build {
	pub name: &'static str,
	pub offerings: &'static [Offering],
	/// The launcher package, for Reinhardt's Cargo checks.
	pub manifest_dir: &'static str,
	pub package: &'static str,
	/// The labels of the apps this build adds to Community's.
	pub apps: &'static [&'static str],
	/// The migration roots of those apps, relative to `core.base_dir` like
	/// Community's own `migrations`, so that an image mirrors the source tree.
	pub migrations: &'static [&'static str],
	/// The adapters this build installs into the server.
	pub extension: &'static dyn Extension,
}

/// What a build adds to the Community server. Community defines each hook and
/// adds nothing; an Enterprise build implements them with its own crates, so
/// Community never names them (ADR 0032).
#[async_trait::async_trait]
pub trait Extension: Sync + std::fmt::Debug {
	/// Settings sources the build adds, such as managed settings files.
	fn settings(
		&self,
		builder: reinhardt::conf::settings::builder::SettingsBuilder,
	) -> reinhardt::conf::settings::builder::SettingsBuilder {
		builder
	}
	/// The browser sign-in the build adds, read from its own settings, if
	/// they configure one. Community parses none of those settings.
	fn sign_in(
		&self,
	) -> crate::Result<Option<Arc<dyn crate::apps::identity::services::sign_in::SignInProvider>>> {
		Ok(None)
	}
	/// Whether External Identities of `issuer` sign in only through the sign-in
	/// this build adds, which also reads their Account Status. With that
	/// sign-in unconfigured, their Mappings are refused rather than left
	/// unrestricted.
	fn owns_issuer(&self, _issuer: &str) -> bool {
		false
	}
	/// Endpoints the build adds to Community's for `offering`.
	fn routes(&self, _offering: Offering) -> Option<reinhardt::UnifiedRouter> {
		None
	}
	/// The Provider Credential Store of an Enterprise `kind`, from its settings
	/// fields, and the Capability Token issuer of its Credential Broker.
	fn provider_credential_store(
		&self,
		store: &serde_json::Map<String, serde_json::Value>,
		broker: Option<&serde_json::Value>,
	) -> crate::Result<ProvidedStore>;
	/// The Deployment Key in the Cloud KMS key version `[deployment.key] kms`
	/// names, with its public key already fetched.
	async fn deployment_key(&self, kms: &str) -> crate::Result<Arc<dyn DeploymentKey>>;
}

pub struct ProvidedStore {
	pub store: Arc<dyn aidash_application::provider_credentials::Store>,
	pub issuer: Option<Arc<dyn aidash_application::provider_access::TokenIssuer>>,
}

#[derive(Debug)]
struct Community;

#[async_trait::async_trait]
impl Extension for Community {
	async fn deployment_key(&self, _: &str) -> crate::Result<Arc<dyn DeploymentKey>> {
		Err(crate::Error::Invalid(
			"the Community build has no Cloud KMS Deployment Key; name a file in [deployment.key], or use the Enterprise build"
				.into(),
		))
	}

	fn provider_credential_store(
		&self,
		store: &serde_json::Map<String, serde_json::Value>,
		_: Option<&serde_json::Value>,
	) -> crate::Result<ProvidedStore> {
		let kind = store
			.get("kind")
			.and_then(serde_json::Value::as_str)
			.unwrap_or_default();
		Err(crate::Error::Invalid(format!(
			"the Community build has no {kind} Provider Credential Store; it needs the Enterprise build"
		)))
	}
}

impl Build {
	pub const COMMUNITY: Self = Self {
		name: "Community",
		offerings: &[Offering::SelfHosted],
		manifest_dir: env!("CARGO_MANIFEST_DIR"),
		package: env!("CARGO_PKG_NAME"),
		apps: &[],
		migrations: &[],
		extension: &Community,
	};

	/// Community's installed apps followed by this build's.
	pub fn installed_apps(&self) -> Vec<&'static str> {
		crate::config::apps::APP_LABELS
			.iter()
			.chain(self.apps)
			.copied()
			.collect()
	}

	/// Community's migration root followed by this build's.
	pub fn migration_roots(&self, base_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
		std::iter::once("migrations")
			.chain(self.migrations.iter().copied())
			.map(|root| base_dir.join(root))
			.collect()
	}

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
		self.admit(offering)?;
		let _ = OFFERING.set(offering);
		Ok(())
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

	#[test]
	fn the_community_build_refuses_enterprise_adapters() {
		use aidash_domain::offering::Offering::SelfHosted;
		let extension = Build::COMMUNITY.extension;
		assert!(extension.sign_in().unwrap().is_none());
		let store = serde_json::json!({"kind": "secret_manager"});
		assert_eq!(
			extension
				.provider_credential_store(store.as_object().unwrap(), None)
				.err()
				.unwrap()
				.to_string(),
			"the Community build has no secret_manager Provider Credential Store; it needs the Enterprise build"
		);
		assert!(extension.routes(SelfHosted).is_none());
	}

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
