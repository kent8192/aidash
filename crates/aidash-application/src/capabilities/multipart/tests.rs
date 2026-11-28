use super::*;

#[test]
fn accepts_a_resumed_final_chunk_and_identical_replays() {
	let bytes = b"last part";
	let size = CHUNK_BYTES + bytes.len() as u64;
	let input = Chunk {
		offset: CHUNK_BYTES,
		data: STANDARD.encode(bytes),
	};
	assert!(matches!(
		next_chunk(&input, size, 0, None),
		Err(Error::Conflict(code)) if code == "UPLOAD_OFFSET_CHANGED"
	));
	assert_eq!(
		next_chunk(&input, size, CHUNK_BYTES, None).unwrap(),
		Some(bytes.to_vec())
	);
	let digest = format!("{:x}", Sha256::digest(bytes));
	assert_eq!(
		next_chunk(&input, size, size, Some((&digest, bytes.len() as u64))).unwrap(),
		None
	);
	let replaced = Chunk {
		data: STANDARD.encode(b"different"),
		..input
	};
	assert!(matches!(
		next_chunk(&replaced, size, size, Some((&digest, bytes.len() as u64))),
		Err(Error::Conflict(code)) if code == "CHUNK_CHANGED"
	));
}
