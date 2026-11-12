//! Guards: small, declarative conditions over the show context.
//!
//! Grammar (one expression, no nesting):
//!   key            truthy
//!   !key           falsy / missing
//!   key OP value   OP is ==, !=, >, >=, <, <=; value is a JSON literal or a bare word
//!
//! Guards live in the chart as data, e.g. `"guards": {"energy_high": "energy > 0.7"}`,
//! so thresholds travel with the show file instead of being compiled into the engine.

use serde_json::Value;
use std::collections::BTreeMap;

/// Shared show data guards read from. Ordered, so serialisation is deterministic.
pub type Context = BTreeMap<String, Value>;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum GuardError {
    #[error("empty guard expression")]
    Empty,
    #[error("cannot parse guard '{0}' (expected: key, !key, or key OP value)")]
    Parse(String),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

#[derive(Debug, PartialEq)]
enum Expr {
    Truthy(String),
    Not(String),
    Cmp(String, Op, Value),
}

fn is_key(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
}

fn parse(src: &str) -> Result<Expr, GuardError> {
    let s = src.trim();
    if s.is_empty() {
        return Err(GuardError::Empty);
    }
    if let Some(k) = s.strip_prefix('!') {
        let k = k.trim();
        return if is_key(k) { Ok(Expr::Not(k.into())) } else { Err(GuardError::Parse(src.into())) };
    }
    // two-character operators first so ">=" is not read as ">"
    for (tok, op) in [("==", Op::Eq), ("!=", Op::Ne), (">=", Op::Ge), ("<=", Op::Le), (">", Op::Gt), ("<", Op::Lt)] {
        if let Some((l, r)) = s.split_once(tok) {
            let (l, r) = (l.trim(), r.trim());
            if !is_key(l) || r.is_empty() {
                return Err(GuardError::Parse(src.into()));
            }
            let v = serde_json::from_str(r).unwrap_or_else(|_| Value::String(r.to_string()));
            return Ok(Expr::Cmp(l.into(), op, v));
        }
    }
    if is_key(s) { Ok(Expr::Truthy(s.into())) } else { Err(GuardError::Parse(src.into())) }
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

fn compare(lhs: Option<&Value>, op: Op, rhs: &Value) -> bool {
    let Some(lhs) = lhs else {
        // a missing key is only ever "not equal"
        return op == Op::Ne;
    };
    if let (Some(a), Some(b)) = (lhs.as_f64(), rhs.as_f64()) {
        return match op {
            Op::Eq => a == b,
            Op::Ne => a != b,
            Op::Gt => a > b,
            Op::Ge => a >= b,
            Op::Lt => a < b,
            Op::Le => a <= b,
        };
    }
    match op {
        Op::Eq => lhs == rhs,
        Op::Ne => lhs != rhs,
        _ => false, // ordering only makes sense for numbers
    }
}

fn lookup<'a>(ctx: &'a Context, key: &str) -> Option<&'a Value> {
    // dotted keys walk into objects: "venue.capacity"
    let mut parts = key.split('.');
    let mut cur = ctx.get(parts.next()?)?;
    for p in parts {
        cur = cur.get(p)?;
    }
    Some(cur)
}

/// Check an expression parses (used by chart validation).
pub fn check(expr: &str) -> Result<(), GuardError> {
    parse(expr).map(|_| ())
}

pub fn evaluate(expr: &str, ctx: &Context) -> Result<bool, GuardError> {
    Ok(match parse(expr)? {
        Expr::Truthy(k) => truthy(lookup(ctx, &k)),
        Expr::Not(k) => !truthy(lookup(ctx, &k)),
        Expr::Cmp(k, op, v) => compare(lookup(ctx, &k), op, &v),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx() -> Context {
        let mut c = Context::new();
        c.insert("energy".into(), json!(0.82));
        c.insert("audience_count".into(), json!(120));
        c.insert("phase".into(), json!("intro"));
        c.insert("live".into(), json!(true));
        c.insert("venue".into(), json!({"capacity": 500}));
        c
    }

    #[test]
    fn numeric_comparisons() {
        let c = ctx();
        assert_eq!(evaluate("energy > 0.7", &c), Ok(true));
        assert_eq!(evaluate("energy >= 0.82", &c), Ok(true));
        assert_eq!(evaluate("energy < 0.5", &c), Ok(false));
        assert_eq!(evaluate("audience_count == 120", &c), Ok(true));
        assert_eq!(evaluate("audience_count != 120", &c), Ok(false));
    }

    #[test]
    fn strings_booleans_and_dotted_keys() {
        let c = ctx();
        assert_eq!(evaluate("phase == intro", &c), Ok(true)); // bare word
        assert_eq!(evaluate(r#"phase == "intro""#, &c), Ok(true));
        assert_eq!(evaluate("live", &c), Ok(true));
        assert_eq!(evaluate("!live", &c), Ok(false));
        assert_eq!(evaluate("venue.capacity > 400", &c), Ok(true));
    }

    #[test]
    fn missing_keys_are_false_not_errors() {
        let c = ctx();
        assert_eq!(evaluate("nope", &c), Ok(false));
        assert_eq!(evaluate("!nope", &c), Ok(true));
        assert_eq!(evaluate("nope > 1", &c), Ok(false));
        assert_eq!(evaluate("nope != 1", &c), Ok(true));
        assert_eq!(evaluate("phase > 3", &c), Ok(false)); // ordering on a string
    }

    #[test]
    fn bad_expressions_are_errors() {
        assert_eq!(evaluate("", &ctx()), Err(GuardError::Empty));
        assert!(matches!(check("energy >"), Err(GuardError::Parse(_))));
        assert!(matches!(check("> 3"), Err(GuardError::Parse(_))));
        assert!(matches!(check("two words"), Err(GuardError::Parse(_))));
    }
}
