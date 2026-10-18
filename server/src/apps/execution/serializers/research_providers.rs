//! The Research providers this Deployment offers (ADR 0048). Each is a
//! Built-in definition every Tenant may approve; a change takes effect when
//! the server restarts.
use aidash_domain::research::{Operation, account::AccountTerms};
use reinhardt::conf::settings::{
	fragment::SettingsValidation,
	profile::Profile,
	validation::{ValidationError, ValidationResult},
};
use reinhardt::settings;
use serde::{Deserialize, Serialize};

/// `[research_providers]` with `page_reading = true` to read pages with
/// Aidash's own page reader, and `[research_providers.brave]` to search
/// through the Research account holder's Brave account. `fixture = true`
/// offers the deterministic in-memory corpus for tests and evaluation; the
/// production profile refuses it.
#[settings(fragment = true, section = "research_providers", validate = false)]
pub struct ResearchProviderSettings {
	#[setting(default = "false")]
	pub page_reading: bool,
	#[setting(default = "false")]
	pub fixture: bool,
	#[setting(default = "None", leaf)]
	pub brave: Option<BraveSettings>,
}

/// Brave Web Search through the Research account holder's account.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BraveSettings {
	/// The `AIDASH_SECRET_*` Deployment secret holding the Brave key. A
	/// secret that is not set leaves Brave not set up.
	pub secret: String,
	/// The Research account terms, shown to Tenants without the key's name.
	pub account: AccountTerms,
}

impl ResearchProviderSettings {
	/// Startup refuses a malformed declaration, and the fixture in the
	/// production profile. Terms that lapse later only leave their provider
	/// not set up.
	pub fn check(&self, profile: &Profile) -> crate::Result<()> {
		if self.fixture && *profile == Profile::Production {
			return Err(crate::Error::Invalid(
				"research_providers.fixture is refused in the production profile".into(),
			));
		}
		if let Some(brave) = &self.brave {
			crate::config::validate_secret_reference(&brave.secret)?;
			brave
				.account
				.validate(&[Operation::Search])
				.map_err(|error| {
					crate::Error::Invalid(format!("research_providers.brave.account: {error}"))
				})?;
		}
		Ok(())
	}
}

impl SettingsValidation for ResearchProviderSettings {
	fn validate(&self, profile: &Profile) -> ValidationResult {
		self.check(profile)
			.map_err(|error| ValidationError::Constraint(error.to_string()))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use rstest::rstest;
	use serde_json::json;

	fn brave(terms: serde_json::Value) -> ResearchProviderSettings {
		serde_json::from_value(json!({
			"page_reading": false,
			"fixture": false,
			"brave": {"secret": "AIDASH_SECRET_BRAVE_SEARCH", "account": terms},
		}))
		.unwrap()
	}

	fn terms() -> serde_json::Value {
		json!({
			"account_id":"brave-account",
			"agreement_reference":"approved-contract-record",
			"verified_by":"deployment_administrator",
			"verified_at":"2026-09-01T00:00:00Z",
			"review_after":"2026-10-01T00:00:00Z",
			"storage_permitted":true,
			"query_training_prohibited":true,
			"evaluation_permitted":true,
			"retention_days":90,
			"deletion_after_termination_days":30,
			"price_effective_at":"2026-09-01T00:00:00Z",
			"price_review_after":"2026-10-01T00:00:00Z",
			"search_price_micro_usd":5000,
			"monthly_base_fee_micro_usd":0,
			"monthly_limit_micro_usd":20_000_000
		})
	}

	#[rstest]
	fn the_fixture_is_refused_only_in_production() {
		let settings: ResearchProviderSettings =
			serde_json::from_value(json!({"page_reading": true, "fixture": true, "brave": null}))
				.unwrap();
		assert!(settings.check(&Profile::Development).is_ok());
		assert!(settings.check(&Profile::Staging).is_ok());
		assert!(settings.check(&Profile::Production).is_err());
	}

	#[rstest]
	fn lapsed_brave_terms_start_but_incomplete_ones_do_not() {
		// The fixed dates above have lapsed; that is checked per call.
		assert!(brave(terms()).check(&Profile::Production).is_ok());
		let mut incomplete = terms();
		incomplete["search_price_micro_usd"] = json!(null);
		assert!(brave(incomplete).check(&Profile::Production).is_err());
		let mut fetch_priced = terms();
		fetch_priced["fetch_price_micro_usd"] = json!(1);
		assert!(brave(fetch_priced).check(&Profile::Production).is_err());
	}

	#[rstest]
	fn the_brave_key_is_a_deployment_secret_outside_the_terms() {
		let mut settings = brave(terms());
		settings.brave.as_mut().unwrap().secret = "BRAVE_KEY".into();
		assert!(settings.check(&Profile::Development).is_err());
		let mut terms = terms();
		terms["credential_env"] = json!("AIDASH_SECRET_BRAVE_SEARCH");
		assert!(
			serde_json::from_value::<ResearchProviderSettings>(json!({
				"page_reading": false, "fixture": false,
				"brave": {"secret": "AIDASH_SECRET_BRAVE_SEARCH", "account": terms},
			}))
			.is_err()
		);
	}
}
