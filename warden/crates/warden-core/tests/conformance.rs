//! LocalBackend against the executable Backend contract.

use tempfile::tempdir;
use warden_core::backend::{conformance, LocalBackend};

#[test]
fn local_backend_is_conformant() {
    let td = tempdir().expect("td");
    let be = LocalBackend::create(td.path()).expect("create");
    let problems = conformance::check(&be);
    assert!(
        problems.is_empty(),
        "contract violations:\n{}",
        problems.join("\n")
    );
}
