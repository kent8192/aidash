//! Apply one ordered File publication and its quota decisions atomically.
use crate::{Result, ports::semantic::image_publications::ImagePublicationSession};

pub async fn process(mut scope: Box<dyn ImagePublicationSession>) -> Result<bool> {
	let Some(publication) = scope.load().await? else {
		scope.commit().await?;
		return Ok(false);
	};
	let before = publication.slots.clone();
	let mut after = before.clone();
	after.apply(
		publication.area,
		publication.generation,
		publication.sequence,
		&publication.changes,
		publication.max_images,
	);
	if publication.rebuild_after {
		after.finish_publication(publication.sequence, publication.max_images);
	}
	scope.store(&publication, &before, &after).await?;
	scope.commit().await?;
	Ok(true)
}
