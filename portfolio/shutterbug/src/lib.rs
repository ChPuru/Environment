//! shutterbug: file photos into DST/YYYY/MM by their EXIF
//! DateTimeOriginal — parsed from JPEG bytes directly (APP1/TIFF walk,
//! both endians), no C bindings. Identical files (BLAKE3) dedup to a
//! skip; name collisions with different bytes get -1, -2 suffixes.
//! No EXIF -> file mtime, honestly labeled in the plan.

use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

fn u16_at(b: &[u8], i: usize, le: bool) -> Option<u16> {
    let s: [u8; 2] = b.get(i..i + 2)?.try_into().ok()?;
    Some(if le {
        u16::from_le_bytes(s)
    } else {
        u16::from_be_bytes(s)
    })
}
fn u32_at(b: &[u8], i: usize, le: bool) -> Option<u32> {
    let s: [u8; 4] = b.get(i..i + 4)?.try_into().ok()?;
    Some(if le {
        u32::from_le_bytes(s)
    } else {
        u32::from_be_bytes(s)
    })
}

/// EXIF DateTimeOriginal "YYYY:MM:DD HH:MM:SS" -> (year, month), if present.
pub fn exif_year_month(jpeg: &[u8]) -> Option<(i32, u32)> {
    if jpeg.get(0..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut i = 2;
    while i + 4 <= jpeg.len() {
        if jpeg[i] != 0xFF {
            return None;
        }
        let marker = jpeg[i + 1];
        let seg_len = u16_at(jpeg, i + 2, false)? as usize;
        if marker == 0xE1 && jpeg.get(i + 4..i + 10)? == b"Exif\0\0" {
            return tiff_year_month(jpeg.get(i + 10..i + 2 + seg_len)?);
        }
        if marker == 0xDA {
            return None; // image data; no EXIF ahead
        }
        i += 2 + seg_len;
    }
    None
}

fn tiff_year_month(t: &[u8]) -> Option<(i32, u32)> {
    let le = match t.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    if u16_at(t, 2, le)? != 42 {
        return None;
    }
    let ifd0 = u32_at(t, 4, le)? as usize;
    let exif_ifd = find_tag(t, ifd0, 0x8769, le).map(|(_, v)| v as usize)?;
    let (count, val_off) = find_tag(t, exif_ifd, 0x9003, le)?;
    if count < 19 {
        return None;
    }
    let s = t.get(val_off as usize..val_off as usize + 19)?;
    let s = std::str::from_utf8(s).ok()?;
    let year: i32 = s.get(0..4)?.parse().ok()?;
    let month: u32 = s.get(5..7)?.parse().ok()?;
    if !(1..=12).contains(&month) {
        return None;
    }
    Some((year, month))
}

/// Returns (count, value-or-offset) for `tag` in the IFD at `off`.
fn find_tag(t: &[u8], off: usize, tag: u16, le: bool) -> Option<(u32, u32)> {
    let n = u16_at(t, off, le)? as usize;
    for e in 0..n {
        let base = off + 2 + e * 12;
        if u16_at(t, base, le)? == tag {
            let count = u32_at(t, base + 4, le)?;
            let val = u32_at(t, base + 8, le)?;
            return Some((count, val));
        }
    }
    None
}

#[derive(Debug, PartialEq)]
pub enum Action {
    Move { to: PathBuf, dated_by_exif: bool },
    SkipIdentical,
}

fn mtime_year_month(p: &Path) -> Result<(i32, u32)> {
    let secs = fs::metadata(p)?
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // civil-from-days, year+month only
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    Ok(((if m <= 2 { y + 1 } else { y }) as i32, m))
}

fn hash(p: &Path) -> Result<blake3::Hash> {
    let mut h = blake3::Hasher::new();
    std::io::copy(&mut fs::File::open(p)?, &mut h)?;
    Ok(h.finalize())
}

/// Decide what should happen to `photo` under `dst`. Pure planning.
pub fn plan_one(photo: &Path, dst: &Path) -> Result<Action> {
    let bytes = fs::read(photo)?;
    let (ym, dated_by_exif) = match exif_year_month(&bytes) {
        Some(ym) => (ym, true),
        None => (mtime_year_month(photo)?, false),
    };
    let dir = dst
        .join(format!("{:04}", ym.0))
        .join(format!("{:02}", ym.1));
    let name = photo
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("photo.jpg");
    let mut candidate = dir.join(name);
    let mut n = 0;
    while candidate.exists() {
        if hash(&candidate)? == hash(photo)? {
            return Ok(Action::SkipIdentical);
        }
        n += 1;
        let stem = Path::new(name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("photo");
        let ext = Path::new(name)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("jpg");
        candidate = dir.join(format!("{stem}-{n}.{ext}"));
    }
    Ok(Action::Move {
        to: candidate,
        dated_by_exif,
    })
}

pub fn apply_one(photo: &Path, action: &Action) -> Result<()> {
    if let Action::Move { to, .. } = action {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(photo, to).or_else(|_| fs::copy(photo, to).map(|_| ()))?;
    }
    Ok(())
}

/// Build a minimal-but-valid JPEG with an EXIF DateTimeOriginal. Public so
/// tests and demos share one honest fixture generator.
pub fn synthetic_jpeg(date: &str, le: bool) -> Vec<u8> {
    assert_eq!(date.len(), 19, "YYYY:MM:DD HH:MM:SS");
    // TIFF block: header(8) + IFD0(2 + 12 + 4) + ExifIFD(2 + 12 + 4) + date(20)
    let ifd0 = 8u32;
    let exif_ifd = ifd0 + 2 + 12 + 4;
    let date_off = exif_ifd + 2 + 12 + 4;
    let mut t = Vec::new();
    type W16 = fn(&mut Vec<u8>, u16);
    type W32 = fn(&mut Vec<u8>, u32);
    let (w16, w32): (W16, W32) = if le {
        (
            |v, x| v.extend(x.to_le_bytes()),
            |v, x| v.extend(x.to_le_bytes()),
        )
    } else {
        (
            |v, x| v.extend(x.to_be_bytes()),
            |v, x| v.extend(x.to_be_bytes()),
        )
    };
    t.extend(if le { b"II" } else { b"MM" });
    w16(&mut t, 42);
    w32(&mut t, ifd0);
    // IFD0: one entry -> ExifIFD pointer
    w16(&mut t, 1);
    w16(&mut t, 0x8769);
    w16(&mut t, 4); // LONG
    w32(&mut t, 1);
    w32(&mut t, exif_ifd);
    w32(&mut t, 0); // next IFD
                    // ExifIFD: one entry -> DateTimeOriginal
    w16(&mut t, 1);
    w16(&mut t, 0x9003);
    w16(&mut t, 2); // ASCII
    w32(&mut t, 20);
    w32(&mut t, date_off);
    w32(&mut t, 0);
    t.extend(date.as_bytes());
    t.push(0);
    // Wrap in JPEG: SOI + APP1(Exif) + EOI
    let mut j = vec![0xFF, 0xD8, 0xFF, 0xE1];
    let seg_len = (2 + 6 + t.len()) as u16;
    j.extend(seg_len.to_be_bytes());
    j.extend(b"Exif\0\0");
    j.extend(&t);
    j.extend([0xFF, 0xD9]);
    j
}

#[cfg(test)]
mod tests {
    use super::*;

    fn td() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "shutterbug-{}-{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn exif_dates_parse_in_both_endiannesses() {
        for le in [true, false] {
            let j = synthetic_jpeg("2019:08:23 14:00:00", le);
            assert_eq!(exif_year_month(&j), Some((2019, 8)), "le={le}");
        }
        assert_eq!(exif_year_month(b"not a jpeg"), None);
    }

    #[test]
    fn organizing_dedups_identical_and_suffixes_different() {
        let base = td();
        let (src, dst) = (base.join("src"), base.join("dst"));
        fs::create_dir_all(&src).unwrap();
        let a = src.join("pic.jpg");
        fs::write(&a, synthetic_jpeg("2019:08:23 14:00:00", true)).unwrap();

        let act = plan_one(&a, &dst).unwrap();
        match &act {
            Action::Move { to, dated_by_exif } => {
                assert!(to.ends_with("2019/08/pic.jpg"), "{}", to.display());
                assert!(dated_by_exif);
            }
            other => panic!("{other:?}"),
        }
        apply_one(&a, &act).unwrap();

        // Same bytes again: skip.
        let b = src.join("pic.jpg");
        fs::write(&b, synthetic_jpeg("2019:08:23 14:00:00", true)).unwrap();
        assert_eq!(plan_one(&b, &dst).unwrap(), Action::SkipIdentical);

        // Same name+month, different seconds => different bytes: suffix.
        fs::write(&b, synthetic_jpeg("2019:08:23 14:00:01", true)).unwrap();
        match plan_one(&b, &dst).unwrap() {
            Action::Move { to, .. } => assert!(to.ends_with("2019/08/pic-1.jpg")),
            other => panic!("{other:?}"),
        }
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn missing_exif_falls_back_to_mtime_and_says_so() {
        let base = td();
        let p = base.join("noexif.jpg");
        fs::write(&p, b"\xFF\xD8\xFF\xD9").unwrap();
        match plan_one(&p, &base.join("dst")).unwrap() {
            Action::Move { dated_by_exif, .. } => assert!(!dated_by_exif),
            other => panic!("{other:?}"),
        }
        let _ = fs::remove_dir_all(base);
    }
}
