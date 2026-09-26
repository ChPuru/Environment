//! I3, the cardinal test, single-file shape: for EVERY stored file, a bitflip
//! or truncation must make restore fail loudly with no destination, or still
//! produce byte-identical output. Wrong bytes silently is the one sin.

mod common;
use common::*;
use tempfile::tempdir;

#[test]
fn any_single_corruption_is_never_silent_single_file() {
    let td = tempdir().expect("td");
    let root = td.path().join("repo");
    let src = td.path().join("src.bin");
    let data = deterministic_bytes(16 * 1024, 99);
    write_file(&src, &data);

    let snap = {
        let mut repo = init_tiny(&root);
        repo.backup_file(&src).expect("backup")
    };

    tamper_matrix(&root, &snap, td.path(), &|dest| {
        assert_eq!(
            std::fs::read(dest).expect("read restored"),
            data,
            "restore 'succeeded' with WRONG BYTES — silent corruption"
        );
    });

    let dest = td.path().join("out-final.bin");
    open_tiny(&root)
        .restore_file(&snap, &dest)
        .expect("pristine restore");
    assert_eq!(std::fs::read(&dest).expect("read"), data);
}
