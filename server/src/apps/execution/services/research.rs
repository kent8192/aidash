//! Research provider packages bound into Runs. The Node registers which
//! providers are available; every bound operation runs the same host-owned
//! research use case with this Node's evidence store and disclosure gate.
use crate::{
	Error, Result,
	domain::Run,
	provider::ToolSpec,
	tool::{Tool, ToolContext},
};
use aidash_application::{
	capabilities::approvals::DisclosureRequest,
	ports::research::{
		DisclosureGate, DisclosureState, PageProvider, SearchProvider, WebDisclosure,
	},
	research::{Call, Research},
};
use aidash_domain::{
	research::{Operation, account::AccountProfile, evidence::Limits},
	tool::{
		ToolContract,
		providers::{ToolDescriptor, research_contract, research_operation},
	},
};
use aidash_integrations::research::{
	brave::BraveSearch, fixture::FixtureResearch, page::PageReader,
};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::{Arc, OnceLock};
use uuid::Uuid;

/// Path of the Search account holder's Brave account profile (JSON).
pub const BRAVE_PROFILE_ENV: &str = "AIDASH_RESEARCH_BRAVE_PROFILE";
/// `enabled` admits the public page reading provider.
pub const PAGE_READING_ENV: &str = "AIDASH_RESEARCH_PAGE_READING";
/// `enabled` admits the deterministic fixture provider (tests and evaluation).
pub const FIXTURE_ENV: &str = "AIDASH_RESEARCH_FIXTURE";

/// Research providers this Node implements, read once from its environment.
pub struct Providers {
	brave: std::result::Result<Arc<BraveSearch>, String>,
	pages: Option<Arc<PageReader>>,
	fixture: Option<Arc<FixtureResearch>>,
	pub limits: Limits,
}

impl Providers {
	pub fn from_env() -> Self {
		Self {
			brave: brave_from_env(),
			pages: (std::env::var(PAGE_READING_ENV).as_deref() == Ok("enabled"))
				.then(|| Arc::new(PageReader::default())),
			fixture: (std::env::var(FIXTURE_ENV).as_deref() == Ok("enabled"))
				.then(|| Arc::new(FixtureResearch::default())),
			limits: Limits::default(),
		}
	}

	/// Whether `descriptor` names a research provider this Node can dispatch.
	pub fn available(&self, descriptor: &ToolDescriptor) -> aidash_application::Result<()> {
		let unavailable = |reason: &str| {
			aidash_application::Error::Invalid(format!(
				"PROVIDER_UNAVAILABLE: {} {reason}",
				descriptor.provider
			))
		};
		match descriptor.provider.as_str() {
			"research.brave@1" => self
				.brave
				.as_ref()
				.map(|_| ())
				.map_err(|reason| unavailable(reason)),
			"research.page@1" if self.pages.is_none() => {
				Err(unavailable(&format!("requires {PAGE_READING_ENV}=enabled")))
			}
			"research.fixture@1" if self.fixture.is_none() => {
				Err(unavailable(&format!("requires {FIXTURE_ENV}=enabled")))
			}
			"research.page@1" | "research.fixture@1" => Ok(()),
			_ => Err(unavailable("is not a research provider")),
		}
	}

	fn search(&self, provider: &str) -> Result<Arc<dyn SearchProvider>> {
		match provider {
			"research.brave@1" => self
				.brave
				.clone()
				.map(|brave| brave as Arc<dyn SearchProvider>)
				.map_err(|_| unavailable(provider)),
			"research.fixture@1" => self
				.fixture
				.clone()
				.map(|fixture| fixture as Arc<dyn SearchProvider>)
				.ok_or_else(|| unavailable(provider)),
			_ => Err(unavailable(provider)),
		}
	}

	fn pages(&self, provider: &str) -> Result<Arc<dyn PageProvider>> {
		match provider {
			"research.page@1" => self
				.pages
				.clone()
				.map(|pages| pages as Arc<dyn PageProvider>)
				.ok_or_else(|| unavailable(provider)),
			"research.fixture@1" => self
				.fixture
				.clone()
				.map(|fixture| fixture as Arc<dyn PageProvider>)
				.ok_or_else(|| unavailable(provider)),
			_ => Err(unavailable(provider)),
		}
	}
}

fn unavailable(provider: &str) -> Error {
	Error::Invalid(format!("PROVIDER_UNAVAILABLE: {provider}"))
}

fn brave_from_env() -> std::result::Result<Arc<BraveSearch>, String> {
	let path = match std::env::var(BRAVE_PROFILE_ENV) {
		Ok(path) if !path.is_empty() => path,
		_ => return Err(format!("requires {BRAVE_PROFILE_ENV}")),
	};
	let profile: AccountProfile = std::fs::read(&path)
		.ok()
		.and_then(|bytes| serde_json::from_slice(&bytes).ok())
		.ok_or_else(|| "has an unreadable account profile".to_owned())?;
	let token = crate::config::secret(&profile.credential_env)
		.map_err(|_| "has no configured tool secret".to_owned())?;
	BraveSearch::new(profile, &token)
		.map(Arc::new)
		.map_err(|_| "has unavailable account terms".to_owned())
}

/// The Node's Research evidence store over `store`.
pub fn evidence_store(
	store: crate::store::Store,
) -> impl aidash_application::ports::research::EvidenceStore {
	crate::apps::execution::repositories::research::Evidence { store }
}

/// The Node's research providers. Configuration changes need a restart.
pub fn providers() -> &'static Providers {
	static PROVIDERS: OnceLock<Providers> = OnceLock::new();
	PROVIDERS.get_or_init(Providers::from_env)
}

/// A bound research operation of one provider package.
pub(crate) struct ResearchTool {
	provider: String,
	operation: Operation,
}

impl ResearchTool {
	pub(crate) fn bind(descriptor: &ToolDescriptor) -> Option<Self> {
		let (operation, _) = research_operation(&descriptor.provider, &descriptor.operation)?;
		Some(Self {
			provider: descriptor.provider.clone(),
			operation,
		})
	}
}

/// Web disclosure approval through the worker's capability approvals.
struct Gate<'a> {
	context: &'a ToolContext,
}

#[async_trait]
impl DisclosureGate for Gate<'_> {
	async fn admit(
		&self,
		run: Uuid,
		disclosure: &WebDisclosure,
	) -> aidash_application::Result<DisclosureState> {
		// Without local authority there is no approval flow: fail closed.
		let Some(authority) = &self.context.home.authority else {
			return Ok(DisclosureState::Denied);
		};
		let request = DisclosureRequest {
			digest: disclosure.digest(run),
			operation: disclosure.operation,
			provider: disclosure.provider.clone(),
			destination: disclosure.outbound.destination.clone(),
			content: disclosure.outbound.content.clone(),
			context_revision: disclosure.context_revision,
		};
		authority
			.web_disclosure(&self.context.store, &self.context.run, &request)
			.await
			.map_err(Into::into)
	}
	async fn consume(&self, _: Uuid, approval_id: Uuid) -> aidash_application::Result<bool> {
		let Some(authority) = &self.context.home.authority else {
			return Ok(false);
		};
		authority
			.consume_web_disclosure(&self.context.store, &self.context.run, approval_id)
			.await
			.map_err(Into::into)
	}
}

#[async_trait]
impl Tool for ResearchTool {
	fn specification(&self) -> ToolSpec {
		aidash_application::research::specification(self.operation)
	}
	fn contract(&self) -> ToolContract {
		research_contract(self.operation)
	}
	fn replay_safe(&self) -> bool {
		self.contract().replay_safe()
	}
	async fn invoke(&self, context: &ToolContext, input: Value, key: &str) -> Result<Value> {
		let providers = providers();
		let store = crate::apps::execution::repositories::research::Evidence {
			store: context.store.clone(),
		};
		let gate = Gate { context };
		let research = Research {
			store: &store,
			gate: &gate,
			limits: providers.limits,
		};
		let run: &Run = &context.run;
		let call = Call {
			run: run.id,
			context_revision: run.observed_input_seq,
			key,
			provider: &self.provider,
			disclosure_approval: self.contract().behavior.disclosure_approval,
		};
		let result = match self.operation {
			Operation::Search => {
				research
					.search(&call, providers.search(&self.provider)?.as_ref(), &input)
					.await
			}
			Operation::Fetch => {
				research
					.fetch(&call, providers.pages(&self.provider)?.as_ref(), &input)
					.await
			}
			Operation::Find => research.find(&call, &input).await,
		};
		result.map_err(Into::into)
	}
}
