//! The Backend contract, executable. Every backend — local disk today, S3
//! tomorrow, whatever comes after — must pass this exact suite, run against
//! a FRESH scratch instance. Returns findings instead of panicking so the
//! caller owns the assertion (and so a partial run reports everything).

use super::{Backend, Kind};
use crate::error::Error;
use crate::object::{Hash256, StoredName};

fn name(b: u8) -> StoredName {
    StoredName(Hash256([b; 32]))
}

/// Run the full contract against an EMPTY backend. Empty vec == conformant.
pub fn check<B: Backend>(b: &B) -> Vec<String> {
    let mut out = Vec::new();
    let mut bad = |msg: String| out.push(msg);

    // 1) Missing-object taxonomy.
    match b.get(Kind::Object, &name(1)) {
        Err(Error::NotFound(m)) => {
            if !m.contains(&name(1).0.to_hex()) {
                bad(format!("NotFound message must name the object, got {m:?}"));
            }
        }
        other => bad(format!("get(missing) must be NotFound, got {other:?}")),
    }
    match b.exists(Kind::Object, &name(1)) {
        Ok(false) => {}
        other => bad(format!("exists(missing) must be Ok(false), got {other:?}")),
    }
    if !matches!(
        b.get_range(Kind::Object, &name(1), 0, 4),
        Err(Error::NotFound(_))
    ) {
        bad("get_range(missing, len>0) must be NotFound".into());
    }

    // 2) Roundtrips per kind; kinds are isolated namespaces.
    let payload = |k: Kind, i: u8| -> Vec<u8> { format!("{}-{}", k.dir(), i).into_bytes() };
    for kind in [Kind::Object, Kind::Index, Kind::Snapshot, Kind::Health] {
        for i in [1u8, 2] {
            if let Err(e) = b.put(kind, &name(i), &payload(kind, i)) {
                bad(format!("put {}/{i}: {e}", kind.dir()));
            }
        }
        match b.get(kind, &name(1)) {
            Ok(v) if v == payload(kind, 1) => {}
            other => bad(format!("get {}/1 roundtrip failed: {other:?}", kind.dir())),
        }
        if !matches!(b.exists(kind, &name(1)), Ok(true)) {
            bad(format!("exists {}/1 must be true", kind.dir()));
        }
        // Idempotent by value: re-putting identical content succeeds.
        if let Err(e) = b.put(kind, &name(1), &payload(kind, 1)) {
            bad(format!("re-put of identical content must be Ok, got {e}"));
        }
        let mut listed: Vec<String> = match b.list(kind) {
            Ok(v) => v.into_iter().map(|n| n.0.to_hex()).collect(),
            Err(e) => {
                bad(format!("list {}: {e}", kind.dir()));
                continue;
            }
        };
        listed.sort();
        let mut expect = vec![name(1).0.to_hex(), name(2).0.to_hex()];
        expect.sort();
        if listed != expect {
            bad(format!(
                "list {} must return exactly the puts, got {listed:?}",
                kind.dir()
            ));
        }
    }

    // 2b) Sizes are exact; missing is NotFound.
    match b.size(Kind::Object, &name(1)) {
        Ok(n) if n == payload(Kind::Object, 1).len() as u64 => {}
        other => bad(format!("size must match stored bytes, got {other:?}")),
    }
    if !matches!(b.size(Kind::Object, &name(77)), Err(Error::NotFound(_))) {
        bad("size(missing) must be NotFound".into());
    }

    // 3) Ranged reads: boundaries exact, degenerate canonical, overrange LOUD.
    let data: Vec<u8> = (0u8..100).collect();
    let rn = name(9);
    let _ = b.put(Kind::Object, &rn, &data);
    match b.get_range(Kind::Object, &rn, 0, 100) {
        Ok(v) if v == data => {}
        other => bad(format!("full-range read failed: {other:?}")),
    }
    match b.get_range(Kind::Object, &rn, 10, 20) {
        Ok(v) if v == data[10..30] => {}
        other => bad(format!("mid-range read failed: {other:?}")),
    }
    match b.get_range(Kind::Object, &rn, 96, 4) {
        Ok(v) if v == data[96..100] => {}
        other => bad(format!("tail-range read failed: {other:?}")),
    }
    for (off, l, label) in [
        (90u64, 20u64, "spill"),
        (100, 1, "at-end"),
        (200, 4, "past-end"),
    ] {
        if !matches!(
            b.get_range(Kind::Object, &rn, off, l),
            Err(Error::Corrupt { .. })
        ) {
            bad(format!("overrange ({label}) must be LOUD Corrupt"));
        }
    }
    for n in [&rn, &name(33)] {
        match b.get_range(Kind::Object, n, 0, 0) {
            Ok(v) if v.is_empty() => {}
            other => bad(format!(
                "len=0 range must be Ok(empty) even if absent: {other:?}"
            )),
        }
    }

    // 4) Meta: overwrite contract is ATOMIC AlreadyExists.
    if let Err(e) = b.put_meta("conformance/x", b"one", false) {
        bad(format!("first put_meta must succeed: {e}"));
    }
    match b.put_meta("conformance/x", b"two", false) {
        Err(Error::AlreadyExists(p)) => {
            if !p.contains("conformance/x") {
                bad(format!("AlreadyExists must name the path, got {p:?}"));
            }
        }
        other => bad(format!(
            "second put_meta(overwrite=false) must be AlreadyExists: {other:?}"
        )),
    }
    match b.get_meta("conformance/x") {
        Ok(v) if v == b"one" => {}
        other => bad(format!("failed create-new must not clobber: {other:?}")),
    }
    if b.put_meta("conformance/x", b"three", true).is_err() {
        bad("put_meta(overwrite=true) must replace".into());
    }
    match b.get_meta("conformance/x") {
        Ok(v) if v == b"three" => {}
        other => bad(format!("overwrite must be visible: {other:?}")),
    }
    if b.get_meta("conformance/missing").is_ok() {
        bad("get_meta(missing) must err".into());
    }

    // 5) Deletes: effective and idempotent.
    if b.delete(Kind::Object, &rn).is_err() {
        bad("delete must succeed".into());
    }
    if !matches!(b.exists(Kind::Object, &rn), Ok(false)) {
        bad("deleted object must not exist".into());
    }
    if !matches!(b.get(Kind::Object, &rn), Err(Error::NotFound(_))) {
        bad("deleted object must be NotFound".into());
    }
    if b.delete(Kind::Object, &rn).is_err() {
        bad("delete of absent must be Ok (idempotent)".into());
    }
    if let Ok(l) = b.list(Kind::Object) {
        if l.contains(&rn) {
            bad("list must not contain deleted names".into());
        }
    }
    if b.delete_meta("conformance/x").is_err() || b.delete_meta("conformance/x").is_err() {
        bad("delete_meta must be Ok and idempotent".into());
    }
    if b.get_meta("conformance/x").is_ok() {
        bad("deleted meta must be gone".into());
    }

    out
}
