//! shapeshift: CSV/TSV <-> JSON. Quoting, embedded commas/newlines, and
//! ragged JSON objects (union-of-keys headers) handled; `infer` turns
//! "42"/"3.5"/"true"/"" into real JSON types instead of strings.

use anyhow::{bail, Context, Result};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

pub fn infer_scalar(s: &str) -> Value {
    if s.is_empty() {
        return Value::Null;
    }
    if s == "true" {
        return Value::Bool(true);
    }
    if s == "false" {
        return Value::Bool(false);
    }
    if let Ok(i) = s.parse::<i64>() {
        return Value::from(i);
    }
    if let Ok(f) = s.parse::<f64>() {
        if f.is_finite() {
            return Value::from(f);
        }
    }
    Value::String(s.to_string())
}

pub fn csv_to_json(input: &str, delimiter: u8, infer: bool) -> Result<Value> {
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .from_reader(input.as_bytes());
    let headers: Vec<String> = rdr
        .headers()
        .context("csv needs a header row")?
        .iter()
        .map(str::to_string)
        .collect();
    let mut rows = Vec::new();
    for rec in rdr.records() {
        let rec = rec?;
        let mut obj = Map::new();
        for (i, h) in headers.iter().enumerate() {
            let cell = rec.get(i).unwrap_or("");
            let v = if infer {
                infer_scalar(cell)
            } else {
                Value::String(cell.to_string())
            };
            obj.insert(h.clone(), v);
        }
        rows.push(Value::Object(obj));
    }
    Ok(Value::Array(rows))
}

fn scalar_to_cell(v: &Value) -> Result<String> {
    Ok(match v {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        other => bail!("nested value cannot become a CSV cell: {other}"),
    })
}

pub fn json_to_csv(v: &Value, delimiter: u8) -> Result<String> {
    let rows = v
        .as_array()
        .context("top level must be a JSON array of objects")?;
    // Union of keys, sorted: ragged objects get empty cells, deterministically.
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for r in rows {
        let obj = r.as_object().context("every row must be an object")?;
        keys.extend(obj.keys().cloned());
    }
    let keys: Vec<String> = keys.into_iter().collect();
    let mut w = csv::WriterBuilder::new()
        .delimiter(delimiter)
        .from_writer(Vec::new());
    w.write_record(&keys)?;
    for r in rows {
        let obj = r.as_object().expect("checked");
        let mut rec = Vec::with_capacity(keys.len());
        for k in &keys {
            rec.push(match obj.get(k) {
                None => String::new(),
                Some(v) => scalar_to_cell(v)?,
            });
        }
        w.write_record(&rec)?;
    }
    Ok(String::from_utf8(w.into_inner()?)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_with_quoted_commas_and_newlines_roundtrips() {
        let csv = "name,note\nalice,\"hi, there\"\nbob,\"line1\nline2\"\n";
        let j = csv_to_json(csv, b',', false).unwrap();
        assert_eq!(j[0]["note"], "hi, there");
        assert_eq!(j[1]["note"], "line1\nline2");
        let back = json_to_csv(&j, b',').unwrap();
        let j2 = csv_to_json(&back, b',', false).unwrap();
        assert_eq!(j, j2, "csv -> json -> csv -> json is a fixed point");
    }

    #[test]
    fn inference_makes_real_types_and_empty_is_null() {
        let csv = "a,b,c,d,e\n42,3.5,true,,hello\n";
        let j = csv_to_json(csv, b',', true).unwrap();
        assert_eq!(j[0]["a"], 42);
        assert_eq!(j[0]["b"], 3.5);
        assert_eq!(j[0]["c"], true);
        assert_eq!(j[0]["d"], Value::Null);
        assert_eq!(j[0]["e"], "hello");
    }

    #[test]
    fn ragged_objects_union_headers_deterministically() {
        let v: Value = serde_json::from_str(r#"[{"b":1,"a":2},{"c":"x"}]"#).unwrap();
        let out = json_to_csv(&v, b',').unwrap();
        let mut lines = out.lines();
        assert_eq!(lines.next().unwrap(), "a,b,c");
        assert_eq!(lines.next().unwrap(), "2,1,");
        assert_eq!(lines.next().unwrap(), ",,x");
    }

    #[test]
    fn tsv_delimiter_works_and_nested_values_are_refused() {
        let j = csv_to_json("x\ty\n1\t2\n", b'\t', true).unwrap();
        assert_eq!(j[0]["y"], 2);
        let nested: Value = serde_json::from_str(r#"[{"a":{"deep":1}}]"#).unwrap();
        assert!(
            json_to_csv(&nested, b',').is_err(),
            "no silent flattening lies"
        );
    }
}
