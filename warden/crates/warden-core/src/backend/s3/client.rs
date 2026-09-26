//! The S3 Backend: path-style requests, same key layout as local, SigV4 on
//! every call. Every response is checked for EXACTLY what we asked for —
//! S3's Range end-clamping in particular is converted into our LOUD
//! short-range contract, never partial data.

use super::sigv4::{self, SigningKey};
use crate::backend::{range_err, Backend, Kind};
use crate::error::{Error, Result};
use crate::object::{Hash256, StoredName};
use std::collections::BTreeMap;
use std::io::Read;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct S3Config {
    /// e.g. "http://127.0.0.1:9000" or "https://s3.eu-central-1.amazonaws.com"
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
    /// Key prefix inside the bucket; "" or "some/prefix/" (trailing slash).
    pub prefix: String,
}

pub struct S3Backend {
    cfg: S3Config,
    host: String,
    agent: ureq::Agent,
}

impl S3Backend {
    pub fn new(cfg: S3Config) -> Result<S3Backend> {
        let host = cfg
            .endpoint
            .split("://")
            .nth(1)
            .ok_or_else(|| Error::Format(format!("bad s3 endpoint: {}", cfg.endpoint)))?
            .trim_end_matches('/')
            .to_string();
        let agent = ureq::AgentBuilder::new()
            .timeout_read(Duration::from_secs(30))
            .timeout_write(Duration::from_secs(30))
            .build();
        Ok(S3Backend { cfg, host, agent })
    }

    fn object_key(&self, kind: Kind, name: &StoredName) -> String {
        let hexn = name.0.to_hex();
        match kind {
            Kind::Object => format!("{}{}/{}/{}", self.cfg.prefix, kind.dir(), &hexn[..2], hexn),
            _ => format!("{}{}/{}", self.cfg.prefix, kind.dir(), hexn),
        }
    }

    /// One signed request. `query` pairs must be pre-sorted; both sides get
    /// uri-encoded here. Extra headers are signed too.
    fn send(
        &self,
        method: &str,
        key: &str,
        query: &[(&str, &str)],
        extra_headers: &[(&str, String)],
        body: &[u8],
    ) -> std::result::Result<ureq::Response, Box<ureq::Error>> {
        let canonical_uri = format!("/{}/{}", self.cfg.bucket, sigv4::uri_encode(key, true));
        let canonical_query = query
            .iter()
            .map(|(k, v)| {
                format!(
                    "{}={}",
                    sigv4::uri_encode(k, false),
                    sigv4::uri_encode(v, false)
                )
            })
            .collect::<Vec<_>>()
            .join("&");
        let payload_hash = sigv4::sha256_hex(body);
        let (date, ts) = sigv4::timestamp(now_unix());
        let mut headers: BTreeMap<String, String> = BTreeMap::new();
        headers.insert("host".into(), self.host.clone());
        headers.insert("x-amz-content-sha256".into(), payload_hash.clone());
        headers.insert("x-amz-date".into(), ts.clone());
        for (k, v) in extra_headers {
            headers.insert((*k).to_string(), v.clone());
        }
        let sk = SigningKey {
            access_key: &self.cfg.access_key,
            secret_key: &self.cfg.secret_key,
            region: &self.cfg.region,
        };
        let auth = sigv4::authorization(
            &sk,
            method,
            &canonical_uri,
            &canonical_query,
            &headers,
            &payload_hash,
            &date,
            &ts,
        );
        let mut url = format!(
            "{}{}",
            self.cfg.endpoint.trim_end_matches('/'),
            canonical_uri
        );
        if !canonical_query.is_empty() {
            url.push('?');
            url.push_str(&canonical_query);
        }
        let mut req = self.agent.request(method, &url);
        for (k, v) in &headers {
            if k != "host" {
                req = req.set(k, v); // ureq sets Host itself from the URL
            }
        }
        req = req.set("authorization", &auth);
        req.send_bytes(body).map_err(Box::new)
    }

    fn read_body(resp: ureq::Response) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        resp.into_reader()
            .read_to_end(&mut out)
            .map_err(|e| Error::Format(format!("s3 body read: {e}")))?;
        Ok(out)
    }
}

fn s3_err(what: &str, e: Box<ureq::Error>) -> Error {
    Error::Format(format!("s3 {what}: {e}"))
}

fn status(e: &ureq::Error) -> Option<u16> {
    match e {
        ureq::Error::Status(code, _) => Some(*code),
        ureq::Error::Transport(_) => None,
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Backend for S3Backend {
    fn put(&self, kind: Kind, name: &StoredName, data: &[u8]) -> Result<()> {
        let key = self.object_key(kind, name);
        match self.send("PUT", &key, &[], &[], data) {
            Ok(_) => Ok(()),
            Err(e) => Err(s3_err("put", e)),
        }
    }

    fn get(&self, kind: Kind, name: &StoredName) -> Result<Vec<u8>> {
        let key = self.object_key(kind, name);
        match self.send("GET", &key, &[], &[], b"") {
            Ok(resp) => Self::read_body(resp),
            Err(e) if status(&e) == Some(404) => Err(Error::NotFound(format!(
                "{}/{}",
                kind.dir(),
                name.0.to_hex()
            ))),
            Err(e) => Err(s3_err("get", e)),
        }
    }

    fn exists(&self, kind: Kind, name: &StoredName) -> Result<bool> {
        let key = self.object_key(kind, name);
        match self.send("HEAD", &key, &[], &[], b"") {
            Ok(_) => Ok(true),
            Err(e) if status(&e) == Some(404) => Ok(false),
            Err(e) => Err(s3_err("head", e)),
        }
    }

    fn get_range(&self, kind: Kind, name: &StoredName, offset: u64, len: u64) -> Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new()); // canonical degenerate case, no IO
        }
        let key = self.object_key(kind, name);
        let range = format!("bytes={}-{}", offset, offset + len - 1);
        match self.send("GET", &key, &[], &[("range", range)], b"") {
            Ok(resp) => {
                let body = Self::read_body(resp)?;
                // S3 CLAMPS ranges past the end and can answer 200-full for
                // ignored Range headers. Anything but exactly `len` bytes is
                // a short/overlong range: LOUD, never partial.
                if body.len() as u64 != len {
                    return Err(range_err(name));
                }
                Ok(body)
            }
            Err(e) if status(&e) == Some(416) => Err(range_err(name)),
            Err(e) if status(&e) == Some(404) => Err(Error::NotFound(format!(
                "{}/{}",
                kind.dir(),
                name.0.to_hex()
            ))),
            Err(e) => Err(s3_err("get_range", e)),
        }
    }

    fn list(&self, kind: Kind) -> Result<Vec<StoredName>> {
        let prefix = format!("{}{}/", self.cfg.prefix, kind.dir());
        let mut token: Option<String> = None;
        let mut out = Vec::new();
        loop {
            // Query pairs pre-sorted: continuation-token < list-type < prefix.
            let mut q: Vec<(&str, &str)> = Vec::new();
            if let Some(t) = &token {
                q.push(("continuation-token", t));
            }
            q.push(("list-type", "2"));
            q.push(("prefix", &prefix));
            let resp = match self.send("GET", "", &q, &[], b"") {
                Ok(r) => r,
                Err(e) => return Err(s3_err("list", e)),
            };
            let xml = String::from_utf8_lossy(&Self::read_body(resp)?).into_owned();
            for part in xml.split("<Key>").skip(1) {
                let Some(k) = part.split("</Key>").next() else {
                    continue;
                };
                let Some(short) = k.strip_prefix(&prefix) else {
                    continue;
                };
                let hexn = match kind {
                    Kind::Object => match short.split_once('/') {
                        Some((_fanout, h)) => h,
                        None => continue,
                    },
                    _ => short,
                };
                if let Ok(h) = Hash256::from_hex(hexn) {
                    out.push(StoredName(h));
                }
            }
            let truncated = xml.contains("<IsTruncated>true</IsTruncated>");
            if !truncated {
                break;
            }
            token = xml
                .split("<NextContinuationToken>")
                .nth(1)
                .and_then(|s| s.split("</NextContinuationToken>").next())
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        Ok(out)
    }

    fn put_meta(&self, rel_path: &str, data: &[u8], overwrite: bool) -> Result<()> {
        let key = format!("{}{}", self.cfg.prefix, rel_path);
        let extra: Vec<(&str, String)> = if overwrite {
            vec![]
        } else {
            vec![("if-none-match", "*".into())]
        };
        match self.send("PUT", &key, &[], &extra, data) {
            Ok(_) => Ok(()),
            Err(e) if status(&e) == Some(412) => Err(Error::AlreadyExists(rel_path.to_string())),
            Err(e) => Err(s3_err("put_meta", e)),
        }
    }

    fn get_meta(&self, rel_path: &str) -> Result<Vec<u8>> {
        let key = format!("{}{}", self.cfg.prefix, rel_path);
        match self.send("GET", &key, &[], &[], b"") {
            Ok(resp) => Self::read_body(resp),
            Err(e) if status(&e) == Some(404) => Err(Error::NotFound(rel_path.to_string())),
            Err(e) => Err(s3_err("get_meta", e)),
        }
    }

    fn size(&self, kind: Kind, name: &StoredName) -> Result<u64> {
        let key = self.object_key(kind, name);
        match self.send("HEAD", &key, &[], &[], b"") {
            Ok(resp) => resp
                .header("content-length")
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| Error::Format("s3 HEAD without content-length".into())),
            Err(e) if status(&e) == Some(404) => Err(Error::NotFound(format!(
                "{}/{}",
                kind.dir(),
                name.0.to_hex()
            ))),
            Err(e) => Err(s3_err("size", e)),
        }
    }

    fn delete(&self, kind: Kind, name: &StoredName) -> Result<()> {
        let key = self.object_key(kind, name);
        match self.send("DELETE", &key, &[], &[], b"") {
            Ok(_) => Ok(()),
            Err(e) if status(&e) == Some(404) => Ok(()), // idempotent
            Err(e) => Err(s3_err("delete", e)),
        }
    }

    fn delete_meta(&self, rel_path: &str) -> Result<()> {
        let key = format!("{}{}", self.cfg.prefix, rel_path);
        match self.send("DELETE", &key, &[], &[], b"") {
            Ok(_) => Ok(()),
            Err(e) if status(&e) == Some(404) => Ok(()), // idempotent
            Err(e) => Err(s3_err("delete_meta", e)),
        }
    }
}
