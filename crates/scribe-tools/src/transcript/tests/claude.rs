//! Bounded reads on append and retry, independent of elapsed time.

use crate::transcript::claude::read_window;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};

struct Counted {
    file: File,
    bytes_read: usize,
}

impl Read for Counted {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.file.read(buf)?;
        self.bytes_read += n;
        Ok(n)
    }
}

impl Seek for Counted {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.file.seek(pos)
    }
}

#[test]
fn only_the_suffix_and_bounded_boundary_samples_are_read() {
    let mut file = tempfile::tempfile().unwrap();
    let history = b"{\"type\":\"ignored\"}\n".repeat(512);
    file.write_all(&history).unwrap();
    let meta = file.metadata().unwrap();
    let initial = read_window(&mut file, &meta, None).unwrap().unwrap();
    assert_eq!(initial.bytes, history);
    assert!(!initial.resumed);

    let mut file = Counted {
        file,
        bytes_read: 0,
    };
    let meta = file.file.metadata().unwrap();
    assert!(
        read_window(&mut file, &meta, Some(&initial.position))
            .unwrap()
            .is_none()
    );
    assert!(file.bytes_read <= 512, "read {} bytes", file.bytes_read);

    let new = b"{\"type\":\"user\"}\n";
    file.file.write_all(new).unwrap();
    file.bytes_read = 0;
    let meta = file.file.metadata().unwrap();
    let append = read_window(&mut file, &meta, Some(&initial.position))
        .unwrap()
        .unwrap();
    assert!(append.resumed);
    assert_eq!(append.bytes, new);
    assert!(
        file.bytes_read <= 1024 + new.len(),
        "read {} bytes",
        file.bytes_read
    );

    // A same-inode rewrite with a larger length is still detected when
    // it changes a boundary sample.
    file.file.seek(SeekFrom::Start(0)).unwrap();
    file.file.write_all(b"{}\n").unwrap();
    file.file.seek(SeekFrom::End(0)).unwrap();
    file.file.write_all(new).unwrap();
    let meta = file.file.metadata().unwrap();
    let replay = read_window(&mut file, &meta, Some(&append.position))
        .unwrap()
        .unwrap();
    assert!(!replay.resumed);
    assert!(replay.bytes.starts_with(b"{}\n"));
}
