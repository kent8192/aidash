//! Factual inspection only. No Trust verdict or certification is inferred.
use super::*;
use crate::registry::EntityRef;
use reinhardt::injectable;

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
		let output = aidash_application::registry::workbench::report::assemble(
			&crate::bootstrap::workbench_report_sources(&self.runtime, actor),
			EntityRef { id, version },
			query,
		)
		.await?;
		let report = output.report;
		if output.format == aidash_application::registry::workbench::report::Format::Json {
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
