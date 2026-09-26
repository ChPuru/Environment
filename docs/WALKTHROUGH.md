# Walkthrough — a guided tour of all eleven tools

Assumes you ran `./setup.sh` and `export PATH="$PWD/bin:$PATH"`.
Each section is self-contained; skip around.

---

## 1. warden — backup that proves itself

    export WARDEN_PASSPHRASE=demo
    mkdir -p /tmp/wt/src && printf 'hello\n' > /tmp/wt/src/a.txt
    head -c 200000 /dev/urandom > /tmp/wt/src/big.bin

    warden --repo /tmp/wt/repo init
    warden --repo /tmp/wt/repo backup /tmp/wt/src

The backup already did read-back-after-write: it re-fetched a sample of
what it wrote through full verification before printing the id. Now prove
more:

    warden --repo /tmp/wt/repo verify --deep    # re-checks every byte
    warden --repo /tmp/wt/repo drill --seed 1   # restores random files for real
    warden --repo /tmp/wt/repo health           # exits nonzero if stale/unproven

Restore and compare:

    warden --repo /tmp/wt/repo list
    warden --repo /tmp/wt/repo restore <ID> /tmp/wt/out
    diff -r /tmp/wt/src /tmp/wt/out             # identical

Watch integrity catch corruption:

    f=$(find /tmp/wt/repo/objects -type f | head -1)
    dd if=/dev/zero of="$f" bs=1 count=1 seek=50 conv=notrunc 2>/dev/null
    warden --repo /tmp/wt/repo verify --deep    # names the damaged object, exits 1

Keyless audit (no passphrase — imagine running this on the storage server):

    env -u WARDEN_PASSPHRASE warden --repo /tmp/wt/repo scrub

S3 usage is in warden/README.md (set WARDEN_S3_* env vars, use
`--repo s3://bucket/prefix`); it is exercised in tests against a mock.

---

## 2. ferry — resumable file transfer (two terminals)

Terminal 1 (receiver):

    mkdir -p /tmp/inbox
    ferry recv --port 3777 --dir /tmp/inbox

Terminal 2 (sender) — make a big file, start sending, Ctrl-C partway:

    head -c 50000000 /dev/urandom > /tmp/big.bin
    ferry send /tmp/big.bin 127.0.0.1:3777
    # Ctrl-C mid-transfer, then run the SAME line again:
    ferry send /tmp/big.bin 127.0.0.1:3777      # resumes; only the rest is sent

Completion is gated on a whole-file BLAKE3 match; a corrupted partial is
deleted rather than delivered.

---

## 3. mirrorer — one-way directory mirror

    mkdir -p /tmp/m/src && echo one > /tmp/m/src/a && echo two > /tmp/m/src/b
    mirrorer /tmp/m/src /tmp/m/dst --dry-run     # shows the plan
    mirrorer /tmp/m/src /tmp/m/dst               # copies
    echo CHANGED > /tmp/m/src/a
    mirrorer /tmp/m/src /tmp/m/dst --dry-run     # detects a's change by hash
    rm /tmp/m/src/b
    mirrorer /tmp/m/src /tmp/m/dst --delete      # removes b from dst (only with --delete)

Same length is not treated as same content — BLAKE3 decides.

---

## 4. shapeshift — CSV/TSV <-> JSON

    printf 'name,age,active\nAda,36,true\nBo,,false\n' | shapeshift csv2json --infer
    # numbers/bools/nulls become real JSON types

    echo '[{"a":1,"b":2},{"a":3}]' | shapeshift json2csv
    # ragged objects -> deterministic union header with empty cells

    printf 'x\ty\n1\t2\n' | shapeshift csv2json --delimiter "$(printf '\t')"

Quoted commas and embedded newlines survive a full round trip. Nested
JSON values are refused, not silently flattened.

---

## 5. icalx — calendar (.ics) parse and recurrence expansion

    cat > /tmp/cal.ics <<'ICS'
    BEGIN:VCALENDAR
    VERSION:2.0
    BEGIN:VEVENT
    UID:1
    DTSTART:20260706T090000Z
    SUMMARY:Standup
    RRULE:FREQ=WEEKLY;BYDAY=MO,WE;COUNT=4
    END:VEVENT
    END:VCALENDAR
    ICS
    icalx /tmp/cal.ics --from 20260101 --to 20261231
    # prints each expanded occurrence

Supported RRULE subset is documented in icalx/README.md; unsupported
constructs (e.g. positional BYDAY) are refused loudly, not half-handled.

---

## 6. mailsift — mbox archive stats, search, export

    cat > /tmp/a.mbox <<'MBOX'
    From x@y Thu Jul  9 10:00:00 2026
    From: Alice <alice@example.com>
    Subject: Invoice 42
    Date: Thu, 9 Jul 2026 10:00:00 +0000

    body text
    From x@y Fri Jul 10 09:00:00 2026
    From: Bob <bob@example.com>
    Subject: =?UTF-8?B?Y2Fmw6k=?= chat
    Date: Fri, 10 Jul 2026 14:30:00 +0530

    more
    MBOX
    mailsift /tmp/a.mbox stats
    mailsift /tmp/a.mbox search --from alice
    mailsift /tmp/a.mbox search --subject café     # encoded-word decoded
    mailsift /tmp/a.mbox export 0 > /tmp/msg0.eml

Reads archives; it does not connect to mail servers.

---

## 7. shutterbug — organize photos by EXIF date

    mkdir -p /tmp/cam
    # drop some .jpg files from a real camera into /tmp/cam, then:
    shutterbug /tmp/cam /tmp/photos --dry-run    # shows YYYY/MM placement
    shutterbug /tmp/cam /tmp/photos              # files them; dups skipped by hash

Photos with no EXIF fall back to file mtime and the plan says so. EXIF is
parsed from the JPEG bytes directly (both byte orders), no libraries.

---

## 8. textract — batch OCR to a searchable index (needs tesseract)

    sudo apt-get install -y tesseract-ocr    # or brew install tesseract
    # put some images with text in /tmp/scans, then:
    textract run /tmp/scans --out /tmp/index.json
    textract search /tmp/index.json "invoice"    # path:line: snippet

The OCR engine is a seam; per-file failures are recorded in the index,
never silently dropped.

---

## 9. relay — TCP chat with rooms (three terminals)

Terminal 1:  relay serve 4000
Terminal 2:  relay join 127.0.0.1:4000   then type:  /nick alice
Terminal 3:  relay join 127.0.0.1:4000   then type:  /nick bob

Now `/join dev` in one, chat; messages reach only the same room. Nick
changes, joins, and departures are announced. No accounts, no encryption,
no persistence — a clean small chat core.

---

## 10. clipsmith — video probe/trim/thumbnail (needs ffmpeg)

    sudo apt-get install -y ffmpeg
    clipsmith probe movie.mp4               # duration + streams
    clipsmith trim movie.mp4 60 90 clip.mp4 # lossless [60s,90s) stream copy
    clipsmith thumb movie.mp4 12.5 frame.png

This is a wrapper around ffmpeg for these three operations. It is not
videocall software — none was built.

---

## 11. gridcalc — a spreadsheet engine over CSV

    printf '1,2,=A1+B1\n=SUM(A1:B1),,=C1*2\n' > /tmp/s.csv
    gridcalc recalc /tmp/s.csv              # prints values: 1,2,3 / 3,,6
    gridcalc eval /tmp/s.csv C1             # 3

Formulas: + - * / parens, unary minus, cell refs (incl. AA-columns),
SUM/AVG/MIN/MAX/COUNT over A1:C4 ranges. Reference cycles are reported as
named errors, never a crash. Try a cycle:

    printf '=B1,=A1\n' > /tmp/cyc.csv && gridcalc eval /tmp/cyc.csv A1   # errors, names the cell
