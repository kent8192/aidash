//! Factual inspection only. No Trust verdict or certification is inferred.
use super::*;
use crate::registry::EntityRef;
use reinhardt::injectable;

pub(super) async fn require_inspection(
	tx: &mut dyn TransactionExecutor,
	actor: &Actor,
	reference: &EntityRef,
) -> Result<()> {
	aidash_application::registry::workbench::inspection::require(
		&mut crate::bootstrap::draft_authority_scope(tx, actor),
		reference,
	)
	.await
	.map_err(Into::into)
}

fn escape_html(value: &str) -> String {
	value
		.replace('&', "&amp;")
		.replace('<', "&lt;")
		.replace('>', "&gt;")
		.replace('"', "&quot;")
}

pub(crate) use crate::apps::registry::workbench::serializers::trust::ReportQuery;
pub use crate::apps::registry::workbench::serializers::trust::{
	Inspection, PermissionContext, PermissionInput, PermissionRow, Report, TestEvidence,
	WorkspaceUse,
};

use reinhardt::Response;

#[derive(Clone)]
pub struct TrustInspection {
	pub(crate) runtime: Federation,
}

#[injectable(scope = "request")]
pub async fn provide_trust(#[inject] runtime: Federation) -> TrustInspection {
	TrustInspection { runtime }
}

impl TrustInspection {
	pub(crate) async fn inspect(
		&self,
		actor: Actor,
		(id, version): (String, String),
	) -> Result<Inspection> {
		aidash_application::registry::workbench::inspection::usage::inspect(
			&crate::bootstrap::workbench_inspection_repository(&self.runtime, actor),
			EntityRef { id, version },
		)
		.await
		.map_err(Into::into)
	}
	pub(crate) async fn permission_context(
		&self,
		actor: Actor,
		(id, version): (String, String),
		input: PermissionInput,
	) -> Result<PermissionContext> {
		aidash_application::registry::workbench::permissions::inspect(
			&crate::bootstrap::workbench_permission_repository(&self.runtime, actor),
			EntityRef { id, version },
			input,
		)
		.await
		.map_err(Into::into)
	}

	pub(crate) async fn report(
		&self,
		actor: Actor,
		(id, version): (String, String),
		query: ReportQuery,
	) -> Result<Response> {
		let f = self.runtime.clone();
		let format = query.format.as_deref().unwrap_or("json");
		if !matches!(format, "json" | "html") {
			return Err(Error::Invalid("report format must be json or html".into()));
		}
		let inspection = self
			.inspect(actor.clone(), (id.clone(), version.clone()))
			.await?;
		let context_input = match &actor {
			Actor::Subject(identity) => {
				if query
					.tenant
					.as_ref()
					.is_some_and(|tenant| tenant != &identity.tenant)
					|| query
						.subject
						.as_ref()
						.is_some_and(|subject| subject != &identity.subject)
				{
					return Err(Error::Forbidden);
				}
				Some(PermissionInput {
					tenant: identity.tenant.clone(),
					subject: identity.subject.clone(),
					workspace_id: query.workspace_id,
				})
			}
			Actor::Operator => match (&query.tenant, &query.subject) {
				(Some(tenant), Some(subject)) => Some(PermissionInput {
					tenant: tenant.clone(),
					subject: subject.clone(),
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
			Some(
				self.permission_context(actor.clone(), (id.clone(), version.clone()), input)
					.await?,
			)
		} else {
			None
		};
		let mut incidents = super::incident::Incidents { runtime: f }
			.list(actor, (id, version))
			.await?;
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
		let report = Report {
			inspection,
			permission_context: context,
			incidents,
			exported_at: Utc::now(),
			note:
				"Factual connected-node snapshot; no Trust assessment or certification is provided."
					.into(),
		};
		if format == "json" {
			return Ok(Response::ok().with_json(&(report))?);
		}
		let title = escape_html(&format!(
			"Aidash agent report · {}@{}",
			report.inspection.entry.id, report.inspection.entry.version
		));
		let content = escape_html(&serde_json::to_string_pretty(&report)?);
		let html = format!(
			"<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>{title}</title><style>body{{font:14px/1.5 system-ui,sans-serif;max-width:900px;margin:32px auto;color:#172236}}h1{{font-size:24px}}pre{{white-space:pre-wrap;overflow-wrap:anywhere;background:#f4f6fa;padding:20px;border:1px solid #dce2ec}}@media print{{body{{margin:0}}pre{{border:0;padding:0}}}}</style></head><body><h1>{title}</h1><p>Factual connected-node snapshot. No Trust assessment or certification is provided.</p><pre>{content}</pre></body></html>"
		);
		Ok(Response::ok()
			.with_body(html)
			.with_header("Content-Type", "text/html; charset=utf-8"))
	}
}
