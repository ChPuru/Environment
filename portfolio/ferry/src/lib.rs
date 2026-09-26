//! ferry: send a file over TCP; interrupted transfers RESUME from the
//! receiver's byte offset; completion is gated on a whole-file BLAKE3 match.
//! Wire: one JSON header line, then raw bytes from the agreed offset.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
struct Offer {
    name: String,
    len: u64,
    blake3: String,
}

#[derive(Serialize, Deserialize)]
struct Accept {
    offset: u64,
}

pub fn file_hash(path: &Path) -> Result<String> {
    let mut h = blake3::Hasher::new();
    let mut f = File::open(path)?;
    std::io::copy(&mut f, &mut h)?;
    Ok(h.finalize().to_hex().to_string())
}

fn part_paths(dir: &Path, name: &str) -> (PathBuf, PathBuf) {
    (
        dir.join(format!("{name}.part")),
        dir.join(format!("{name}.part.meta")),
    )
}

/// Receiver side: accept ONE transfer on the listener into `dir`.
/// Returns the final path when the hash checks out.
pub fn serve_once(listener: &TcpListener, dir: &Path) -> Result<PathBuf> {
    let (stream, _) = listener.accept()?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let offer: Offer = serde_json::from_str(line.trim()).context("bad offer header")?;
    if offer.name.contains('/') || offer.name.contains("..") || offer.name.is_empty() {
        bail!("unsafe file name in offer");
    }
    let (part, meta) = part_paths(dir, &offer.name);
    // Resume only if the .part belongs to the SAME (hash,len) offer.
    let mut offset = 0u64;
    if let Ok(m) = fs::read_to_string(&meta) {
        if m.trim() == format!("{} {}", offer.blake3, offer.len) {
            offset = fs::metadata(&part)
                .map(|md| md.len())
                .unwrap_or(0)
                .min(offer.len);
        } else {
            let _ = fs::remove_file(&part); // different content: start over
        }
    }
    fs::write(&meta, format!("{} {}\n", offer.blake3, offer.len))?;
    let mut out = OpenOptions::new().create(true).append(true).open(&part)?;
    if out.metadata()?.len() != offset {
        // A stale over-long part after meta mismatch handling: truncate.
        out.set_len(offset)?;
    }
    let mut w = stream;
    let accept = serde_json::to_string(&Accept { offset })? + "\n";
    w.write_all(accept.as_bytes())?;
    w.flush()?;

    let mut remaining = offer.len - offset;
    let mut buf = [0u8; 64 * 1024];
    while remaining > 0 {
        let want = buf.len().min(remaining as usize);
        let n = reader.read(&mut buf[..want])?;
        if n == 0 {
            bail!("sender vanished with {remaining} bytes left (resume later)");
        }
        out.write_all(&buf[..n])?;
        remaining -= n as u64;
    }
    out.sync_all()?;
    drop(out);
    // The gate: whole-file hash or nothing.
    let got = file_hash(&part)?;
    if got != offer.blake3 {
        fs::remove_file(&part)?;
        fs::remove_file(&meta)?;
        bail!(
            "hash mismatch after transfer: got {got}, offered {}",
            offer.blake3
        );
    }
    let final_path = dir.join(&offer.name);
    fs::rename(&part, &final_path)?;
    let _ = fs::remove_file(&meta);
    Ok(final_path)
}

/// Sender side. `limit` (tests only) stops after that many payload bytes to
/// simulate an interruption; None sends to completion.
pub fn send_with_limit(path: &Path, addr: &str, limit: Option<u64>) -> Result<u64> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .context("file needs a UTF-8 name")?
        .to_string();
    let len = fs::metadata(path)?.len();
    let offer = Offer {
        name,
        len,
        blake3: file_hash(path)?,
    };
    let stream = TcpStream::connect(addr)?;
    let mut w = stream.try_clone()?;
    w.write_all((serde_json::to_string(&offer)? + "\n").as_bytes())?;
    w.flush()?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let accept: Accept = serde_json::from_str(line.trim()).context("bad accept")?;
    if accept.offset > len {
        bail!("receiver claims more bytes than the file has");
    }
    let mut f = File::open(path)?;
    f.seek(SeekFrom::Start(accept.offset))?;
    let mut sent = 0u64;
    let mut buf = [0u8; 64 * 1024];
    let mut to_send = len - accept.offset;
    while to_send > 0 {
        let cap = match limit {
            Some(l) if sent >= l => break,
            Some(l) => buf.len().min((l - sent) as usize).min(to_send as usize),
            None => buf.len().min(to_send as usize),
        };
        let n = f.read(&mut buf[..cap])?;
        if n == 0 {
            break;
        }
        w.write_all(&buf[..n])?;
        sent += n as u64;
        to_send -= n as u64;
    }
    w.flush()?;
    Ok(sent)
}

pub fn send(path: &Path, addr: &str) -> Result<u64> {
    send_with_limit(path, addr, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn junk(len: usize, mut x: u64) -> Vec<u8> {
        let mut v = vec![0u8; len];
        for b in v.iter_mut() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *b = x as u8;
        }
        v
    }

    fn setup() -> (tempdir::Dir, PathBuf, TcpListener, String) {
        let td = tempdir::Dir::new();
        let src = td.path.join("payload.bin");
        fs::write(&src, junk(300_000, 7)).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
        (td, src, listener, addr)
    }

    // Tiny tempdir to avoid a dev-dependency.
    mod tempdir {
        use std::path::PathBuf;
        pub struct Dir {
            pub path: PathBuf,
        }
        impl Dir {
            pub fn new() -> Dir {
                let p = std::env::temp_dir().join(format!(
                    "ferry-{}-{:x}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
                std::fs::create_dir_all(&p).unwrap();
                Dir { path: p }
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }
    }

    #[test]
    fn clean_transfer_roundtrips_byte_perfect() {
        let (td, src, listener, addr) = setup();
        let dest_dir = td.path.join("in");
        fs::create_dir(&dest_dir).unwrap();
        let dd = dest_dir.clone();
        let server = thread::spawn(move || serve_once(&listener, &dd).unwrap());
        send(&src, &addr).unwrap();
        let final_path = server.join().unwrap();
        assert_eq!(fs::read(&src).unwrap(), fs::read(final_path).unwrap());
        assert!(!dest_dir.join("payload.bin.part").exists(), "no leftovers");
    }

    #[test]
    fn interrupted_transfer_resumes_from_offset_and_verifies() {
        let (td, src, listener, addr) = setup();
        let dest_dir = td.path.join("in");
        fs::create_dir(&dest_dir).unwrap();

        // First attempt dies after 100k of 300k.
        let dd = dest_dir.clone();
        let server = thread::spawn(move || serve_once(&listener, &dd));
        let sent = send_with_limit(&src, &addr, Some(100_000)).unwrap();
        assert_eq!(sent, 100_000);
        assert!(
            server.join().unwrap().is_err(),
            "receiver must report the cut"
        );
        assert_eq!(
            fs::metadata(dest_dir.join("payload.bin.part"))
                .unwrap()
                .len(),
            100_000,
            "partial bytes kept for resume"
        );

        // Second attempt resumes: sender ships ONLY the remaining 200k.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
        let dd = dest_dir.clone();
        let server = thread::spawn(move || serve_once(&listener, &dd).unwrap());
        let sent = send(&src, &addr).unwrap();
        assert_eq!(sent, 200_000, "resume must not resend delivered bytes");
        let final_path = server.join().unwrap();
        assert_eq!(fs::read(&src).unwrap(), fs::read(final_path).unwrap());
    }

    #[test]
    fn a_changed_file_does_not_resume_onto_old_bytes() {
        let (td, src, listener, addr) = setup();
        let dest_dir = td.path.join("in");
        fs::create_dir(&dest_dir).unwrap();
        let dd = dest_dir.clone();
        let server = thread::spawn(move || serve_once(&listener, &dd));
        send_with_limit(&src, &addr, Some(50_000)).unwrap();
        let _ = server.join().unwrap();

        // Source changes between attempts: meta mismatch => fresh start.
        fs::write(&src, junk(300_000, 99)).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());
        let dd = dest_dir.clone();
        let server = thread::spawn(move || serve_once(&listener, &dd).unwrap());
        let sent = send(&src, &addr).unwrap();
        assert_eq!(sent, 300_000, "different content must restart from zero");
        let final_path = server.join().unwrap();
        assert_eq!(fs::read(&src).unwrap(), fs::read(final_path).unwrap());
    }

    #[test]
    fn unsafe_names_are_refused() {
        let (_td, _src, listener, addr) = setup();
        let dir = std::env::temp_dir();
        let server = thread::spawn(move || serve_once(&listener, &dir));
        let mut s = TcpStream::connect(addr).unwrap();
        s.write_all(b"{\"name\":\"../evil\",\"len\":1,\"blake3\":\"00\"}\n")
            .unwrap();
        assert!(server.join().unwrap().is_err());
    }
}
