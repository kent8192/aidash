use super::Api;
use crate::{Error, Result, apps::identity::serializers::data_protection::Settings};
use aidash_domain::data_protection::DataProtection;
use serde_json::{Value, json};

fn proposed(input: &Value, current: DataProtection) -> Result<DataProtection> {
	if let Some(policy) = input.get("policy") {
		if input.as_object().is_none_or(|args| args.len() != 1) {
			return Err(Error::Invalid(
				"supply a policy document or individual settings".into(),
			));
		}
		return Ok(serde_json::from_value(policy.clone())?);
	}
	let mut policy = current;
	if let Some(level) = input.get("level") {
		policy.level = serde_json::from_value(level.clone())?;
	}
	for (setting, forbid) in [
		("keyless_endpoints", &mut policy.forbid_keyless_endpoints),
		("plan_connections", &mut policy.forbid_plan_connections),
	] {
		match (
			input[format!("allow_{setting}")].as_bool(),
			input[format!("forbid_{setting}")].as_bool(),
		) {
			(Some(allow), Some(block)) if allow == block => {
				return Err(Error::Invalid(format!(
					"conflicting switches for {setting}"
				)));
			}
			(Some(allow), _) => *forbid = !allow,
			(_, Some(block)) => *forbid = block,
			_ => {}
		}
	}
	Ok(policy)
}

impl Api<'_> {
	pub(super) async fn set_data_protection(&self, input: Value) -> Result<Value> {
		let path = format!("/api/tenants/{}/data-protection", self.caller.tenant());
		let current: Settings =
			serde_json::from_value(self.call("GET", &path, Value::Null).await?)?;
		let policy = proposed(&input, current.policy)?;
		if policy.level < current.deployment_floor {
			return Err(Error::Invalid(format!(
				"Data Protection Level {:?} is below the Deployment floor {:?}",
				policy.level, current.deployment_floor
			)));
		}
		let mut body = serde_json::to_value(policy)?;
		let preview = format!(
			"{path}/impact?level={}&forbid_keyless_endpoints={}&forbid_plan_connections={}",
			body["level"]
				.as_str()
				.expect("level serializes as a string"),
			policy.forbid_keyless_endpoints,
			policy.forbid_plan_connections
		);
		let impact = self.call("GET", &preview, Value::Null).await?;
		body["expected_revision"] = json!(current.revision);
		let mut updated = self.call("PUT", &path, body).await?;
		updated["previous"] = json!(current.policy);
		updated["impact"] = impact;
		Ok(updated)
	}
}
