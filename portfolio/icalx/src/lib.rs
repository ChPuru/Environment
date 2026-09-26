//! icalx: the useful core of iCalendar without a server. Parses VEVENTs
//! (line unfolding, TEXT escaping, DATE and DATE-TIME forms), expands a
//! documented RRULE subset (FREQ daily/weekly/monthly/yearly, INTERVAL,
//! COUNT, UNTIL, BYDAY for weekly, BYMONTHDAY for monthly), and writes
//! spec-folded output. Floating times are treated as UTC — stated, not
//! hidden. No chrono: civil-date math is Hinnant's algorithm both ways.

use anyhow::{bail, Context, Result};
use std::collections::BTreeSet;

// ---------- civil date math ----------
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp as i64 + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn weekday(days: i64) -> u32 {
    // 1970-01-01 was a Thursday; 0=MO..6=SU
    ((days + 3).rem_euclid(7)) as u32
}

// ---------- model ----------
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub uid: String,
    pub summary: String,
    pub start_unix: i64,
    pub all_day: bool,
    pub rrule: Option<Rrule>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rrule {
    pub freq: Freq,
    pub interval: i64,
    pub count: Option<u32>,
    pub until_unix: Option<i64>,
    pub byday: BTreeSet<u32>,    // weekly: 0=MO..6=SU
    pub bymonthday: Option<u32>, // monthly
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Freq {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

// ---------- text escaping ----------
pub fn escape_text(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace('\n', "\\n")
}

pub fn unescape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') | Some('N') => out.push('\n'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

// ---------- parse ----------
fn unfold(input: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in input.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if (line.starts_with(' ') || line.starts_with('\t')) && !out.is_empty() {
            out.last_mut().expect("checked").push_str(&line[1..]);
        } else {
            out.push(line.to_string());
        }
    }
    out.retain(|l| !l.is_empty());
    out
}

fn parse_dt(v: &str) -> Result<(i64, bool)> {
    let all_day = v.len() == 8;
    let (date, time) = if all_day {
        (v, "000000")
    } else {
        v.split_once('T').context("bad DATE-TIME")?
    };
    if date.len() != 8 {
        bail!("bad date {date:?}");
    }
    let y: i64 = date[0..4].parse()?;
    let m: u32 = date[4..6].parse()?;
    let d: u32 = date[6..8].parse()?;
    let time = time.trim_end_matches('Z');
    if time.len() != 6 {
        bail!("bad time {time:?}");
    }
    let (h, mi, s): (i64, i64, i64) = (
        time[0..2].parse()?,
        time[2..4].parse()?,
        time[4..6].parse()?,
    );
    Ok((
        days_from_civil(y, m, d) * 86_400 + h * 3600 + mi * 60 + s,
        all_day,
    ))
}

fn day_code(s: &str) -> Result<u32> {
    Ok(match s {
        "MO" => 0,
        "TU" => 1,
        "WE" => 2,
        "TH" => 3,
        "FR" => 4,
        "SA" => 5,
        "SU" => 6,
        other => bail!("unsupported BYDAY {other} (positional prefixes not in subset)"),
    })
}

fn parse_rrule(v: &str) -> Result<Rrule> {
    let mut r = Rrule {
        freq: Freq::Daily,
        interval: 1,
        count: None,
        until_unix: None,
        byday: BTreeSet::new(),
        bymonthday: None,
    };
    let mut saw_freq = false;
    for part in v.split(';') {
        let (k, val) = part.split_once('=').context("bad RRULE part")?;
        match k {
            "FREQ" => {
                saw_freq = true;
                r.freq = match val {
                    "DAILY" => Freq::Daily,
                    "WEEKLY" => Freq::Weekly,
                    "MONTHLY" => Freq::Monthly,
                    "YEARLY" => Freq::Yearly,
                    other => bail!("unsupported FREQ {other}"),
                };
            }
            "INTERVAL" => r.interval = val.parse::<i64>()?.max(1),
            "COUNT" => r.count = Some(val.parse()?),
            "UNTIL" => r.until_unix = Some(parse_dt(val)?.0),
            "BYDAY" => {
                for d in val.split(',') {
                    r.byday.insert(day_code(d)?);
                }
            }
            "BYMONTHDAY" => r.bymonthday = Some(val.parse()?),
            _ => bail!("unsupported RRULE key {k} (subset is documented in README)"),
        }
    }
    if !saw_freq {
        bail!("RRULE without FREQ");
    }
    Ok(r)
}

pub fn parse(input: &str) -> Result<Vec<Event>> {
    let mut events = Vec::new();
    let mut cur: Option<Event> = None;
    for line in unfold(input) {
        let (name_full, value) = line.split_once(':').context("property needs a colon")?;
        let name = name_full
            .split(';')
            .next()
            .expect("nonempty")
            .to_ascii_uppercase();
        match name.as_str() {
            "BEGIN" if value.eq_ignore_ascii_case("VEVENT") => {
                cur = Some(Event {
                    uid: String::new(),
                    summary: String::new(),
                    start_unix: 0,
                    all_day: false,
                    rrule: None,
                });
            }
            "END" if value.eq_ignore_ascii_case("VEVENT") => {
                let e = cur.take().context("END without BEGIN")?;
                events.push(e);
            }
            "UID" => {
                if let Some(e) = cur.as_mut() {
                    e.uid = value.to_string();
                }
            }
            "SUMMARY" => {
                if let Some(e) = cur.as_mut() {
                    e.summary = unescape_text(value);
                }
            }
            "DTSTART" => {
                if let Some(e) = cur.as_mut() {
                    let (t, ad) = parse_dt(value)?;
                    e.start_unix = t;
                    e.all_day = ad;
                }
            }
            "RRULE" => {
                if let Some(e) = cur.as_mut() {
                    e.rrule = Some(parse_rrule(value)?);
                }
            }
            _ => {} // tolerated, preserved-nowhere: this is a toolkit, not a proxy
        }
    }
    Ok(events)
}

// ---------- expand ----------
/// Occurrences of `e` with start in [from, to] (unix, inclusive).
pub fn expand(e: &Event, from: i64, to: i64) -> Vec<i64> {
    let Some(r) = &e.rrule else {
        return if e.start_unix >= from && e.start_unix <= to {
            vec![e.start_unix]
        } else {
            vec![]
        };
    };
    let start_day = e.start_unix.div_euclid(86_400);
    let tod = e.start_unix.rem_euclid(86_400);
    let (sy, sm, sd) = civil_from_days(start_day);
    let byday: BTreeSet<u32> = if r.byday.is_empty() && r.freq == Freq::Weekly {
        [weekday(start_day)].into_iter().collect()
    } else {
        r.byday.clone()
    };
    let monthday = r.bymonthday.unwrap_or(sd);

    let mut out = Vec::new();
    let mut emitted: u32 = 0;
    let cap_day = to.div_euclid(86_400);
    let mut day = start_day;
    let hard_stop = start_day + 366 * 200; // two centuries of scanning, max
    while day <= cap_day && day < hard_stop {
        let matches = match r.freq {
            Freq::Daily => (day - start_day) % r.interval == 0,
            Freq::Weekly => {
                byday.contains(&weekday(day)) && ((day - start_day).div_euclid(7)) % r.interval == 0
            }
            Freq::Monthly => {
                let (y, m, d) = civil_from_days(day);
                let months = (y - sy) * 12 + (m as i64 - sm as i64);
                d == monthday && months % r.interval == 0 && months >= 0
            }
            Freq::Yearly => {
                let (y, m, d) = civil_from_days(day);
                m == sm && d == sd && (y - sy) % r.interval == 0
            }
        };
        if matches {
            let t = day * 86_400 + tod;
            if let Some(u) = r.until_unix {
                if t > u {
                    break;
                }
            }
            emitted += 1;
            if t >= from {
                out.push(t);
            }
            if let Some(c) = r.count {
                if emitted >= c {
                    break;
                }
            }
        }
        day += 1;
    }
    out
}

// ---------- write ----------
fn fold(line: &str) -> String {
    let bytes = line.as_bytes();
    if bytes.len() <= 75 {
        return format!("{line}\r\n");
    }
    let mut out = String::new();
    let mut start = 0;
    let mut first = true;
    while start < bytes.len() {
        let budget = if first { 75 } else { 74 };
        let mut end = (start + budget).min(bytes.len());
        while end < bytes.len() && !line.is_char_boundary(end) {
            end -= 1;
        }
        if !first {
            out.push(' ');
        }
        out.push_str(&line[start..end]);
        out.push_str("\r\n");
        start = end;
        first = false;
    }
    out
}

pub fn write(events: &[Event]) -> String {
    let mut s = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//icalx//EN\r\n");
    for e in events {
        s.push_str("BEGIN:VEVENT\r\n");
        s.push_str(&fold(&format!("UID:{}", e.uid)));
        let day = e.start_unix.div_euclid(86_400);
        let (y, m, d) = civil_from_days(day);
        let tod = e.start_unix.rem_euclid(86_400);
        let dt = if e.all_day {
            format!("DTSTART:{y:04}{m:02}{d:02}")
        } else {
            format!(
                "DTSTART:{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
                tod / 3600,
                (tod % 3600) / 60,
                tod % 60
            )
        };
        s.push_str(&fold(&dt));
        s.push_str(&fold(&format!("SUMMARY:{}", escape_text(&e.summary))));
        s.push_str("END:VEVENT\r\n");
    }
    s.push_str("END:VCALENDAR\r\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(start: i64, rrule: &str) -> Event {
        Event {
            uid: "u1".into(),
            summary: "s".into(),
            start_unix: start,
            all_day: false,
            rrule: Some(parse_rrule(rrule).unwrap()),
        }
    }
    const DAY: i64 = 86_400;

    #[test]
    fn civil_math_roundtrips_across_leap_years() {
        for &(y, m, d) in &[
            (1970, 1, 1),
            (2000, 2, 29),
            (2024, 2, 29),
            (2026, 12, 31),
            (1999, 3, 1),
        ] {
            let days = days_from_civil(y, m, d);
            assert_eq!(civil_from_days(days), (y, m, d));
        }
        assert_eq!(
            weekday(days_from_civil(2026, 7, 9)),
            3,
            "2026-07-09 is a Thursday"
        );
    }

    #[test]
    fn escaping_and_folding_survive_a_parse_write_parse_loop() {
        let e = Event {
            uid: "long-uid".into(),
            summary: "hello, world; with\nnewline and a very long tail ".repeat(4),
            start_unix: days_from_civil(2026, 7, 9) * DAY + 3600,
            all_day: false,
            rrule: None,
        };
        let text = write(std::slice::from_ref(&e));
        assert!(text.lines().all(|l| l.len() <= 75), "folded at 75 octets");
        let back = parse(&text).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].summary, e.summary);
        assert_eq!(back[0].start_unix, e.start_unix);
    }

    #[test]
    fn daily_with_interval_and_count() {
        let start = days_from_civil(2026, 1, 1) * DAY;
        let e = ev(start, "FREQ=DAILY;INTERVAL=2;COUNT=5");
        let got = expand(&e, start, start + 30 * DAY);
        assert_eq!(
            got,
            vec![
                start,
                start + 2 * DAY,
                start + 4 * DAY,
                start + 6 * DAY,
                start + 8 * DAY
            ]
        );
        // COUNT counts from DTSTART even when the window starts later.
        let later = expand(&e, start + 5 * DAY, start + 30 * DAY);
        assert_eq!(later, vec![start + 6 * DAY, start + 8 * DAY]);
    }

    #[test]
    fn weekly_byday_and_until() {
        // 2026-07-06 is a Monday.
        let start = days_from_civil(2026, 7, 6) * DAY + 9 * 3600;
        let e = ev(start, "FREQ=WEEKLY;BYDAY=MO,WE;UNTIL=20260717T000000Z");
        let got = expand(&e, start, start + 60 * DAY);
        let expect = vec![
            start,           // Mon 06
            start + 2 * DAY, // Wed 08
            start + 7 * DAY, // Mon 13
            start + 9 * DAY, // Wed 15
        ];
        assert_eq!(got, expect, "Fri 17 UNTIL bound excludes the next Monday");
    }

    #[test]
    fn monthly_day_31_skips_short_months_and_yearly_hits_the_date() {
        let start = days_from_civil(2026, 1, 31) * DAY;
        let e = ev(start, "FREQ=MONTHLY;BYMONTHDAY=31;COUNT=3");
        let got = expand(&e, start, start + 400 * DAY);
        assert_eq!(
            got,
            vec![
                days_from_civil(2026, 1, 31) * DAY,
                days_from_civil(2026, 3, 31) * DAY, // February has no 31st
                days_from_civil(2026, 5, 31) * DAY, // April neither
            ]
        );
        let y = ev(days_from_civil(2024, 2, 29) * DAY, "FREQ=YEARLY;COUNT=2");
        let got = expand(&y, 0, days_from_civil(2040, 1, 1) * DAY);
        assert_eq!(
            got,
            vec![
                days_from_civil(2024, 2, 29) * DAY,
                days_from_civil(2028, 2, 29) * DAY
            ],
            "yearly Feb-29 lands only on leap years"
        );
    }

    #[test]
    fn parses_a_folded_real_world_shape() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:abc\r\nDTSTART;TZID=UTC:20260709T120000Z\r\nSUMMARY:standup\\, daily\r\n sync\r\nRRULE:FREQ=DAILY;COUNT=2\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let evs = parse(ics).unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].summary, "standup, dailysync");
        assert_eq!(evs[0].rrule.as_ref().unwrap().count, Some(2));
        assert!(parse("BEGIN:VEVENT\r\nRRULE:FREQ=SECONDLY\r\nEND:VEVENT\r\n").is_err());
    }
}
