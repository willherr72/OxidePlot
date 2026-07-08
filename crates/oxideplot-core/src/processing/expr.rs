use crate::data::loader::{column_to_f64, resolve_col, LoadedData};
use std::collections::{HashMap, HashSet};

// ---- Mini arithmetic expression evaluator (derive_column op="expr") ----
// Supports numbers, column names, + - * / ^, parentheses, unary minus, and
// functions: sqrt abs sin cos tan asin acos atan atan2 hypot pow exp ln log10
// log floor ceil round sign deg rad min max.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Rel {
    Gt,
    Lt,
    Ge,
    Le,
    Eq,
    Ne,
}

#[derive(Clone)]
pub enum Ast {
    Num(f64),
    Col(usize),
    Neg(Box<Ast>),
    Bin(char, Box<Ast>, Box<Ast>),
    Cmp(Rel, Box<Ast>, Box<Ast>),
    And(Box<Ast>, Box<Ast>),
    Or(Box<Ast>, Box<Ast>),
    Func(String, Vec<Ast>),
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Op(char),
    Cmp(Rel),
    And,
    Or,
    LParen,
    RParen,
    Comma,
}

fn tokenize_expr(s: &str) -> Result<Vec<Tok>, String> {
    let b = s.as_bytes();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as char;
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || c == '.' {
            let start = i;
            while i < b.len() {
                let ch = b[i] as char;
                if ch.is_ascii_digit() || ch == '.' {
                    i += 1;
                } else if ch == 'e' || ch == 'E' {
                    i += 1;
                    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
                        i += 1;
                    }
                } else {
                    break;
                }
            }
            let n: f64 = s[start..i]
                .parse()
                .map_err(|_| format!("bad number '{}'", &s[start..i]))?;
            toks.push(Tok::Num(n));
        } else if c == '"' {
            // Quoted column name — allows spaces/symbols in real headers, e.g.
            // "Temp PV °C" or "T3 X". Content is taken verbatim (resolve_col does
            // an exact name match). Quotes are ASCII so the byte slice lands on
            // char boundaries even with multi-byte content between them.
            let start = i + 1;
            i += 1;
            while i < b.len() && b[i] != b'"' {
                i += 1;
            }
            if i >= b.len() {
                return Err("unterminated quoted column name".to_string());
            }
            let word = &s[start..i];
            i += 1; // consume closing quote
            toks.push(Tok::Ident(word.to_string()));
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < b.len() && ((b[i] as char).is_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let word = &s[start..i];
            match word.to_ascii_lowercase().as_str() {
                "and" => toks.push(Tok::And),
                "or" => toks.push(Tok::Or),
                _ => toks.push(Tok::Ident(word.to_string())),
            }
        } else {
            let next = if i + 1 < b.len() { b[i + 1] as char } else { '\0' };
            match c {
                '+' | '-' | '*' | '/' | '^' => {
                    toks.push(Tok::Op(c));
                    i += 1;
                }
                '>' if next == '=' => {
                    toks.push(Tok::Cmp(Rel::Ge));
                    i += 2;
                }
                '<' if next == '=' => {
                    toks.push(Tok::Cmp(Rel::Le));
                    i += 2;
                }
                '=' if next == '=' => {
                    toks.push(Tok::Cmp(Rel::Eq));
                    i += 2;
                }
                '!' if next == '=' => {
                    toks.push(Tok::Cmp(Rel::Ne));
                    i += 2;
                }
                '>' => {
                    toks.push(Tok::Cmp(Rel::Gt));
                    i += 1;
                }
                '<' => {
                    toks.push(Tok::Cmp(Rel::Lt));
                    i += 1;
                }
                '&' if next == '&' => {
                    toks.push(Tok::And);
                    i += 2;
                }
                '|' if next == '|' => {
                    toks.push(Tok::Or);
                    i += 2;
                }
                '(' => {
                    toks.push(Tok::LParen);
                    i += 1;
                }
                ')' => {
                    toks.push(Tok::RParen);
                    i += 1;
                }
                ',' => {
                    toks.push(Tok::Comma);
                    i += 1;
                }
                _ => return Err(format!("unexpected character '{c}'")),
            }
        }
    }
    Ok(toks)
}

struct ExprParser<'a> {
    toks: Vec<Tok>,
    pos: usize,
    data: &'a LoadedData,
}

impl ExprParser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn bump(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }
    fn parse(&mut self) -> Result<Ast, String> {
        let e = self.or_expr()?;
        if self.pos != self.toks.len() {
            return Err("trailing tokens in expression".to_string());
        }
        Ok(e)
    }
    fn or_expr(&mut self) -> Result<Ast, String> {
        let mut left = self.and_expr()?;
        while let Some(Tok::Or) = self.peek() {
            self.pos += 1;
            let right = self.and_expr()?;
            left = Ast::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }
    fn and_expr(&mut self) -> Result<Ast, String> {
        let mut left = self.rel_expr()?;
        while let Some(Tok::And) = self.peek() {
            self.pos += 1;
            let right = self.rel_expr()?;
            left = Ast::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }
    fn rel_expr(&mut self) -> Result<Ast, String> {
        let left = self.add_sub()?;
        if let Some(Tok::Cmp(rel)) = self.peek().cloned() {
            self.pos += 1;
            let right = self.add_sub()?;
            return Ok(Ast::Cmp(rel, Box::new(left), Box::new(right)));
        }
        Ok(left)
    }
    fn add_sub(&mut self) -> Result<Ast, String> {
        let mut left = self.mul_div()?;
        while let Some(Tok::Op(c @ ('+' | '-'))) = self.peek().cloned() {
            self.pos += 1;
            let right = self.mul_div()?;
            left = Ast::Bin(c, Box::new(left), Box::new(right));
        }
        Ok(left)
    }
    fn mul_div(&mut self) -> Result<Ast, String> {
        let mut left = self.unary()?;
        while let Some(Tok::Op(c @ ('*' | '/'))) = self.peek().cloned() {
            self.pos += 1;
            let right = self.unary()?;
            left = Ast::Bin(c, Box::new(left), Box::new(right));
        }
        Ok(left)
    }
    fn unary(&mut self) -> Result<Ast, String> {
        match self.peek() {
            Some(Tok::Op('-')) => {
                self.pos += 1;
                Ok(Ast::Neg(Box::new(self.unary()?)))
            }
            Some(Tok::Op('+')) => {
                self.pos += 1;
                self.unary()
            }
            _ => self.power(),
        }
    }
    fn power(&mut self) -> Result<Ast, String> {
        let base = self.atom()?;
        if let Some(Tok::Op('^')) = self.peek() {
            self.pos += 1;
            let exp = self.unary()?;
            return Ok(Ast::Bin('^', Box::new(base), Box::new(exp)));
        }
        Ok(base)
    }
    fn atom(&mut self) -> Result<Ast, String> {
        match self.bump() {
            Some(Tok::Num(n)) => Ok(Ast::Num(n)),
            Some(Tok::LParen) => {
                let e = self.or_expr()?;
                match self.bump() {
                    Some(Tok::RParen) => Ok(e),
                    _ => Err("expected ')'".to_string()),
                }
            }
            Some(Tok::Ident(name)) => {
                if let Some(Tok::LParen) = self.peek() {
                    self.pos += 1;
                    let mut args = Vec::new();
                    if !matches!(self.peek(), Some(Tok::RParen)) {
                        loop {
                            args.push(self.or_expr()?);
                            if let Some(Tok::Comma) = self.peek() {
                                self.pos += 1;
                            } else {
                                break;
                            }
                        }
                    }
                    match self.bump() {
                        Some(Tok::RParen) => {}
                        _ => return Err("expected ')' after function arguments".to_string()),
                    }
                    Ok(Ast::Func(name.to_ascii_lowercase(), args))
                } else {
                    let ci = resolve_col(self.data, &name)
                        .ok_or_else(|| format!("unknown column '{name}'"))?;
                    Ok(Ast::Col(ci))
                }
            }
            other => Err(format!("unexpected token: {other:?}")),
        }
    }
}

pub fn parse_expr(data: &LoadedData, s: &str) -> Result<Ast, String> {
    let toks = tokenize_expr(s)?;
    if toks.is_empty() {
        return Err("empty expression".to_string());
    }
    ExprParser {
        toks,
        pos: 0,
        data,
    }
    .parse()
}

pub fn collect_expr_cols(a: &Ast, out: &mut HashSet<usize>) {
    match a {
        Ast::Col(ci) => {
            out.insert(*ci);
        }
        Ast::Neg(e) => collect_expr_cols(e, out),
        Ast::Bin(_, l, r) | Ast::Cmp(_, l, r) | Ast::And(l, r) | Ast::Or(l, r) => {
            collect_expr_cols(l, out);
            collect_expr_cols(r, out);
        }
        Ast::Func(_, args) => args.iter().for_each(|e| collect_expr_cols(e, out)),
        Ast::Num(_) => {}
    }
}

pub fn eval_expr(a: &Ast, cols: &HashMap<usize, Vec<f64>>, row: usize) -> f64 {
    match a {
        Ast::Num(n) => *n,
        Ast::Col(ci) => cols
            .get(ci)
            .and_then(|v| v.get(row))
            .copied()
            .unwrap_or(f64::NAN),
        Ast::Neg(e) => -eval_expr(e, cols, row),
        Ast::Bin(c, l, r) => {
            let a = eval_expr(l, cols, row);
            let b = eval_expr(r, cols, row);
            match c {
                '+' => a + b,
                '-' => a - b,
                '*' => a * b,
                '/' => a / b,
                '^' => a.powf(b),
                _ => f64::NAN,
            }
        }
        Ast::Cmp(rel, l, r) => {
            let a = eval_expr(l, cols, row);
            let b = eval_expr(r, cols, row);
            let t = match rel {
                Rel::Gt => a > b,
                Rel::Lt => a < b,
                Rel::Ge => a >= b,
                Rel::Le => a <= b,
                Rel::Eq => a == b,
                Rel::Ne => a != b,
            };
            if t {
                1.0
            } else {
                0.0
            }
        }
        Ast::And(l, r) => {
            if eval_expr(l, cols, row) != 0.0 && eval_expr(r, cols, row) != 0.0 {
                1.0
            } else {
                0.0
            }
        }
        Ast::Or(l, r) => {
            if eval_expr(l, cols, row) != 0.0 || eval_expr(r, cols, row) != 0.0 {
                1.0
            } else {
                0.0
            }
        }
        Ast::Func(name, args) => {
            let v: Vec<f64> = args.iter().map(|e| eval_expr(e, cols, row)).collect();
            let x = v.first().copied().unwrap_or(f64::NAN);
            let y = v.get(1).copied().unwrap_or(f64::NAN);
            match name.as_str() {
                "sqrt" => x.sqrt(),
                "abs" => x.abs(),
                "sin" => x.sin(),
                "cos" => x.cos(),
                "tan" => x.tan(),
                "asin" => x.asin(),
                "acos" => x.acos(),
                "atan" => x.atan(),
                "atan2" => x.atan2(y),
                "hypot" => x.hypot(y),
                "pow" => x.powf(y),
                "exp" => x.exp(),
                "ln" => x.ln(),
                "log10" | "log" => x.log10(),
                "floor" => x.floor(),
                "ceil" => x.ceil(),
                "round" => x.round(),
                "sign" => x.signum(),
                "deg" => x.to_degrees(),
                "rad" => x.to_radians(),
                "min" => v.iter().copied().fold(f64::INFINITY, f64::min),
                "max" => v.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                _ => f64::NAN,
            }
        }
    }
}

/// Keep the subset of `rows` (row indices) for which the boolean `filter`
/// expression is true (non-zero and finite). Errors on a bad expression.
pub fn apply_filter(data: &LoadedData, rows: &[usize], filter: &str) -> Result<Vec<usize>, String> {
    let ast = parse_expr(data, filter)?;
    let mut refs = HashSet::new();
    collect_expr_cols(&ast, &mut refs);
    let colvals: HashMap<usize, Vec<f64>> = refs
        .iter()
        .map(|&ci| (ci, column_to_f64(&data.column_data[ci]).0))
        .collect();
    Ok(rows
        .iter()
        .copied()
        .filter(|&r| {
            let v = eval_expr(&ast, &colvals, r);
            v.is_finite() && v != 0.0
        })
        .collect())
}

/// Trailing-window rolling statistic per row: rolling_mean/std/min/max over cols[0],
/// or rolling_corr (Pearson) over cols[0] vs cols[1]. Window = last `win` rows.
pub fn rolling_compute(op: &str, cols: &[Vec<f64>], win: usize, n_rows: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(n_rows);
    for r in 0..n_rows {
        let lo = (r + 1).saturating_sub(win);
        let hi = r + 1;
        let v = if op == "rolling_corr" {
            let a = cols[0].get(lo..hi).unwrap_or(&[]);
            let b = cols[1].get(lo..hi).unwrap_or(&[]);
            crate::processing::statistics::pearson(a, b).unwrap_or(f64::NAN)
        } else {
            let w: Vec<f64> = cols[0]
                .get(lo..hi)
                .unwrap_or(&[])
                .iter()
                .copied()
                .filter(|x| x.is_finite())
                .collect();
            if w.is_empty() {
                f64::NAN
            } else {
                let m = w.iter().sum::<f64>() / w.len() as f64;
                match op {
                    "rolling_mean" => m,
                    "rolling_min" => w.iter().copied().fold(f64::INFINITY, f64::min),
                    "rolling_max" => w.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                    "rolling_std" => {
                        (w.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / w.len() as f64).sqrt()
                    }
                    _ => f64::NAN,
                }
            }
        };
        out.push(v);
    }
    out
}

#[cfg(test)]
mod expr_tests {
    use super::*;
    use crate::data::loader::LoadedData;

    fn dataset() -> LoadedData {
        // ax=3, ay=4, az=0 for all 5 rows.
        let col = |v: &str| vec![v.to_string(); 5];
        LoadedData {
            columns: vec!["ax".into(), "ay".into(), "az".into()],
            column_data: vec![col("3"), col("4"), col("0")],
            row_count: 5,
        }
    }

    fn eval_all(d: &LoadedData, s: &str) -> Vec<f64> {
        let ast = parse_expr(d, s).unwrap();
        let mut refs = HashSet::new();
        collect_expr_cols(&ast, &mut refs);
        let cols: HashMap<usize, Vec<f64>> = refs
            .iter()
            .map(|&ci| (ci, column_to_f64(&d.column_data[ci]).0))
            .collect();
        (0..d.row_count).map(|r| eval_expr(&ast, &cols, r)).collect()
    }

    #[test]
    fn magnitude_via_expr() {
        let d = dataset();
        assert!((eval_all(&d, "sqrt(ax^2 + ay^2 + az^2)")[0] - 5.0).abs() < 1e-9);
    }

    #[test]
    fn quoted_multiword_column_names() {
        // Real instrument headers have spaces/symbols; quotes reference them.
        let col = |v: &str| vec![v.to_string(); 3];
        let d = LoadedData {
            columns: vec!["T3 X".into(), "Temp PV °C".into()],
            column_data: vec![col("3"), col("4")],
            row_count: 3,
        };
        let out = eval_all(&d, "\"T3 X\" + \"Temp PV °C\"");
        assert!((out[0] - 7.0).abs() < 1e-9);
    }

    #[test]
    fn trig_survey_math() {
        let d = dataset();
        assert!((eval_all(&d, "deg(atan2(ay, ax))")[0] - 53.13010235).abs() < 1e-6);
    }

    #[test]
    fn comparisons_and_logic() {
        let d = dataset();
        assert_eq!(eval_all(&d, "ax > 2 and ay < 5")[0], 1.0);
        assert_eq!(eval_all(&d, "ax > 5 or az == 0")[0], 1.0);
        assert_eq!(eval_all(&d, "ax == ay")[0], 0.0);
    }

    #[test]
    fn unknown_column_errors() {
        let d = dataset();
        assert!(parse_expr(&d, "nope * 2").is_err());
    }

    #[test]
    fn filter_selects_rows() {
        // one row where ax=99, rest ax=3
        let mut d = dataset();
        d.column_data[0][2] = "99".into();
        let kept = apply_filter(&d, &(0..5).collect::<Vec<_>>(), "ax > 50").unwrap();
        assert_eq!(kept, vec![2]);
    }

    #[test]
    fn rolling_mean_trailing_window() {
        let cols = vec![vec![0.0, 2.0, 4.0, 6.0]];
        let out = rolling_compute("rolling_mean", &cols, 2, 4);
        // trailing window of 2: [0], [0,2]→1, [2,4]→3, [4,6]→5
        assert_eq!(out, vec![0.0, 1.0, 3.0, 5.0]);
    }

    #[test]
    fn rolling_corr_trailing_window() {
        // Two perfectly correlated columns (b = 2a) → trailing rolling correlation
        // is 1.0 once the window has >= 2 points. Exercises the pearson seam.
        let a = vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0];
        let b = vec![0.0, 2.0, 4.0, 6.0, 8.0, 10.0];
        let out = rolling_compute("rolling_corr", &[a, b], 3, 6);
        assert!(out[0].is_nan(), "row 0 has a 1-point window → pearson None → NaN");
        assert!((out[5] - 1.0).abs() < 1e-9, "expected r≈1 for the trailing window, got {}", out[5]);
    }
}
