//! clipsmith: probe/trim/thumbnail via ffmpeg. The crate owns argument
//! construction and ffprobe-JSON parsing (pure, unit-tested); process
//! execution goes through located binaries ($CLIPSMITH_FFMPEG/FFPROBE or
//! PATH). The e2e tests run against SHIM executables that validate argv
//! and produce canned output — the seam is proven without a 500 MB
//! dependency; a gated smoke covers the real thing when installed.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn probe_args(input: &Path) -> Vec<String> {
    vec![
        "-v".into(),
        "error".into(),
        "-print_format".into(),
        "json".into(),
        "-show_format".into(),
        "-show_streams".into(),
        input.display().to_string(),
    ]
}

pub fn trim_args(input: &Path, start_s: f64, end_s: f64, out: &Path) -> Vec<String> {
    vec![
        "-y".into(),
        "-ss".into(),
        format!("{start_s}"),
        "-to".into(),
        format!("{end_s}"),
        "-i".into(),
        input.display().to_string(),
        "-c".into(),
        "copy".into(),
        out.display().to_string(),
    ]
}

pub fn thumb_args(input: &Path, at_s: f64, out: &Path) -> Vec<String> {
    vec![
        "-y".into(),
        "-ss".into(),
        format!("{at_s}"),
        "-i".into(),
        input.display().to_string(),
        "-frames:v".into(),
        "1".into(),
        out.display().to_string(),
    ]
}

#[derive(Debug, PartialEq)]
pub struct Info {
    pub duration_s: f64,
    pub streams: Vec<String>, // "video/h264", "audio/aac", ...
}

pub fn parse_probe(json: &str) -> Result<Info> {
    let v: serde_json::Value = serde_json::from_str(json).context("ffprobe json")?;
    let duration_s = v["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .context("format.duration missing")?;
    let mut streams = Vec::new();
    for s in v["streams"].as_array().context("streams missing")? {
        let kind = s["codec_type"].as_str().unwrap_or("?");
        let codec = s["codec_name"].as_str().unwrap_or("?");
        streams.push(format!("{kind}/{codec}"));
    }
    Ok(Info {
        duration_s,
        streams,
    })
}

pub struct Tool {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

fn which(bin: &str) -> Option<PathBuf> {
    let out = Command::new("which").arg(bin).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

impl Tool {
    pub fn locate() -> Option<Tool> {
        let ffmpeg = std::env::var("CLIPSMITH_FFMPEG")
            .map(PathBuf::from)
            .ok()
            .or_else(|| which("ffmpeg"))?;
        let ffprobe = std::env::var("CLIPSMITH_FFPROBE")
            .map(PathBuf::from)
            .ok()
            .or_else(|| which("ffprobe"))?;
        Some(Tool { ffmpeg, ffprobe })
    }

    fn run(bin: &Path, args: &[String]) -> Result<Vec<u8>> {
        let out = Command::new(bin)
            .args(args)
            .output()
            .with_context(|| format!("spawn {}", bin.display()))?;
        if !out.status.success() {
            bail!(
                "{} failed: {}",
                bin.display(),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(out.stdout)
    }

    pub fn probe(&self, input: &Path) -> Result<Info> {
        let out = Self::run(&self.ffprobe, &probe_args(input))?;
        parse_probe(&String::from_utf8_lossy(&out))
    }

    pub fn trim(&self, input: &Path, start_s: f64, end_s: f64, out: &Path) -> Result<()> {
        if end_s <= start_s {
            bail!("end must be after start");
        }
        Self::run(&self.ffmpeg, &trim_args(input, start_s, end_s, out))?;
        if !out.exists() {
            bail!("ffmpeg reported success but produced no file");
        }
        Ok(())
    }

    pub fn thumbnail(&self, input: &Path, at_s: f64, out: &Path) -> Result<()> {
        Self::run(&self.ffmpeg, &thumb_args(input, at_s, out))?;
        if !out.exists() {
            bail!("ffmpeg reported success but produced no file");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    const CANNED: &str = r#"{
        "format": {"duration": "12.480000"},
        "streams": [
            {"codec_type": "video", "codec_name": "h264"},
            {"codec_type": "audio", "codec_name": "aac"}
        ]
    }"#;

    #[test]
    fn arg_builders_are_exact() {
        let args = trim_args(Path::new("in.mp4"), 1.5, 4.0, Path::new("out.mp4"));
        assert_eq!(
            args,
            ["-y", "-ss", "1.5", "-to", "4", "-i", "in.mp4", "-c", "copy", "out.mp4"]
        );
        let args = probe_args(Path::new("x.mkv"));
        assert_eq!(args[args.len() - 1], "x.mkv");
        assert!(args.contains(&"-show_streams".to_string()));
    }

    #[test]
    fn probe_json_parses_duration_and_streams() {
        let info = parse_probe(CANNED).unwrap();
        assert_eq!(info.duration_s, 12.48);
        assert_eq!(info.streams, ["video/h264", "audio/aac"]);
        assert!(parse_probe("{}").is_err());
    }

    fn td() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "clipsmith-{}-{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[cfg(unix)] // shims are #!/bin/sh scripts
    fn shim(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    #[cfg(unix)]
    fn the_seam_works_end_to_end_against_validating_shims() {
        let d = td();
        // Fake ffprobe: demand the json flags, print canned output.
        let ffprobe = shim(
            &d,
            "ffprobe",
            r#"case "$*" in *"-print_format json"*) ;; *) echo "bad args: $*" >&2; exit 2;; esac
cat <<'JSON'
{"format":{"duration":"12.480000"},"streams":[{"codec_type":"video","codec_name":"h264"}]}
JSON"#,
        );
        // Fake ffmpeg: demand -c copy for trim, then create the last arg.
        let ffmpeg = shim(
            &d,
            "ffmpeg",
            r#"for last in "$@"; do :; done
case "$*" in *"-c copy"*|*"-frames:v 1"*) ;; *) echo "bad args: $*" >&2; exit 2;; esac
: > "$last""#,
        );
        let tool = Tool { ffmpeg, ffprobe };
        let info = tool.probe(Path::new("whatever.mp4")).unwrap();
        assert_eq!(info.duration_s, 12.48);

        let out = d.join("cut.mp4");
        tool.trim(Path::new("in.mp4"), 1.0, 2.0, &out).unwrap();
        assert!(out.exists(), "shim validated argv and produced the file");
        assert!(tool
            .trim(Path::new("in.mp4"), 2.0, 1.0, &d.join("x"))
            .is_err());

        let thumb = d.join("t.png");
        tool.thumbnail(Path::new("in.mp4"), 0.5, &thumb).unwrap();
        assert!(thumb.exists());
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn real_ffmpeg_smoke_when_installed() {
        if std::env::var("CLIPSMITH_FFMPEG").is_ok() {
            return; // don't let shims from other tests masquerade as real
        }
        let Some(tool) = Tool::locate() else {
            eprintln!("ffmpeg not installed; smoke skipped");
            return;
        };
        // Generate 1s of test video with ffmpeg itself, then probe it.
        let d = td();
        let clip = d.join("t.mp4");
        let gen = Command::new(&tool.ffmpeg)
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=1:size=64x64:rate=10",
                clip.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        if !gen.status.success() {
            eprintln!("ffmpeg present but lavfi unavailable; smoke skipped");
            return;
        }
        let info = tool.probe(&clip).unwrap();
        assert!(info.duration_s > 0.5 && info.duration_s < 2.0, "{info:?}");
        let _ = fs::remove_dir_all(d);
    }
}
