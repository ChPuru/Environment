# shutterbug

Photo organizer: files JPEGs into DST/YYYY/MM by EXIF DateTimeOriginal,
parsed from the bytes directly (APP1/TIFF walk, both endiannesses, no C
bindings). Byte-identical duplicates are skipped via BLAKE3; same-name
different-content collisions get -1/-2 suffixes. Photos without EXIF fall
back to mtime and the plan SAYS so. `--dry-run` prints, moves nothing.
The synthetic-JPEG fixture generator is public: the tests do not depend
on binary blobs.

    shutterbug ~/camera-dump ~/photos --dry-run
