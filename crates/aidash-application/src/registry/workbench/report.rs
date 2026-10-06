//! Report assembly enforces the selected execution identity and redacts only copied payloads by default.
use crate::{Error, Result, ports::registry::workbench::report::ReportSources};
use aidash_domain::{
	identity::Principal,
	registry::{
		EntityRef,
		workbench::{
			permissions::PermissionInput,
			report::{Report, ReportQuery},
		},
	},
};
use chrono::Utc;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
	Json,
	Html,
}
#[derive(Debug)]
pub struct Output {
	pub report: Report,
	pub format: Format,
}
pub async fn assemble(
	sources: &dyn ReportSources,
	reference: EntityRef,
	query: ReportQuery,
) -> Result<Output> {
	let format = match query.format.as_deref().unwrap_or("json") {
		"json" => Format::Json,
		"html" => Format::Html,
		_ => return Err(Error::Invalid("report format must be json or html".into())),
	};
	let inspection = sources.inspect(&reference).await?;
	let context_input = match sources.principal() {
		Principal::Subject { tenant, subject } => {
			if query.tenant.as_ref().is_some_and(|t| t != &tenant)
				|| query.subject.as_ref().is_some_and(|s| s != &subject)
			{
				return Err(Error::Forbidden);
			}
			Some(PermissionInput {
				tenant,
				subject,
				workspace_id: query.workspace_id,
			})
		}
		Principal::Operator => match (query.tenant, query.subject) {
			(Some(tenant), Some(subject)) => Some(PermissionInput {
				tenant,
				subject,
				workspace_id: query.workspace_id,
			}),
			(None, None) if query.workspace_id.is_none() => None,
			_ => {
				return Err(Error::Invalid(
					"report permission context requires tenant and subject together".into(),
				));
			}
		},
	};
	let context = if let Some(input) = context_input {
		Some(sources.permission_context(&reference, input).await?)
	} else {
		None
	};
	let mut incidents = sources.incidents(&reference).await?;
	if !query.include_sensitive {
		for incident in &mut incidents {
			if let Some(copies) = incident.evidence.as_array_mut() {
				for copy in copies {
					if let Some(copy) = copy.as_object_mut() {
						copy.remove("content");
					}
				}
			}
		}
	}
	Ok(Output {
		format,
		report: Report {
			inspection,
			permission_context: context,
			incidents,
			exported_at: Utc::now(),
			note:
				"Factual connected-node snapshot; no Trust assessment or certification is provided."
					.into(),
		},
	})
}
#[cfg(test)]
mod tests;
