//! gridcalc: a spreadsheet engine over CSV. Cells hold text, numbers, or
//! `=formulas` with + - * / parens, unary minus, cell refs (B3), and
//! SUM/AVG/MIN/MAX/COUNT over ranges (A1:C4). Evaluation memoizes and
//! detects reference cycles BY NAME — a spreadsheet that stack-overflows
//! on =A1 in A1 is not an engine, it is a grenade.

use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, HashMap, HashSet};

pub type Pos = (u32, u32); // (col, row), zero-based

#[derive(Default)]
pub struct Sheet {
    pub cells: BTreeMap<Pos, String>,
}

pub fn parse_ref(s: &str) -> Option<Pos> {
    let letters: String = s.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
    let digits = &s[letters.len()..];
    if letters.is_empty() || digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut col: u32 = 0;
    for c in letters.chars() {
        col = col * 26 + (c.to_ascii_uppercase() as u32 - 'A' as u32 + 1);
    }
    let row: u32 = digits.parse().ok()?;
    if row == 0 {
        return None;
    }
    Some((col - 1, row - 1))
}

pub fn ref_name(p: Pos) -> String {
    let (mut c, r) = (p.0 + 1, p.1 + 1);
    let mut name = String::new();
    while c > 0 {
        let rem = (c - 1) % 26;
        name.insert(0, (b'A' + rem as u8) as char);
        c = (c - 1) / 26;
    }
    format!("{name}{r}")
}

// ---------- tokens ----------
#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Ref(Pos),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    LParen,
    RParen,
    Comma,
    Colon,
}

fn lex(s: &str) -> Result<Vec<Tok>> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        match c {
            ' ' | '\t' => i += 1,
            '+' => {
                out.push(Tok::Plus);
                i += 1
            }
            '-' => {
                out.push(Tok::Minus);
                i += 1
            }
            '*' => {
                out.push(Tok::Star);
                i += 1
            }
            '/' => {
                out.push(Tok::Slash);
                i += 1
            }
            '(' => {
                out.push(Tok::LParen);
                i += 1
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1
            }
            ',' => {
                out.push(Tok::Comma);
                i += 1
            }
            ':' => {
                out.push(Tok::Colon);
                i += 1
            }
            '0'..='9' | '.' => {
                let start = i;
                while i < b.len() && ((b[i] as char).is_ascii_digit() || b[i] == b'.') {
                    i += 1;
                }
                out.push(Tok::Num(s[start..i].parse().context("bad number")?));
            }
            'A'..='Z' | 'a'..='z' => {
                let start = i;
                while i < b.len() && (b[i] as char).is_ascii_alphanumeric() {
                    i += 1;
                }
                let word = &s[start..i];
                match parse_ref(word) {
                    Some(p) => out.push(Tok::Ref(p)),
                    None => out.push(Tok::Ident(word.to_ascii_uppercase())),
                }
            }
            other => bail!("unexpected character {other:?} in formula"),
        }
    }
    Ok(out)
}

// ---------- parser: expr -> term -> factor ----------
#[derive(Debug, PartialEq)]
enum Expr {
    Num(f64),
    Ref(Pos),
    Neg(Box<Expr>),
    Bin(char, Box<Expr>, Box<Expr>),
    Call(String, Vec<Arg>),
}

#[derive(Debug, PartialEq)]
enum Arg {
    One(Expr),
    Range(Pos, Pos),
}

struct P<'a> {
    t: &'a [Tok],
    i: usize,
}

impl<'a> P<'a> {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.i)
    }
    fn eat(&mut self) -> Option<Tok> {
        let t = self.t.get(self.i).cloned();
        self.i += 1;
        t
    }
    fn expect(&mut self, want: &Tok) -> Result<()> {
        match self.eat() {
            Some(ref t) if t == want => Ok(()),
            other => bail!("expected {want:?}, got {other:?}"),
        }
    }

    fn expr(&mut self) -> Result<Expr> {
        let mut left = self.term()?;
        while let Some(op) = self.peek().cloned() {
            let c = match op {
                Tok::Plus => '+',
                Tok::Minus => '-',
                _ => break,
            };
            self.eat();
            left = Expr::Bin(c, Box::new(left), Box::new(self.term()?));
        }
        Ok(left)
    }

    fn term(&mut self) -> Result<Expr> {
        let mut left = self.factor()?;
        while let Some(op) = self.peek().cloned() {
            let c = match op {
                Tok::Star => '*',
                Tok::Slash => '/',
                _ => break,
            };
            self.eat();
            left = Expr::Bin(c, Box::new(left), Box::new(self.factor()?));
        }
        Ok(left)
    }

    fn factor(&mut self) -> Result<Expr> {
        match self.eat() {
            Some(Tok::Num(n)) => Ok(Expr::Num(n)),
            Some(Tok::Ref(p)) => Ok(Expr::Ref(p)),
            Some(Tok::Minus) => Ok(Expr::Neg(Box::new(self.factor()?))),
            Some(Tok::LParen) => {
                let e = self.expr()?;
                self.expect(&Tok::RParen)?;
                Ok(e)
            }
            Some(Tok::Ident(name)) => {
                self.expect(&Tok::LParen)?;
                let mut args = Vec::new();
                if self.peek() != Some(&Tok::RParen) {
                    loop {
                        args.push(self.arg()?);
                        match self.peek() {
                            Some(Tok::Comma) => {
                                self.eat();
                            }
                            _ => break,
                        }
                    }
                }
                self.expect(&Tok::RParen)?;
                Ok(Expr::Call(name, args))
            }
            other => bail!("unexpected token {other:?}"),
        }
    }

    fn arg(&mut self) -> Result<Arg> {
        // Range only appears here: Ref ':' Ref.
        if let (Some(Tok::Ref(a)), Some(Tok::Colon)) = (self.t.get(self.i), self.t.get(self.i + 1))
        {
            let a = *a;
            self.i += 2;
            match self.eat() {
                Some(Tok::Ref(b)) => return Ok(Arg::Range(a, b)),
                other => bail!("range needs a closing ref, got {other:?}"),
            }
        }
        Ok(Arg::One(self.expr()?))
    }
}

fn parse_formula(s: &str) -> Result<Expr> {
    let toks = lex(s)?;
    let mut p = P { t: &toks, i: 0 };
    let e = p.expr()?;
    if p.i != toks.len() {
        bail!("trailing tokens after formula");
    }
    Ok(e)
}

// ---------- evaluation ----------
impl Sheet {
    pub fn set(&mut self, name: &str, raw: &str) {
        let p = parse_ref(name).expect("valid ref");
        self.cells.insert(p, raw.to_string());
    }

    pub fn value(&self, name: &str) -> Result<f64> {
        let p = parse_ref(name).context("bad cell name")?;
        let mut memo = HashMap::new();
        let mut visiting = HashSet::new();
        self.eval_pos(p, &mut memo, &mut visiting)
    }

    fn eval_pos(
        &self,
        p: Pos,
        memo: &mut HashMap<Pos, f64>,
        visiting: &mut HashSet<Pos>,
    ) -> Result<f64> {
        if let Some(v) = memo.get(&p) {
            return Ok(*v);
        }
        if !visiting.insert(p) {
            bail!("reference cycle through {}", ref_name(p));
        }
        let raw = self.cells.get(&p).map(String::as_str).unwrap_or("");
        let v = if let Some(f) = raw.strip_prefix('=') {
            let expr = parse_formula(f)?;
            self.eval_expr(&expr, memo, visiting)?
        } else if raw.is_empty() {
            0.0
        } else {
            raw.trim().parse::<f64>().unwrap_or(0.0) // text is 0 for math
        };
        visiting.remove(&p);
        memo.insert(p, v);
        Ok(v)
    }

    fn range_cells(a: Pos, b: Pos) -> Vec<Pos> {
        let (c0, c1) = (a.0.min(b.0), a.0.max(b.0));
        let (r0, r1) = (a.1.min(b.1), a.1.max(b.1));
        let mut out = Vec::new();
        for r in r0..=r1 {
            for c in c0..=c1 {
                out.push((c, r));
            }
        }
        out
    }

    fn eval_expr(
        &self,
        e: &Expr,
        memo: &mut HashMap<Pos, f64>,
        visiting: &mut HashSet<Pos>,
    ) -> Result<f64> {
        Ok(match e {
            Expr::Num(n) => *n,
            Expr::Ref(p) => self.eval_pos(*p, memo, visiting)?,
            Expr::Neg(x) => -self.eval_expr(x, memo, visiting)?,
            Expr::Bin(op, l, r) => {
                let (l, r) = (
                    self.eval_expr(l, memo, visiting)?,
                    self.eval_expr(r, memo, visiting)?,
                );
                match op {
                    '+' => l + r,
                    '-' => l - r,
                    '*' => l * r,
                    '/' => l / r,
                    _ => unreachable!(),
                }
            }
            Expr::Call(name, args) => {
                let mut vals = Vec::new();
                let mut nonempty = 0usize;
                for a in args {
                    match a {
                        Arg::One(x) => {
                            vals.push(self.eval_expr(x, memo, visiting)?);
                            nonempty += 1;
                        }
                        Arg::Range(a, b) => {
                            for p in Self::range_cells(*a, *b) {
                                let filled =
                                    self.cells.get(&p).map(|s| !s.is_empty()).unwrap_or(false);
                                if filled {
                                    nonempty += 1;
                                }
                                vals.push(self.eval_pos(p, memo, visiting)?);
                            }
                        }
                    }
                }
                match name.as_str() {
                    "SUM" => vals.iter().sum(),
                    "AVG" => {
                        if vals.is_empty() {
                            bail!("AVG of nothing")
                        }
                        vals.iter().sum::<f64>() / vals.len() as f64
                    }
                    "MIN" => vals.iter().cloned().fold(f64::INFINITY, f64::min),
                    "MAX" => vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
                    "COUNT" => nonempty as f64,
                    other => bail!("unknown function {other}"),
                }
            }
        })
    }

    // ---------- CSV ----------
    pub fn from_csv(text: &str) -> Result<Sheet> {
        let mut rdr = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .from_reader(text.as_bytes());
        let mut sheet = Sheet::default();
        for (r, rec) in rdr.records().enumerate() {
            for (c, cell) in rec?.iter().enumerate() {
                if !cell.is_empty() {
                    sheet.cells.insert((c as u32, r as u32), cell.to_string());
                }
            }
        }
        Ok(sheet)
    }

    /// Materialize every cell to its computed value (formulas evaluated).
    pub fn to_values_csv(&self) -> Result<String> {
        let (mut maxc, mut maxr) = (0u32, 0u32);
        for &(c, r) in self.cells.keys() {
            maxc = maxc.max(c);
            maxr = maxr.max(r);
        }
        let mut w = csv::WriterBuilder::new().from_writer(Vec::new());
        for r in 0..=maxr {
            let mut row = Vec::new();
            for c in 0..=maxc {
                let raw = self.cells.get(&(c, r)).map(String::as_str).unwrap_or("");
                let cell = if raw.starts_with('=') {
                    fmt_num(self.value(&ref_name((c, r)))?)
                } else {
                    raw.to_string()
                };
                row.push(cell);
            }
            w.write_record(&row)?;
        }
        Ok(String::from_utf8(w.into_inner()?)?)
    }
}

pub fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(pairs: &[(&str, &str)]) -> Sheet {
        let mut s = Sheet::default();
        for (k, v) in pairs {
            s.set(k, v);
        }
        s
    }

    #[test]
    fn refs_roundtrip_including_double_letters() {
        for name in ["A1", "Z9", "AA1", "AB12", "BA100"] {
            assert_eq!(ref_name(parse_ref(name).unwrap()), name);
        }
        assert!(parse_ref("A0").is_none());
        assert!(parse_ref("1A").is_none());
    }

    #[test]
    fn precedence_parens_and_unary_minus() {
        let s = sheet(&[
            ("A1", "=2+3*4"),
            ("A2", "=(2+3)*4"),
            ("A3", "=-3+10"),
            ("A4", "=8/4/2"),
        ]);
        assert_eq!(s.value("A1").unwrap(), 14.0);
        assert_eq!(s.value("A2").unwrap(), 20.0);
        assert_eq!(s.value("A3").unwrap(), 7.0);
        assert_eq!(s.value("A4").unwrap(), 1.0, "division is left-associative");
    }

    #[test]
    fn references_chain_and_text_is_zero_for_math() {
        let s = sheet(&[
            ("A1", "5"),
            ("B1", "=A1*2"),
            ("C1", "=B1+A1"),
            ("D1", "hello"),
            ("E1", "=D1+1"),
        ]);
        assert_eq!(s.value("C1").unwrap(), 15.0);
        assert_eq!(s.value("E1").unwrap(), 1.0);
    }

    #[test]
    fn functions_over_ranges_and_count_counts_nonempty() {
        let s = sheet(&[
            ("A1", "1"),
            ("A2", "2"),
            ("A3", "3"),
            ("B1", "10"),
            ("B3", "note"),
            ("C1", "=SUM(A1:B3)"),
            ("C2", "=AVG(A1:A3)"),
            ("C3", "=MIN(A1:A3, 0)"),
            ("C4", "=MAX(A1:A3)"),
            ("C5", "=COUNT(A1:B3)"),
            ("C6", "=SUM(A1, A2, 100)"),
        ]);
        assert_eq!(s.value("C1").unwrap(), 16.0, "text and empty are 0 for SUM");
        assert_eq!(s.value("C2").unwrap(), 2.0);
        assert_eq!(s.value("C3").unwrap(), 0.0);
        assert_eq!(s.value("C4").unwrap(), 3.0);
        assert_eq!(s.value("C5").unwrap(), 5.0, "COUNT counts nonempty cells");
        assert_eq!(s.value("C6").unwrap(), 103.0);
    }

    #[test]
    fn cycles_are_named_not_stack_overflowed() {
        let s = sheet(&[("A1", "=B1"), ("B1", "=A1+1")]);
        let err = s.value("A1").unwrap_err().to_string();
        assert!(err.contains("cycle"), "{err}");
        assert!(
            err.contains("A1") || err.contains("B1"),
            "names the cell: {err}"
        );
        let s = sheet(&[("A1", "=A1")]);
        assert!(
            s.value("A1").is_err(),
            "self-reference is the tightest cycle"
        );
    }

    #[test]
    fn csv_in_values_csv_out() {
        let text = "1,2,=A1+B1\n=SUM(A1:B1),,=C1*2\n";
        let s = Sheet::from_csv(text).unwrap();
        assert_eq!(s.value("C1").unwrap(), 3.0);
        let out = s.to_values_csv().unwrap();
        assert_eq!(out, "1,2,3\n3,,6\n");
        assert!(
            Sheet::from_csv("=SUM(\n").unwrap().value("A1").is_err(),
            "bad formula errs"
        );
    }
}
