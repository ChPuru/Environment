# textract

Batch OCR pipeline: index every image under a directory into JSON, then
search it with line numbers and snippets. The OCR engine is a trait — the
real one shells out to `tesseract` when installed (a gated smoke test
proves the seam); the pipeline, per-file error capture (failures are
recorded, never dropped), recursion, and search are proven with a fake
engine either way. Honest split: this crate owns the pipeline, tesseract
owns the pixels.

    textract run scans/ --out index.json
    textract search index.json "total due"
