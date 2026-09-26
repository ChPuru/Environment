# TRY-IT — a two-minute demo of each tool (after ./setup.sh)

    export PATH="$PWD/bin:$PATH"

## warden (backup) — the flagship
    export WARDEN_PASSPHRASE=demo
    mkdir -p /tmp/w-data && echo "precious" > /tmp/w-data/file.txt
    warden --repo /tmp/w-repo init
    warden --repo /tmp/w-repo backup /tmp/w-data       # prints snapshot id
    warden --repo /tmp/w-repo verify --deep            # proves + records
    warden --repo /tmp/w-repo drill                    # restore-drill, recorded
    warden --repo /tmp/w-repo health                   # receipts + staleness
    warden --repo /tmp/w-repo restore <SNAPSHOT-ID> /tmp/w-out
    # S3-compatible stores: see warden/README.md (env creds, s3://bucket/prefix)

## ferry (transfer)  — two terminals
    ferry recv --port 3777 --dir /tmp/inbox        # terminal 1
    ferry send some-big-file 127.0.0.1:3777        # terminal 2; Ctrl-C and re-run: it RESUMES

## mirrorer (sync)
    mirrorer ~/some-dir /tmp/mirror --dry-run
    mirrorer ~/some-dir /tmp/mirror --delete

## shapeshift (convert)
    printf 'a,b\n1,true\n' | shapeshift csv2json --infer
    echo '[{"x":1},{"y":2}]' | shapeshift json2csv

## icalx (calendar)
    icalx some-calendar.ics --from 20260101 --to 20261231

## mailsift (email archives)
    mailsift archive.mbox stats
    mailsift archive.mbox search --from alice --subject invoice

## shutterbug (photos)
    shutterbug ~/camera-dump /tmp/photos --dry-run

## textract (OCR; needs tesseract installed)
    textract run scans/ --out index.json
    textract search index.json "total due"

## relay (chat) — three terminals
    relay serve 4000
    relay join 127.0.0.1:4000        # then: /nick alice, /join dev, type away

## clipsmith (video; needs ffmpeg installed)
    clipsmith probe movie.mp4
    clipsmith trim movie.mp4 60 90 clip.mp4

## gridcalc (spreadsheet)
    printf '1,2,=A1+B1\n=SUM(A1:B1),,=C1*2\n' > /tmp/sheet.csv
    gridcalc recalc /tmp/sheet.csv
    gridcalc eval /tmp/sheet.csv C1
