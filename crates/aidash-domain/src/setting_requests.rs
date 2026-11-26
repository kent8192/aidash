//! Person-owned inputs to the Tenant Configurator. The persisted form describes
//! destinations and constraints. Secret bytes belong only to the answer writer.
use crate::{Error, Result, tenant_secrets};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;

pub const KIND: &str = "SETTING_REQUEST";
pub fn is_tenant_configurator(agent_id: &str) -> bool {
	agent_id == "aidash.tenant-configurator"
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(rename = "SettingRequestForm")]
pub enum Form {
	Secret {
		label: String,
		name: String,
		hosts: Vec<String>,
	},
	Text {
		fields: Vec<TextField>,
	},
	Choice {
		label: String,
		options: Vec<String>,
		#[serde(default)]
		multiple: bool,
	},
	Oauth {
		label: String,
		target: OAuth,
	},
	Payment {
		label: String,
		target: Payment,
	},
	Purge {
		label: String,
		target: Purge,
	},
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(rename = "SettingOAuth")]
pub enum OAuth {
	Tool {
		resource: String,
		#[serde(default)]
		scopes: Vec<String>,
	},
	Provider {
		provider: String,
	},
	Plan {
		provider: String,
	},
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(rename = "SettingPayment")]
pub enum Payment {
	Subscription,
	AgentPlugin { listing: String },
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(rename = "SettingPurge")]
pub enum Purge {
	Tenant,
	Node { node: String },
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "SettingTextField")]
pub struct TextField {
	pub name: String,
	pub label: String,
	pub constraint: Option<Constraint>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(rename = "SettingConstraint")]
pub enum Constraint {
	Regex {
		pattern: String,
		message: String,
	},
	Number {
		min: f64,
		max: f64,
		integer: bool,
		unit: String,
	},
	Format {
		format: Format,
	},
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "SettingFormat")]
pub enum Format {
	Email,
	HttpsUrl,
	Date,
	Time,
	TimeZone,
}
#[derive(Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "SettingLanguage")]
pub enum Language {
	Ja,
	#[default]
	En,
}
impl Language {
	fn message(self, en: &str, ja: &str) -> Error {
		Error::Invalid(
			match self {
				Self::En => en,
				Self::Ja => ja,
			}
			.into(),
		)
	}
}

fn check(ok: bool, message: &str) -> Result<()> {
	if ok {
		Ok(())
	} else {
		Err(Error::Invalid(message.into()))
	}
}
fn label(s: &str) -> Result<()> {
	check(
		!s.trim().is_empty() && s.chars().count() <= 200 && !s.chars().any(char::is_control),
		"Provide a label of 1 to 200 printable characters.",
	)
}
impl Form {
	pub fn validate(&self) -> Result<()> {
		match self {
			Self::Secret {
				label: l,
				name,
				hosts,
			} => {
				label(l)?;
				tenant_secrets::validate_name(name)?;
				for host in tenant_secrets::normalize_hosts(hosts)? {
					let known =
						psl::suffix(host.as_bytes()).is_some_and(|suffix| suffix.is_known());
					check(
						known
							&& psl::domain_str(&host).is_some()
							&& !host.ends_with('.')
							&& !host.ends_with(".arpa")
							&& host.split('.').all(|s| {
								!s.is_empty()
									&& s.len() <= 63 && !s.starts_with('-')
									&& !s.ends_with('-') && s
									.bytes()
									.all(|b| b.is_ascii_alphanumeric() || b == b'-')
							}),
						"Use a public DNS host for each Tenant Secret destination.",
					)?;
				}
			}
			Self::Text { fields } => {
				check(
					(1..=10).contains(&fields.len()),
					"Provide 1 to 10 text fields.",
				)?;
				let mut names = HashSet::new();
				for field in fields {
					tenant_secrets::validate_name(&field.name)?;
					check(names.insert(&field.name), "Field names must be unique.")?;
					label(&field.label)?;
					match &field.constraint {
						Some(Constraint::Regex { pattern, message }) => {
							label(message)?;
							check(
								pattern.chars().count() <= 500
									&& regex::Regex::new(pattern).is_ok(),
								"Supply a valid Rust regex of at most 500 characters.",
							)?;
						}
						Some(Constraint::Number { min, max, unit, .. }) => check(
							min.is_finite()
								&& max.is_finite() && min <= max
								&& unit.chars().count() <= 200,
							"Provide a valid number range and unit.",
						)?,
						_ => {}
					}
				}
			}
			Self::Choice {
				label: l, options, ..
			} => {
				label(l)?;
				check(
					(1..=50).contains(&options.len())
						&& options.iter().collect::<HashSet<_>>().len() == options.len(),
					"Offer 1 to 50 distinct choices.",
				)?;
				for option in options {
					label(option)?;
				}
			}
			Self::Oauth { label: l, target } => {
				label(l)?;
				match target {
					OAuth::Tool { resource, scopes } => {
						crate::oauth_connections::canonical_resource(resource)
							.map_err(|_| Error::Invalid("Invalid tool resource URL.".into()))?;
						crate::oauth_connections::admissible_scopes(scopes, None, None, None)
							.map_err(|_| Error::Invalid("Invalid authorization scopes.".into()))?;
					}
					OAuth::Provider { provider } => {
						check(provider == "openrouter", "Unknown Provider Authorization.")?
					}
					OAuth::Plan { provider } => {
						check(provider == "chatgpt", "Unknown Plan Connection.")?
					}
				}
			}
			Self::Payment { label: l, target } => {
				label(l)?;
				if let Payment::AgentPlugin { listing } = target {
					crate::policy::identifier(listing)?;
				}
			}
			Self::Purge { label: l, target } => {
				label(l)?;
				if let Purge::Node { node } = target {
					crate::nodes::NodeName::parse(node)?;
				}
			}
		}
		Ok(())
	}

	pub fn outcome(&self, outcome: &str) -> Value {
		match self {
			Self::Secret { name, .. } => json!({"name":name,"outcome":outcome}),
			_ => json!({"outcome":outcome}),
		}
	}

	/// The projection persisted in the Human Request and delivered to the model.
	/// For a secret, the caller must save the original answer before committing it.
	pub fn answer(&self, answer: &Answer) -> Result<Value> {
		self.validate()?;
		let lang = answer.language;
		let invalid = || {
			lang.message(
				"Enter an answer for every field.",
				"すべての項目に回答してください。",
			)
		};
		match answer.action {
			Action::Decline if answer.value.is_none() => return Ok(self.outcome("declined")),
			Action::Done
				if matches!(
					self,
					Self::Oauth { .. } | Self::Payment { .. } | Self::Purge { .. }
				) && answer.value.is_none() =>
			{
				return Ok(self.outcome("done"));
			}
			Action::Accept => {}
			_ => return Err(invalid()),
		}
		let value = answer.value.as_ref().ok_or_else(invalid)?;
		match self {
			Self::Secret { .. } => {
				let secret = value.as_str().ok_or_else(invalid)?;
				if secret.is_empty()
					|| secret.len() > 8192
					|| secret
						.chars()
						.any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
				{
					return Err(lang.message(
						"Enter a single-line secret of up to 8 KB without control characters.",
						"秘密情報は制御文字や改行を含めず、8 KB 以内で入力してください。",
					));
				}
				Ok(self.outcome("saved"))
			}
			Self::Choice {
				options, multiple, ..
			} => {
				let allowed =
					|v: &Value| v.as_str().is_some_and(|s| options.iter().any(|o| o == s));
				let ok = if *multiple {
					value.as_array().is_some_and(|a| {
						!a.is_empty()
							&& a.len() <= options.len()
							&& a.iter().all(allowed)
							&& a.iter()
								.filter_map(Value::as_str)
								.collect::<HashSet<_>>()
								.len() == a.len()
					})
				} else {
					allowed(value)
				};
				if !ok {
					return Err(lang.message(
						"Select from the offered choices.",
						"選択肢の中から選んでください。",
					));
				}
				Ok(json!({"outcome":"answered","person_data":value}))
			}
			Self::Text { fields } => {
				let values = value
					.as_object()
					.filter(|o| o.len() == fields.len())
					.ok_or_else(invalid)?;
				let mut data = serde_json::Map::new();
				for field in fields {
					let text: String = values
						.get(&field.name)
						.and_then(Value::as_str)
						.ok_or_else(invalid)?
						.chars()
						.filter(|c| !c.is_control())
						.collect();
					if text.is_empty() || text.chars().count() > 2000 {
						return Err(lang.message(
							"Enter 1 to 2,000 characters.",
							"1〜2,000文字で入力してください。",
						));
					}
					if let Some(constraint) = &field.constraint {
						constraint.validate(&text, lang)?;
					}
					data.insert(field.name.clone(), json!(text));
				}
				Ok(json!({"outcome":"answered","person_data":data}))
			}
			_ => Err(invalid()),
		}
	}
}

#[derive(Deserialize, JsonSchema)]
#[schemars(rename = "SettingRequestAnswer")]
#[serde(deny_unknown_fields)]
pub struct Answer {
	pub action: Action,
	pub value: Option<Value>,
	#[serde(default)]
	pub language: Language,
}
#[derive(Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "SettingAction")]
pub enum Action {
	Accept,
	Decline,
	Done,
}
impl Constraint {
	fn validate(&self, s: &str, lang: Language) -> Result<()> {
		match self {
			Self::Regex { pattern, message } => check(
				regex::Regex::new(pattern).is_ok_and(|r| r.is_match(s)),
				message,
			),
			Self::Number {
				min,
				max,
				integer,
				unit,
			} => {
				if s.parse::<f64>().is_ok_and(|n| {
					n.is_finite() && n >= *min && n <= *max && (!integer || n.fract() == 0.0)
				}) {
					return Ok(());
				}
				let (en, ja) = if *integer {
					("whole number", "整数")
				} else {
					("number", "数値")
				};
				Err(lang.message(
					&format!("Enter a {en} from {min} to {max} {unit}."),
					&format!("{min}〜{max} {unit} の範囲で{ja}を入力してください。"),
				))
			}
			Self::Format { format } => {
				let (ok, en, ja) = match format {
					Format::Email => (
						s.split_once('@').is_some_and(|(local, domain)| {
							!local.is_empty()
								&& !local.starts_with('.')
								&& !local.ends_with('.') && !local.contains("..")
								&& !domain.contains('@') && domain.contains('.')
								&& domain.split('.').all(|label| {
									!label.is_empty()
										&& !label.starts_with('-') && !label.ends_with('-')
										&& label
											.bytes()
											.all(|c| c.is_ascii_alphanumeric() || c == b'-')
								}) && !s.chars().any(char::is_whitespace)
						}),
						"Enter an email address.",
						"メールアドレスを入力してください。",
					),
					Format::HttpsUrl => (
						s.starts_with("https://")
							&& !s.chars().any(char::is_whitespace)
							&& url::Url::parse(s).is_ok_and(|u| u.host().is_some()),
						"Enter an HTTPS URL.",
						"https:// で始まる URL を入力してください。",
					),
					Format::Date => (
						s.len() == 10 && chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok(),
						"Enter a date as YYYY-MM-DD.",
						"日付を YYYY-MM-DD で入力してください。",
					),
					Format::Time => (
						s.len() == 5 && chrono::NaiveTime::parse_from_str(s, "%H:%M").is_ok(),
						"Enter a time as HH:MM.",
						"時刻を HH:MM で入力してください。",
					),
					Format::TimeZone => (
						s.parse::<chrono_tz::Tz>().is_ok(),
						"Enter an IANA time zone such as Asia/Tokyo.",
						"Asia/Tokyo などのタイムゾーンを入力してください。",
					),
				};
				if ok {
					Ok(())
				} else {
					Err(lang.message(en, ja))
				}
			}
		}
	}
}
#[cfg(test)]
mod tests;
