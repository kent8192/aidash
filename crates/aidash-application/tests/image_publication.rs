use aidash_application::{
	Error, Result,
	ports::semantic::image_publications::{ImagePublication, ImagePublicationSession},
	semantic::image_publications::process,
};
use aidash_domain::semantic::indexing::{ImageChange, ImageSlots};
use async_trait::async_trait;
use serde_json::json;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Default)]
struct Effects {
	stored: Option<ImageSlots>,
	committed: bool,
	fail_store: bool,
}

struct Session {
	publication: Option<ImagePublication>,
	effects: Arc<Mutex<Effects>>,
}

#[async_trait]
impl ImagePublicationSession for Session {
	async fn load(&mut self) -> Result<Option<ImagePublication>> {
		Ok(self.publication.take())
	}
	async fn store(
		&mut self,
		_: &ImagePublication,
		_: &ImageSlots,
		slots: &ImageSlots,
	) -> Result<()> {
		let mut effects = self.effects.lock().unwrap();
		if effects.fail_store {
			return Err(Error::External("storage failed".into()));
		}
		effects.stored = Some(slots.clone());
		Ok(())
	}
	async fn commit(self: Box<Self>) -> Result<()> {
		self.effects.lock().unwrap().committed = true;
		Ok(())
	}
}

fn fixture(fail_store: bool) -> (Box<Session>, Arc<Mutex<Effects>>) {
	let effects = Arc::new(Mutex::new(Effects {
		fail_store,
		..Default::default()
	}));
	let publication = ImagePublication {
		workspace: Uuid::from_u128(1),
		area: Uuid::from_u128(2),
		generation: 1,
		sequence: 1,
		changes: vec![ImageChange::Published {
			file_id: Uuid::from_u128(3),
			path: "one.png".into(),
			media_type: "image/png".into(),
			size: 4,
		}],
		authority: json!({"subject":"owner"}),
		max_images: 1,
		slots: ImageSlots::default(),
		rebuild_after: false,
	};
	(
		Box::new(Session {
			publication: Some(publication),
			effects: effects.clone(),
		}),
		effects,
	)
}

#[tokio::test]
async fn publication_commits_the_image_and_its_slot_as_one_effect() {
	let (session, effects) = fixture(false);
	assert!(process(session).await.unwrap());
	let effects = effects.lock().unwrap();
	assert!(effects.committed);
	assert!(
		effects
			.stored
			.as_ref()
			.unwrap()
			.assigned(Uuid::from_u128(2), Uuid::from_u128(3))
	);
}

#[tokio::test]
async fn failed_storage_does_not_acknowledge_the_publication() {
	let (session, effects) = fixture(true);
	assert!(process(session).await.is_err());
	let effects = effects.lock().unwrap();
	assert!(!effects.committed);
	assert!(effects.stored.is_none());
}

#[tokio::test]
async fn initialization_without_a_queued_publication_can_commit() {
	let (mut session, effects) = fixture(false);
	session.publication = None;
	assert!(!process(session).await.unwrap());
	assert!(effects.lock().unwrap().committed);
}

#[tokio::test]
async fn a_rebuild_reconsiders_waiting_images_after_the_last_queued_change() {
	let (mut session, effects) = fixture(false);
	let publication = session.publication.as_mut().unwrap();
	publication.max_images = 2;
	let old = |id| ImageChange::Published {
		file_id: Uuid::from_u128(id),
		path: format!("{id}.png"),
		media_type: "image/png".into(),
		size: 4,
	};
	publication
		.slots
		.apply(publication.area, 1, 1, &[old(10), old(11), old(12)], 2);
	publication.slots.apply(
		publication.area,
		1,
		2,
		&[ImageChange::Removed {
			file_id: Uuid::from_u128(10),
			path: "10.png".into(),
		}],
		2,
	);
	publication.sequence = 3;
	publication.changes = vec![old(13)];
	publication.slots.request_rebuild(Some(3));
	publication.rebuild_after = true;
	assert!(process(session).await.unwrap());
	let effects = effects.lock().unwrap();
	let slots = effects.stored.as_ref().unwrap();
	assert!(slots.assigned(Uuid::from_u128(2), Uuid::from_u128(12)));
	assert!(!slots.assigned(Uuid::from_u128(2), Uuid::from_u128(13)));
}

#[tokio::test]
async fn a_publication_waiting_for_owner_authority_does_not_block_later_images() {
	let (mut first, first_effects) = fixture(false);
	let first_publication = first.publication.as_mut().unwrap();
	first_publication.authority =
		json!({"credential":null,"tenant":"tenant","subject":"owner","subjects":[]});
	first_publication.max_images = 2;
	assert!(process(first).await.unwrap());
	let saved = first_effects.lock().unwrap().stored.clone().unwrap();
	let (mut second, second_effects) = fixture(false);
	let second_publication = second.publication.as_mut().unwrap();
	second_publication.sequence = 2;
	second_publication.max_images = 2;
	second_publication.slots = saved;
	second_publication.changes = vec![ImageChange::Published {
		file_id: Uuid::from_u128(4),
		path: "two.png".into(),
		media_type: "image/png".into(),
		size: 4,
	}];
	assert!(process(second).await.unwrap());
	let effects = second_effects.lock().unwrap();
	let slots = effects.stored.as_ref().unwrap();
	assert!(slots.assigned(Uuid::from_u128(2), Uuid::from_u128(3)));
	assert!(slots.assigned(Uuid::from_u128(2), Uuid::from_u128(4)));
}
