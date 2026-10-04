//! Native source readers delegate authority and dependency decisions to application ports.
use crate::{Result, authorization::access::Access};
use uuid::Uuid;
pub(crate) async fn visible(
	access: &mut Access,
	node: &str,
	id: Uuid,
	admission: Uuid,
) -> Result<bool> {
	aidash_application::authorization::source::reads::visible(
		&mut crate::bootstrap::source_read_scope(access),
		node,
		id,
		admission,
	)
	.await
	.map_err(Into::into)
}
impl Access {
	pub(crate) async fn grant_output_visible(&mut self, id: Uuid) -> Result<bool> {
		aidash_application::authorization::source::reads::output_visible(
			&mut crate::bootstrap::source_read_scope(self),
			id,
		)
		.await
		.map_err(Into::into)
	}
}
