//! Which build a binary belongs to, and the Offerings that build runs.
//!
//! The Community build and the Enterprise build compose the same server
//! library (ADR 0032). Each binary installs its `Build` before it runs a
//! command, so that every settings load refuses another build's Offering
//! before it connects to anything.
use crate::database::classification::{self, Table};
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
	/// The Tier's limits on a Tenant (#242), installed once the Store's pools
	/// exist. Community sets none.
	fn tier_limits(
		&self,
		_store: &crate::store::Store,
	) -> crate::Result<Arc<dyn aidash_application::ports::nodes::TierLimits>> {
		Ok(Arc::new(aidash_application::ports::nodes::NoTierLimits))
	}
	/// Endpoints the build adds to Community's for `offering`.
	fn routes(&self, _offering: Offering) -> Option<reinhardt::UnifiedRouter> {
		None
	}
	/// Installs the adapters this build adds to the Store, once its pools exist.
	/// Community installs none, so its Store reports no usage and admits all work.
	fn install(&self, _store: &mut crate::store::Store) -> crate::Result<()> {
		Ok(())
	}
	/// Long-running services the build adds to Community's. Each runs until
	/// `stopping` changes to true.
	fn services(
		&self,
		_federation: &crate::federation::Federation,
		_stopping: tokio::sync::watch::Receiver<bool>,
	) -> Vec<Service> {
		Vec::new()
	}
	/// The classification of every table the build's own migrations create,
	/// which Community's list does not hold (#266). Read the combined list
	/// through [`Build::tables`].
	fn tables(&self) -> &'static [Table] {
		&[]
	}
	/// The Secret Store of an Enterprise `kind`, from its settings
	/// fields, and the Capability Token issuer of its Credential Broker.
	fn provider_credential_store(
		&self,
		store: &serde_json::Map<String, serde_json::Value>,
		broker: Option<&serde_json::Value>,
	) -> crate::Result<ProvidedStore>;
	/// The Tenant Secret call adapter of an Enterprise Store. The server role
	/// signs with the Tenant Secret broker's key; the worker role reuses its
	/// existing broker key with the Tenant Secret broker's endpoint and
	/// audience (ADR 0095).
	fn tenant_secret_calls(
		&self,
		_broker: Option<&serde_json::Value>,
		_tenant_secret_broker: Option<&serde_json::Value>,
		_worker: bool,
	) -> crate::Result<Option<Arc<dyn aidash_application::tenant_secrets::SecretCalls>>> {
		Ok(None)
	}
	/// The Deployment Key in the Cloud KMS key version `[deployment.key] kms`
	/// names, with its public key already fetched.
	async fn deployment_key(&self, kms: &str) -> crate::Result<Arc<dyn DeploymentKey>>;
}

/// One background service of a build.
pub type Service = std::pin::Pin<Box<dyn std::future::Future<Output = crate::Result<()>> + Send>>;

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
			"the Community build has no {kind} Secret Store; it needs the Enterprise build"
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

	/// Every table of this build's migrations: Community's list, then the
	/// build's own. The Tenant-owned ones are what purge deletes (ADR 0061).
	pub fn tables(&self) -> impl Iterator<Item = &'static Table> {
		classification::TABLES.iter().chain(self.extension.tables())
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
		// Hosted authorization and token custody need OpenAI's approved contract
		// (#248). A PostgreSQL Store does not make a hosted flow permissible.
		if offering != Offering::SelfHosted
			&& settings
				.optional_path::<bool>(&["plan_connections", "enabled"])?
				.unwrap_or(false)
		{
			return Err(BuildError::Validation(
				"hosted Plan Connections are disabled pending OpenAI approval (#248); \
				 turn off [plan_connections] enabled"
					.into(),
			));
		}
		let _ = OFFERING.set(offering);
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::{Build, Extension, ProvidedStore};
	use crate::database::classification::{
		Classification::{DeploymentWide, TenantOwned},
		TABLES, Table, table,
	};
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
			"the Community build has no secret_manager Secret Store; it needs the Enterprise build"
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

	const EXTRA: &[Table] = &[
		table("extra", "extra_notes", DeploymentWide),
		table("extra", "extra_rows", TenantOwned),
	];

	#[derive(Debug)]
	struct Extra;

	#[async_trait::async_trait]
	impl Extension for Extra {
		fn tables(&self) -> &'static [Table] {
			EXTRA
		}
		async fn deployment_key(
			&self,
			_: &str,
		) -> crate::Result<std::sync::Arc<dyn super::DeploymentKey>> {
			unreachable!("no Deployment Key in this test")
		}
		fn provider_credential_store(
			&self,
			_: &serde_json::Map<String, serde_json::Value>,
			_: Option<&serde_json::Value>,
		) -> crate::Result<ProvidedStore> {
			unreachable!("no Secret Store in this test")
		}
	}

	#[test]
	fn the_community_build_lists_only_communitys_tables() {
		assert!(Build::COMMUNITY.extension.tables().is_empty());
		assert!(Build::COMMUNITY.tables().eq(TABLES.iter()));
	}

	#[test]
	fn a_build_lists_its_own_tables_after_communitys() {
		let build = Build {
			extension: &Extra,
			..Build::COMMUNITY
		};
		let listed = build.tables().collect::<Vec<_>>();
		assert_eq!(listed.len(), TABLES.len() + EXTRA.len());
		assert!(listed[..TABLES.len()].iter().copied().eq(TABLES.iter()));
		assert!(listed[TABLES.len()..].iter().copied().eq(EXTRA.iter()));
	}
}
