//! In-process S3 mock: the conformance fixture. Speaks the exact subset the
//! backend uses (PUT/GET/HEAD/DELETE, ListObjectsV2 with FORCED pagination,
//! Range with S3's end-clamping semantics, If-None-Match:*), and VERIFIES
//! every request's SigV4 signature — a wrong secret gets a 403, so every
//! test through this mock exercises real signing, not just transport.
//!
//! Test fixture by design: the acceptor thread parks forever when the last
//! MockS3 handle drops mid-request; tests end with the process.

use super::sigv4::{self, SigningKey};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

const PAGE_SIZE: usize = 2; // tiny on purpose: pagination is exercised, always

pub struct MockS3 {
    pub port: u16,
    pub bucket: String,
    pub access_key: String,
    pub secret_key: String,
    pub region: String,
}

struct State {
    objects: BTreeMap<String, Vec<u8>>,
}

impl MockS3 {
    pub fn start(bucket: &str, access_key: &str, secret_key: &str, region: &str) -> MockS3 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock s3");
        let port = listener.local_addr().expect("addr").port();
        let state = Arc::new(Mutex::new(State {
            objects: BTreeMap::new(),
        }));
        let m = MockS3 {
            port,
            bucket: bucket.to_string(),
            access_key: access_key.to_string(),
            secret_key: secret_key.to_string(),
            region: region.to_string(),
        };
        let (bucket, ak, sk, rg) = (
            m.bucket.clone(),
            m.access_key.clone(),
            m.secret_key.clone(),
            m.region.clone(),
        );
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(stream) = conn else { continue };
                let state = Arc::clone(&state);
                let (bucket, ak, sk, rg) = (bucket.clone(), ak.clone(), sk.clone(), rg.clone());
                std::thread::spawn(move || {
                    let _ = handle(stream, &state, &bucket, &ak, &sk, &rg);
                });
            }
        });
        m
    }

    pub fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

fn respond(s: &mut TcpStream, status: &str, headers: &[(&str, String)], body: &[u8]) {
    let mut out = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n", body.len());
    for (k, v) in headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("Connection: close\r\n\r\n");
    let _ = s.write_all(out.as_bytes());
    let _ = s.write_all(body);
    let _ = s.flush();
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[allow(clippy::too_many_lines)]
fn handle(
    mut stream: TcpStream,
    state: &Mutex<State>,
    bucket: &str,
    access_key: &str,
    secret_key: &str,
    region: &str,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("").to_string();
    let (path, rawquery) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target.clone(), String::new()),
    };

    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    loop {
        let mut h = String::new();
        reader.read_line(&mut h)?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let clen: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; clen];
    reader.read_exact(&mut body)?;

    // ---- SigV4 verification: recompute and compare the whole header. ----
    let Some(auth) = headers.get("authorization").cloned() else {
        respond(&mut stream, "403 Forbidden", &[], b"no auth");
        return Ok(());
    };
    let claimed_hash = headers
        .get("x-amz-content-sha256")
        .cloned()
        .unwrap_or_default();
    if sigv4::sha256_hex(&body) != claimed_hash {
        respond(&mut stream, "403 Forbidden", &[], b"payload hash mismatch");
        return Ok(());
    }
    let signed_list = auth
        .split("SignedHeaders=")
        .nth(1)
        .and_then(|s| s.split(',').next())
        .unwrap_or("")
        .to_string();
    let mut to_sign: BTreeMap<String, String> = BTreeMap::new();
    for name in signed_list.split(';') {
        if let Some(v) = headers.get(name) {
            to_sign.insert(name.to_string(), v.clone());
        }
    }
    let amz_date = headers.get("x-amz-date").cloned().unwrap_or_default();
    let date = amz_date.get(..8).unwrap_or("").to_string();
    let mut qpairs: Vec<(String, String)> = rawquery
        .split('&')
        .filter(|s| !s.is_empty())
        .map(|kv| match kv.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => (kv.to_string(), String::new()),
        })
        .collect();
    qpairs.sort();
    let canonical_query = qpairs
        .iter()
        .map(|(k, v)| format!("{k}={v}")) // client sent pre-encoded pairs
        .collect::<Vec<_>>()
        .join("&");
    let key = SigningKey {
        access_key,
        secret_key,
        region,
    };
    let expect = sigv4::authorization(
        &key,
        &method,
        &path,
        &canonical_query,
        &to_sign,
        &claimed_hash,
        &date,
        &amz_date,
    );
    if expect != auth {
        respond(&mut stream, "403 Forbidden", &[], b"signature mismatch");
        return Ok(());
    }

    // ---- Routing (path-style: /bucket[/key]) ----
    let bucket_prefix = format!("/{bucket}");
    let Some(rest) = path.strip_prefix(&bucket_prefix) else {
        respond(&mut stream, "404 Not Found", &[], b"no such bucket");
        return Ok(());
    };
    let object_key = rest.strip_prefix('/').unwrap_or("").to_string();
    let mut st = state.lock().expect("mock state");

    // ListObjectsV2
    if method == "GET" && rawquery.contains("list-type=2") {
        let mut prefix = String::new();
        let mut token = String::new();
        for (k, v) in &qpairs {
            let dec = v.replace("%2F", "/"); // our keys only need '/' decoding
            if k == "prefix" {
                prefix = dec;
            } else if k == "continuation-token" {
                token = dec;
            }
        }
        let keys: Vec<&String> = st
            .objects
            .keys()
            .filter(|k| k.starts_with(&prefix) && (token.is_empty() || **k > token))
            .take(PAGE_SIZE + 1)
            .collect();
        let truncated = keys.len() > PAGE_SIZE;
        let page = &keys[..keys.len().min(PAGE_SIZE)];
        let mut xml = String::from("<?xml version=\"1.0\"?><ListBucketResult>");
        for k in page {
            xml.push_str(&format!(
                "<Contents><Key>{}</Key></Contents>",
                xml_escape(k)
            ));
        }
        xml.push_str(&format!("<IsTruncated>{truncated}</IsTruncated>"));
        if truncated {
            if let Some(last) = page.last() {
                xml.push_str(&format!(
                    "<NextContinuationToken>{}</NextContinuationToken>",
                    xml_escape(last)
                ));
            }
        }
        xml.push_str("</ListBucketResult>");
        respond(&mut stream, "200 OK", &[], xml.as_bytes());
        return Ok(());
    }

    match method.as_str() {
        "PUT" => {
            if headers.get("if-none-match").map(String::as_str) == Some("*")
                && st.objects.contains_key(&object_key)
            {
                respond(&mut stream, "412 Precondition Failed", &[], b"");
            } else {
                st.objects.insert(object_key, body);
                respond(&mut stream, "200 OK", &[], b"");
            }
        }
        "DELETE" => {
            st.objects.remove(&object_key);
            respond(&mut stream, "204 No Content", &[], b"");
        }
        "GET" | "HEAD" => match st.objects.get(&object_key) {
            None => respond(&mut stream, "404 Not Found", &[], b""),
            Some(data) => {
                if let Some(range) = headers.get("range") {
                    // bytes=a-b with S3 semantics: start past end -> 416;
                    // end CLAMPS to len-1 (the trap our backend must catch).
                    let spec = range.trim_start_matches("bytes=");
                    let (a, b) = spec.split_once('-').unwrap_or(("0", ""));
                    let start: usize = a.parse().unwrap_or(0);
                    if start >= data.len() {
                        respond(&mut stream, "416 Range Not Satisfiable", &[], b"");
                        return Ok(());
                    }
                    let end: usize = b
                        .parse::<usize>()
                        .map(|e| e.min(data.len() - 1))
                        .unwrap_or(data.len() - 1);
                    let slice = &data[start..=end];
                    let cr = format!("bytes {start}-{end}/{}", data.len());
                    let body: &[u8] = if method == "HEAD" { b"" } else { slice };
                    respond(
                        &mut stream,
                        "206 Partial Content",
                        &[("Content-Range", cr)],
                        body,
                    );
                } else if method == "HEAD" {
                    // HEAD: Content-Length must reflect the object, body empty.
                    let out = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        data.len()
                    );
                    let _ = stream.write_all(out.as_bytes());
                    let _ = stream.flush();
                } else {
                    respond(&mut stream, "200 OK", &[], data);
                }
            }
        },
        _ => respond(&mut stream, "405 Method Not Allowed", &[], b""),
    }
    Ok(())
}
