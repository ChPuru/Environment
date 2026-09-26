# shapeshift

CSV/TSV <-> JSON on stdin/stdout. Quoted commas and embedded newlines
survive the roundtrip; ragged JSON objects become a deterministic
union-of-keys header with empty cells; `--infer` produces real JSON
numbers, booleans, and nulls. Nested values are refused loudly rather
than flattened into lies.

    shapeshift csv2json --infer < data.csv > data.json
    shapeshift json2csv < data.json > data.csv
    shapeshift csv2json --delimiter $'\t' < data.tsv
