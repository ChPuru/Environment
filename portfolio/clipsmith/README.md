# clipsmith

Video toolkit over ffmpeg: `probe` (duration, streams), `trim` (lossless
stream-copy cut), `thumb` (frame at timestamp). This crate owns argument
construction and ffprobe-JSON parsing — both pure and unit-tested — and
the process seam is proven end-to-end against SHIM executables that
validate argv, plus a gated smoke against real ffmpeg (which generates
its own test clip via lavfi) when installed. Honest split, stated.

    clipsmith probe talk.mp4
    clipsmith trim talk.mp4 60 90 clip.mp4
