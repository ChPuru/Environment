# mirrorer

One-way directory mirror. Change detection is honest: equal length is not
equal content — BLAKE3 decides. Copies are atomic (tmp + rename in the
destination). Files that exist only in DST are reported always and deleted
only with `--delete`. `--dry-run` prints the plan and touches nothing.

    mirrorer ~/docs /backup/docs --dry-run
    mirrorer ~/docs /backup/docs --delete
