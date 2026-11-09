//! Tracks request framing at the socket, including bytes Hyper has buffered.
//! It only informs the linger counter; Hyper remains the HTTP parser.

const MAX_HEAD: usize = 512 * 1024;
const MAX_CHUNK_LINE: usize = MAX_HEAD;

#[derive(Default)]
pub(super) struct Progress {
	mode: Mode,
	head: Vec<u8>,
	complete: Option<bool>,
}

#[derive(Default)]
enum Mode {
	#[default]
	Head,
	Fixed(u64),
	Chunked(Chunk),
	Lost,
}

#[derive(Default)]
struct Chunk {
	state: ChunkState,
	line: Vec<u8>,
}

#[derive(Default)]
enum ChunkState {
	#[default]
	Size,
	Data(u64),
	DataCr,
	DataLf,
	Trailer,
}

impl Progress {
	pub(super) fn disable(&mut self) {
		self.mode = Mode::Lost;
		self.head = Vec::new();
		self.complete = None;
	}

	/// Hyper has parsed this head. The socket may already have read body bytes.
	pub(super) fn note(&mut self, length: Option<u64>, has_body: bool) {
		if !matches!(self.mode, Mode::Head) {
			self.mode = Mode::Lost;
			self.complete = None;
			return;
		}
		// Hyper accepts empty lines before the request line.
		let mut start = 0;
		while start < self.head.len() {
			if self.head[start..].starts_with(b"\r\n") {
				start += 2;
			} else if self.head[start] == b'\n' {
				start += 1;
			} else {
				break;
			}
		}
		let head = &self.head[start..];
		let end = head
			.windows(2)
			.position(|bytes| bytes == b"\n\n")
			.map(|at| start + at + 2)
			.into_iter()
			.chain(
				head.windows(3)
					.position(|bytes| bytes == b"\n\r\n")
					.map(|at| start + at + 3),
			)
			.min();
		let Some(end) = end else {
			self.mode = Mode::Lost;
			self.complete = None;
			return;
		};
		let tail = self.head.split_off(end);
		self.head.clear();
		let has_body = has_body && length != Some(0);
		self.complete = Some(!has_body);
		self.mode = if !has_body {
			Mode::Head
		} else if let Some(length) = length {
			Mode::Fixed(length)
		} else {
			Mode::Chunked(Chunk::default())
		};
		self.read(&tail);
	}

	pub(super) fn incomplete(&self) -> bool {
		self.complete == Some(false)
	}

	/// Records bytes when read from the socket, not when delivered to Hyper.
	pub(super) fn read(&mut self, mut bytes: &[u8]) {
		while !bytes.is_empty() {
			match &mut self.mode {
				Mode::Head => {
					if self.head.len().saturating_add(bytes.len()) > MAX_HEAD {
						self.disable();
					} else {
						self.head.extend_from_slice(bytes);
					}
					return;
				}
				Mode::Fixed(left) => {
					let taken = (*left).min(bytes.len() as u64);
					*left -= taken;
					bytes = &bytes[taken as usize..];
					if *left == 0 {
						self.complete = Some(true);
						self.mode = Mode::Head;
					}
				}
				Mode::Chunked(chunk) => {
					let (taken, ended, invalid) = chunk.read(bytes);
					bytes = &bytes[taken..];
					if invalid {
						self.complete = None;
						self.mode = Mode::Lost;
					} else if ended {
						self.complete = Some(true);
						self.mode = Mode::Head;
					}
				}
				Mode::Lost => return,
			}
		}
	}
}

impl Chunk {
	/// Returns consumed bytes, whether the body ended, and whether framing was invalid.
	fn read(&mut self, bytes: &[u8]) -> (usize, bool, bool) {
		let mut offset = 0;
		while offset < bytes.len() {
			match self.state {
				ChunkState::Data(left) => {
					let taken = left.min((bytes.len() - offset) as u64);
					offset += taken as usize;
					self.state = if taken == left {
						ChunkState::DataCr
					} else {
						ChunkState::Data(left - taken)
					};
				}
				ChunkState::DataCr | ChunkState::DataLf => {
					let expected = if matches!(self.state, ChunkState::DataCr) {
						b'\r'
					} else {
						b'\n'
					};
					let valid = bytes[offset] == expected;
					offset += 1;
					if !valid {
						return (offset, false, true);
					}
					self.state = if expected == b'\r' {
						ChunkState::DataLf
					} else {
						ChunkState::Size
					};
				}
				ChunkState::Size | ChunkState::Trailer => {
					self.line.push(bytes[offset]);
					offset += 1;
					if self.line.len() > MAX_CHUNK_LINE {
						return (offset, false, true);
					}
					if !self.line.ends_with(b"\r\n") {
						continue;
					}
					let line = &self.line[..self.line.len() - 2];
					if matches!(self.state, ChunkState::Trailer) {
						if line.is_empty() {
							return (offset, true, false);
						}
					} else {
						let size = line
							.split(|b| *b == b';')
							.next()
							.and_then(|value| std::str::from_utf8(value).ok())
							.and_then(|value| u64::from_str_radix(value.trim(), 16).ok());
						self.state = match size {
							Some(0) => ChunkState::Trailer,
							Some(n) => ChunkState::Data(n),
							None => return (offset, false, true),
						};
					}
					self.line.clear();
				}
			}
		}
		(offset, false, false)
	}
}

#[cfg(test)]
mod tests {
	use super::Progress;

	#[test]
	fn skips_empty_lines_before_the_request() {
		let mut progress = Progress::default();
		progress.read(b"\r\n\r\nPOST / HTTP/1.1\r\nhost: gateway\r\ncontent-length: 4\r\n\r\nab");
		progress.note(Some(4), true);
		assert!(progress.incomplete());
		progress.read(b"cd");
		assert!(!progress.incomplete());
	}

	#[test]
	fn tracks_a_body_after_a_bare_lf_request_head() {
		let mut progress = Progress::default();
		progress.read(b"POST / HTTP/1.1\nhost: gateway\ncontent-length: 4\n\nab");
		progress.note(Some(4), true);
		assert!(progress.incomplete());
		progress.read(b"cd");
		assert!(!progress.incomplete());
	}

	#[test]
	fn tracks_a_chunked_upload_before_a_pipelined_fixed_upload() {
		let mut progress = Progress::default();
		progress.read(b"POST /first HTTP/1.1\r\ntransfer-encoding: chunked\r\n\r\n3\r\nab");
		progress.note(None, true);
		assert!(progress.incomplete());
		progress.read(
			b"c\r\n0\r\ntrailer: yes\r\n\r\nPOST /second HTTP/1.1\r\ncontent-length: 4\r\n\r\nab",
		);
		assert!(!progress.incomplete());
		progress.note(Some(4), true);
		assert!(progress.incomplete());
		progress.read(b"cd");
		assert!(!progress.incomplete());
	}
}
