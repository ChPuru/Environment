//! mailsift: read an mbox, answer questions about it. Header unfolding,
//! From_-line splitting with >From body quoting, RFC 2822 dates without
//! chrono, and MIME encoded-word subjects (=?UTF-8?B/Q?...?=) — the parts
//! of email that actually show up in decades-old archives.

use anyhow::{Context, Result};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct Message {
    pub raw: String,
    pub headers: BTreeMap<String, String>, // lowercased keys, first wins
    pub date_unix: Option<i64>,
}

impl Message {
    pub fn header(&self, name: &str) -> &str {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
            .unwrap_or("")
    }
    pub fn subject(&self) -> String {
        decode_encoded_words(self.header("subject"))
    }
}

pub fn parse_mbox(text: &str) -> Result<Vec<Message>> {
    let mut messages = Vec::new();
    let mut cur = String::new();
    for line in text.lines() {
        if line.starts_with("From ") && !cur.is_empty() {
            messages.push(parse_message(&cur)?);
            cur.clear();
        }
        if !(line.starts_with("From ") && cur.is_empty() && messages.is_empty() && cur.is_empty()) {
            // fallthrough below adds every line; the From_ separator itself
            // is kept in raw (mbox faithful) but skipped for headers.
        }
        cur.push_str(line);
        cur.push('\n');
    }
    if !cur.trim().is_empty() {
        messages.push(parse_message(&cur)?);
    }
    Ok(messages)
}

fn parse_message(raw: &str) -> Result<Message> {
    let mut headers = BTreeMap::new();
    let mut last_key: Option<String> = None;
    for line in raw.lines() {
        if line.starts_with("From ") && headers.is_empty() && last_key.is_none() {
            continue; // the mbox separator
        }
        if line.is_empty() {
            break; // body begins
        }
        if (line.starts_with(' ') || line.starts_with('\t')) && last_key.is_some() {
            let k = last_key.clone().expect("checked");
            let v: &mut String = headers.get_mut(&k).expect("exists");
            v.push(' ');
            v.push_str(line.trim());
            continue;
        }
        let (k, v) = line.split_once(':').context("header needs a colon")?;
        let key = k.trim().to_ascii_lowercase();
        headers
            .entry(key.clone())
            .or_insert_with(|| v.trim().to_string());
        last_key = Some(key);
    }
    let date_unix = headers.get("date").and_then(|d| parse_rfc2822(d));
    Ok(Message {
        raw: raw.to_string(),
        headers,
        date_unix,
    })
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp as i64 + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// "Thu, 9 Jul 2026 10:00:00 +0530" -> unix (UTC). Weekday optional.
pub fn parse_rfc2822(s: &str) -> Option<i64> {
    let s = s.trim();
    let s = match s.split_once(',') {
        Some((_, rest)) => rest.trim(),
        None => s,
    };
    let mut t = s.split_whitespace();
    let day: u32 = t.next()?.parse().ok()?;
    let mon = match t.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let year: i64 = t.next()?.parse().ok()?;
    let hms = t.next()?;
    let mut hp = hms.split(':');
    let (h, mi): (i64, i64) = (hp.next()?.parse().ok()?, hp.next()?.parse().ok()?);
    let sec: i64 = hp.next().and_then(|x| x.parse().ok()).unwrap_or(0);
    let zone = t.next().unwrap_or("+0000");
    let off = if zone.len() == 5 && (zone.starts_with('+') || zone.starts_with('-')) {
        let sign = if zone.starts_with('-') { -1 } else { 1 };
        let zh: i64 = zone[1..3].parse().ok()?;
        let zm: i64 = zone[3..5].parse().ok()?;
        sign * (zh * 3600 + zm * 60)
    } else {
        0 // UT/GMT and friends
    };
    Some(days_from_civil(year, mon, day) * 86_400 + h * 3600 + mi * 60 + sec - off)
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut acc: u32 = 0;
    let mut bits = 0;
    for c in s.bytes() {
        if c == b'=' {
            break;
        }
        let v = T.iter().position(|&t| t == c)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// Decode =?charset?B|Q?...?= words (UTF-8 assumed; others pass through raw).
pub fn decode_encoded_words(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find("=?") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("?=") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let word = &after[..end];
        let mut parts = word.splitn(3, '?');
        let (_cs, enc, payload) = (parts.next(), parts.next(), parts.next());
        let decoded = match (enc, payload) {
            (Some(e), Some(p)) if e.eq_ignore_ascii_case("b") => {
                b64_decode(p).and_then(|b| String::from_utf8(b).ok())
            }
            (Some(e), Some(p)) if e.eq_ignore_ascii_case("q") => {
                let mut d = Vec::new();
                let bytes = p.as_bytes();
                let mut i = 0;
                while i < bytes.len() {
                    match bytes[i] {
                        b'_' => d.push(b' '),
                        b'=' if i + 2 < bytes.len() => {
                            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                            match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                                Some(v) => {
                                    d.push(v);
                                    i += 2;
                                }
                                None => d.push(b'='),
                            }
                        }
                        b => d.push(b),
                    }
                    i += 1;
                }
                String::from_utf8(d).ok()
            }
            _ => None,
        };
        match decoded {
            Some(d) => out.push_str(&d),
            None => out.push_str(&rest[start..start + 2 + end + 2]),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

pub struct Query<'a> {
    pub from: Option<&'a str>,
    pub subject: Option<&'a str>,
    pub since_unix: Option<i64>,
}

pub fn search<'m>(msgs: &'m [Message], q: &Query) -> Vec<&'m Message> {
    msgs.iter()
        .filter(|m| {
            q.from.map_or(true, |f| {
                m.header("from")
                    .to_ascii_lowercase()
                    .contains(&f.to_ascii_lowercase())
            }) && q.subject.map_or(true, |s| {
                m.subject()
                    .to_ascii_lowercase()
                    .contains(&s.to_ascii_lowercase())
            }) && q
                .since_unix
                .map_or(true, |t| m.date_unix.is_some_and(|d| d >= t))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MBOX: &str = "From alice@example.com Thu Jul  9 10:00:00 2026\n\
From: Alice <alice@example.com>\n\
To: bob@example.com\n\
Subject: =?UTF-8?B?aGVsbG8gd29ybGQ=?= and more\n\
Date: Thu, 9 Jul 2026 10:00:00 +0000\n\
\n\
first body\n\
>From here this is body text, not a new message\n\
From bob@example.com Fri Jul 10 09:00:00 2026\n\
From: Bob <bob@example.com>\n\
Subject: multi\n\
\x20line folded subject\n\
Date: Fri, 10 Jul 2026 14:30:00 +0530\n\
\n\
second body\n\
From carol@example.com Sat Jul 11 09:00:00 2026\n\
From: Carol <carol@example.com>\n\
Subject: =?utf-8?Q?caf=C3=A9_time?=\n\
Date: Sat, 11 Jul 2026 00:00:00 +0000\n\
\n\
third body\n";

    #[test]
    fn splits_unfolds_and_decodes() {
        let msgs = parse_mbox(MBOX).unwrap();
        assert_eq!(msgs.len(), 3, ">From body line must not split");
        assert_eq!(msgs[0].subject(), "hello world and more");
        assert_eq!(msgs[1].header("subject"), "multi line folded subject");
        assert_eq!(msgs[2].subject(), "café time");
        assert!(msgs[0].raw.contains(">From here"));
    }

    #[test]
    fn dates_respect_zones_and_search_filters_compose() {
        let msgs = parse_mbox(MBOX).unwrap();
        // 14:30 +0530 == 09:00 UTC.
        let d1 = msgs[1].date_unix.unwrap();
        let d0 = msgs[0].date_unix.unwrap();
        assert_eq!(d1 - d0, 23 * 3600, "zone offset must be applied");

        let hits = search(
            &msgs,
            &Query {
                from: Some("bob"),
                subject: None,
                since_unix: None,
            },
        );
        assert_eq!(hits.len(), 1);
        let hits = search(
            &msgs,
            &Query {
                from: None,
                subject: Some("café"),
                since_unix: None,
            },
        );
        assert_eq!(hits.len(), 1);
        let hits = search(
            &msgs,
            &Query {
                from: None,
                subject: None,
                since_unix: Some(d1),
            },
        );
        assert_eq!(hits.len(), 2, "since is inclusive");
    }
}
