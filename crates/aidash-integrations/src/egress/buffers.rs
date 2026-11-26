//! Scratch buffers shared by both directions of all Host relays in a gateway.
use std::{
	io,
	pin::Pin,
	sync::{
		Mutex,
		atomic::{AtomicUsize, Ordering},
	},
	task::{Poll, ready},
};
use tokio::io::{AsyncRead, ReadBuf};

const CHUNK: usize = 64 * 1024;

#[derive(Default)]
pub(super) struct BufferPool {
	cache: Mutex<Cache>,
	borrowed: AtomicUsize,
}

#[derive(Default)]
struct Cache {
	current: Vec<Vec<u8>>,
	previous: Vec<Vec<u8>>,
}

pub(super) struct Borrowed<'a> {
	bytes: Vec<u8>,
	pool: &'a BufferPool,
}

impl BufferPool {
	fn take(&self) -> Borrowed<'_> {
		let cached = {
			let mut cache = self.cache.lock().unwrap();
			cache.current.pop().or_else(|| cache.previous.pop())
		};
		self.borrowed.fetch_add(CHUNK, Ordering::Relaxed);
		Borrowed {
			bytes: cached.unwrap_or_else(|| vec![0; CHUNK]),
			pool: self,
		}
	}

	/// Release buffers that went unused through the previous sampling interval.
	/// Bursts can borrow as many as they need.
	pub(super) fn expire_unused(&self) {
		let expired = {
			let mut cache = self.cache.lock().unwrap();
			let current = std::mem::take(&mut cache.current);
			std::mem::replace(&mut cache.previous, current)
		};
		drop(expired);
	}

	pub(super) fn borrowed_bytes(&self) -> usize {
		self.borrowed.load(Ordering::Relaxed)
	}

	pub(super) async fn read<R: AsyncRead + Unpin>(
		&self,
		reader: &mut R,
	) -> io::Result<(Borrowed<'_>, usize)> {
		// Hyper upgrades can have already buffered bytes and have no readable()
		// API. Polling the read registers for readiness; on Pending the lease
		// drops before the task sleeps.
		std::future::poll_fn(|cx| {
			let mut lease = self.take();
			let mut buffer = ReadBuf::new(&mut lease.bytes);
			ready!(Pin::new(&mut *reader).poll_read(cx, &mut buffer))?;
			let filled = buffer.filled().len();
			Poll::Ready(Ok((lease, filled)))
		})
		.await
	}
}

impl Borrowed<'_> {
	pub(super) fn bytes(&self) -> &[u8] {
		&self.bytes
	}
}

impl Drop for Borrowed<'_> {
	fn drop(&mut self) {
		self.pool
			.cache
			.lock()
			.unwrap()
			.current
			.push(std::mem::take(&mut self.bytes));
		self.pool.borrowed.fetch_sub(CHUNK, Ordering::Relaxed);
	}
}
