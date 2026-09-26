# ferry

Resumable, integrity-verified file transfer over TCP. One JSON header line,
then raw bytes from the receiver's current offset; completion is gated on a
whole-file BLAKE3 match (mismatch deletes the partial — no silent garbage).
A changed source never resumes onto old bytes (the resume meta binds the
partial to (hash, len)). Unsafe names are refused.

    ferry recv --port 3777 --dir inbox/
    ferry send big.iso host:3777        # rerun after a cut: it resumes

Tests cover: clean roundtrip, kill-at-100k resume (only the remainder is
resent), changed-source restart, and path-traversal refusal.
