//! Run-scoped research: durable disclosure, accounting and delivered evidence.
pub(crate) mod accounting;
pub mod api;
pub mod contracts;
pub(crate) mod evidence;
pub(crate) mod extraction;
pub mod maintenance;
pub(crate) mod network;
pub(crate) mod persistence;
pub(crate) mod service;
pub(crate) mod tools;

use crate::{Error, Result, web_search::AccountProfile};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Profile {
	pub admission: bool,
	pub allowed_domains: Vec<String>,
	pub denied_domains: Vec<String>,
	pub search_attempt_limit: u64,
	pub page_attempt_limit: u64,
}
impl Default for Profile {
	fn default() -> Self {
		Self {
			admission: false,
			allowed_domains: vec![],
			denied_domains: vec![],
			search_attempt_limit: 10,
			page_attempt_limit: 20,
		}
	}
}
impl Profile {
	pub(crate) fn validate(&self) -> Result<()> {
		if self.allowed_domains.len() > 5
			|| self.denied_domains.len() > 5
			|| self
				.allowed_domains
				.iter()
				.chain(&self.denied_domains)
				.any(|domain| !crate::web_search::valid_domain(domain))
			|| !(1..=10).contains(&self.search_attempt_limit)
			|| !(1..=20).contains(&self.page_attempt_limit)
		{
			return Err(Error::Invalid("invalid web research profile limits".into()));
		}
		Ok(())
	}
}

#[derive(Debug, Clone)]
pub struct Runtime {
	pub profile: Arc<Profile>,
	pub account: Option<Arc<AccountProfile>>,
}
impl Default for Runtime {
	fn default() -> Self {
		Self::new(Profile::default(), None)
	}
}
impl Runtime {
	pub fn new(profile: Profile, account: Option<AccountProfile>) -> Self {
		Self {
			profile: Arc::new(profile),
			account: account.map(Arc::new),
		}
	}
	pub(crate) fn from_env() -> Result<Self> {
		let profile = match std::env::var("AIDASH_WEB_PROFILE") {
			Ok(path) => serde_json::from_slice(&std::fs::read(path)?)
				.map_err(|_| Error::Invalid("invalid web research profile".into()))?,
			Err(std::env::VarError::NotPresent) => Profile::default(),
			Err(_) => return Err(Error::Invalid("invalid web research profile path".into())),
		};
		profile.validate()?;
		// Account failures disable search, never independent local evidence reads.
		let account = match AccountProfile::from_env(chrono::Utc::now()) {
			Ok(account) => account,
			Err(_) => {
				tracing::warn!("web search disabled: account configuration unavailable");
				None
			}
		};
		Ok(Self::new(profile, account))
	}
	pub(crate) fn search_available(&self) -> bool {
		self.profile.admission
			&& self.account.as_ref().is_some_and(|account| {
				account.validate_at(chrono::Utc::now()).is_ok()
					&& crate::web_search::credential_available(account)
			})
	}
	pub(crate) fn permits_url(&self, value: &str) -> bool {
		let Ok(url) = reqwest::Url::parse(value) else {
			return false;
		};
		let Some(host) = url.host_str() else {
			return false;
		};
		(self.profile.allowed_domains.is_empty()
			|| self
				.profile
				.allowed_domains
				.iter()
				.any(|domain| crate::web_search::domain_matches(host, domain)))
			&& !self
				.profile
				.denied_domains
				.iter()
				.any(|domain| crate::web_search::domain_matches(host, domain))
	}
}
