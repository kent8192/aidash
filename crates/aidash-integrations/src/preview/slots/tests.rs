use super::*;

#[test]
fn unidentified_connections_share_a_few_slots() {
	let slots = Slots::with_sizes(2, 8, 4);
	let first = slots.arrive().unwrap();
	let _second = slots.arrive().unwrap();
	assert!(slots.arrive().is_none());
	drop(first);
	assert!(slots.arrive().is_some());
}

#[test]
fn an_admitted_connection_gives_its_unidentified_slot_back() {
	let slots = Slots::with_sizes(1, 8, 4);
	let connection = slots.arrive().unwrap();
	assert!(slots.arrive().is_none());
	slots.admit(&connection, "a.preview.test").unwrap();
	assert!(slots.arrive().is_some());
}

#[test]
fn a_full_preview_does_not_lock_out_another() {
	let slots = Slots::with_sizes(8, 8, 2);
	let held: Vec<_> = (0..2)
		.map(|_| {
			let connection = slots.arrive().unwrap();
			slots.admit(&connection, "a.preview.test").unwrap();
			connection
		})
		.collect();
	let third = slots.arrive().unwrap();
	assert_eq!(
		slots.admit(&third, "a.preview.test"),
		Err(Refusal::PreviewFull)
	);
	let other = slots.arrive().unwrap();
	assert_eq!(slots.admit(&other, "b.preview.test"), Ok(()));
	drop(held);
	assert_eq!(slots.admit(&third, "a.preview.test"), Ok(()));
}

#[test]
fn the_admitted_ceiling_covers_every_preview() {
	let slots = Slots::with_sizes(8, 2, 2);
	let a = slots.arrive().unwrap();
	slots.admit(&a, "a.preview.test").unwrap();
	let b = slots.arrive().unwrap();
	slots.admit(&b, "b.preview.test").unwrap();
	let c = slots.arrive().unwrap();
	assert_eq!(slots.admit(&c, "c.preview.test"), Err(Refusal::Busy));
}

#[test]
fn a_port_or_letter_case_names_the_same_preview() {
	let slots = Slots::with_sizes(8, 8, 1);
	let first = slots.arrive().unwrap();
	slots.admit(&first, "A.Preview.Test:443").unwrap();
	let second = slots.arrive().unwrap();
	assert_eq!(
		slots.admit(&second, "a.preview.test."),
		Err(Refusal::PreviewFull)
	);
	// The connection already held for that preview needs no second slot.
	assert_eq!(slots.admit(&first, "a.preview.test"), Ok(()));
}

#[test]
fn a_connection_stays_with_the_preview_it_was_admitted_to() {
	let slots = Slots::with_sizes(8, 8, 4);
	let connection = slots.arrive().unwrap();
	slots.admit(&connection, "a.preview.test").unwrap();
	assert_eq!(
		slots.admit(&connection, "b.preview.test"),
		Err(Refusal::OtherPreview)
	);
}

#[test]
fn a_tunnel_keeps_its_slot_after_the_connection_ends() {
	let slots = Slots::with_sizes(8, 8, 1);
	let connection = slots.arrive().unwrap();
	slots.admit(&connection, "a.preview.test").unwrap();
	let tunnel = connection.tunnel().unwrap();
	drop(connection);
	let next = slots.arrive().unwrap();
	assert_eq!(
		slots.admit(&next, "a.preview.test"),
		Err(Refusal::PreviewFull)
	);
	drop(tunnel);
	assert_eq!(slots.admit(&next, "a.preview.test"), Ok(()));
}

#[test]
fn an_unidentified_connection_has_no_slot_to_hand_a_tunnel() {
	let slots = Slots::with_sizes(8, 8, 1);
	let connection = slots.arrive().unwrap();
	assert!(connection.tunnel().is_none());
	assert_eq!(slots.admit(&connection, "a.preview.test"), Ok(()));
}
