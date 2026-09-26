//! textract: batch-OCR a directory into a searchable JSON index. The OCR
//! engine is a trait: the real one shells out to `tesseract` when the
//! binary exists; tests use a fake. The pipeline, index, and search are
//! what this crate owns and proves — the engine seam is honest about it.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

pub trait Engine {
    fn name(&self) -> &str;
    fn ocr(&self, image: &Path) -> Result<String>;
}

pub struct Tesseract {
    pub binary: PathBuf,
}

impl Tesseract {
    /// Found on PATH (or via $TEXTRACT_TESSERACT), if installed.
    pub fn locate() -> Option<Tesseract> {
        if let Ok(p) = std::env::var("TEXTRACT_TESSERACT") {
            return Some(Tesseract {
                binary: PathBuf::from(p),
            });
        }
        let out = Command::new("which").arg("tesseract").output().ok()?;
        if !out.status.success() {
            return None;
        }
        Some(Tesseract {
            binary: PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()),
        })
    }
}

impl Engine for Tesseract {
    fn name(&self) -> &str {
        "tesseract"
    }
    fn ocr(&self, image: &Path) -> Result<String> {
        let out = Command::new(&self.binary)
            .arg(image)
            .arg("stdout")
            .output()
            .with_context(|| format!("spawn {}", self.binary.display()))?;
        if !out.status.success() {
            bail!("tesseract failed: {}", String::from_utf8_lossy(&out.stderr));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct Doc {
    pub path: String,
    pub text: String,
}

#[derive(Serialize, Deserialize, Debug, Default, PartialEq)]
pub struct Index {
    pub engine: String,
    pub docs: Vec<Doc>,
}

const EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "tif", "tiff", "bmp", "pbm", "pgm", "ppm",
];

/// OCR every image under `dir` (recursively) into an Index. Per-file
/// failures are recorded as errors in the doc text, not silently dropped.
pub fn run(engine: &dyn Engine, dir: &Path) -> Result<Index> {
    let mut idx = Index {
        engine: engine.name().to_string(),
        docs: Vec::new(),
    };
    let mut stack = vec![dir.to_path_buf()];
    let mut files = Vec::new();
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)? {
            let p = e?.path();
            if p.is_dir() {
                stack.push(p);
            } else if p
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| EXTS.contains(&x.to_ascii_lowercase().as_str()))
                .unwrap_or(false)
            {
                files.push(p);
            }
        }
    }
    files.sort();
    for f in files {
        let text = match engine.ocr(&f) {
            Ok(t) => t,
            Err(e) => format!("[textract error: {e}]"),
        };
        idx.docs.push(Doc {
            path: f.display().to_string(),
            text,
        });
    }
    Ok(idx)
}

pub struct Hit<'a> {
    pub path: &'a str,
    pub line: usize,
    pub snippet: String,
}

/// Case-insensitive search with line numbers and snippets.
pub fn search<'a>(idx: &'a Index, query: &str) -> Vec<Hit<'a>> {
    let q = query.to_ascii_lowercase();
    let mut hits = Vec::new();
    for d in &idx.docs {
        for (n, line) in d.text.lines().enumerate() {
            if line.to_ascii_lowercase().contains(&q) {
                hits.push(Hit {
                    path: &d.path,
                    line: n + 1,
                    snippet: line.trim().to_string(),
                });
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Fake;
    impl Engine for Fake {
        fn name(&self) -> &str {
            "fake"
        }
        fn ocr(&self, image: &Path) -> Result<String> {
            let name = image.file_name().unwrap().to_string_lossy();
            if name.contains("broken") {
                bail!("cannot read pixels");
            }
            Ok(format!("Invoice for {name}\nTotal Due: 42\n"))
        }
    }

    fn td() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "textract-{}-{:x}",
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
    fn pipeline_indexes_recursively_and_search_finds_lines() {
        let d = td();
        fs::create_dir_all(d.join("sub")).unwrap();
        fs::write(d.join("a.png"), b"img").unwrap();
        fs::write(d.join("sub/b.jpg"), b"img").unwrap();
        fs::write(d.join("notes.txt"), b"not an image").unwrap();

        let idx = run(&Fake, &d).unwrap();
        assert_eq!(idx.docs.len(), 2, "only image extensions, recursively");
        let hits = search(&idx, "total due");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].line, 2);
        assert!(hits[0].snippet.contains("42"));

        // Roundtrip through JSON: the index is a durable artifact.
        let json = serde_json::to_string(&idx).unwrap();
        let back: Index = serde_json::from_str(&json).unwrap();
        assert_eq!(back, idx);
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn engine_failures_are_recorded_not_dropped() {
        let d = td();
        fs::write(d.join("broken.png"), b"img").unwrap();
        let idx = run(&Fake, &d).unwrap();
        assert_eq!(idx.docs.len(), 1);
        assert!(
            idx.docs[0].text.contains("textract error"),
            "{}",
            idx.docs[0].text
        );
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn real_tesseract_smoke_when_installed() {
        // Gated: proves the seam against the real binary when present;
        // absent, the fake-engine tests above carry the suite.
        let Some(t) = Tesseract::locate() else {
            eprintln!("tesseract not installed; smoke test skipped");
            return;
        };
        // A blank white raw PBM (P4): tesseract should run and return
        // (near-)empty text.
        let d = td();
        let p = d.join("blank.pbm");
        let mut pbm = b"P4\n40 20\n".to_vec();
        pbm.extend(std::iter::repeat(0u8).take(5 * 20)); // 40 bits x 20 rows
        fs::write(&p, pbm).unwrap();
        let out = t.ocr(&p);
        assert!(out.is_ok(), "{out:?}");
        let _ = fs::remove_dir_all(d);
    }
}
