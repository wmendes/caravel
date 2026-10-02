//! Expressions in a deployment's strings (M0.6, C-07, DEC-079):
//! `"${lane.name}-admin"`, `"${length(var.validators) * 2 / 3 + 1}"`.
//!
//! - A string that is exactly one `${…}` takes the expression's type, so
//!   `threshold = "${var.n}"` is a number. Any other string with `${…}` in
//!   it is a string; `$${` writes a literal `${`.
//! - Values: null, booleans, integers, strings, lists, maps. Integers are
//!   64-bit and checked: overflow and division by zero are errors, `/`
//!   truncates. TOML floats and dates pass through but can't be computed
//!   with. Nothing reads the clock, randomness or files: the same lane file
//!   and inputs always give the same deployment.
//! - Grammar, loosest first: `c ? a : b`, `||`, `&&`, `== != < <= > >=`,
//!   `+ -`, `* / %`, `! -` (unary), then `.name`, `[index]` and calls.
//!   Literals: `1`, `"s"` or `'s'`, `true`, `false`, `null`, `[a, b]`,
//!   `{ k = v }`, `[for x in xs : f(x) if cond]` (`for k, v in m` too).
//!   Names may contain `-` (`node.validator-1`), so subtraction needs
//!   spaces.
//! - A value only known after Stellar and the host are read (a contract's
//!   address) is [`Value::Deferred`]: anything computed from it is deferred
//!   too, and filled in later.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::ops::Range;

/// Nesting deeper than this is refused (no stack overflow on any input).
const MAX_DEPTH: usize = 64;
/// Lists longer than this are refused (`range`, comprehensions).
const MAX_LEN: usize = 10_000;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
    /// A TOML float or date: kept as written, not computed with.
    Opaque(toml::Value),
    /// Known later; the references it needs (e.g. `contract.settlement.address`).
    Deferred(BTreeSet<String>),
}

impl Value {
    pub fn from_toml(v: &toml::Value) -> Self {
        match v {
            toml::Value::String(s) => Self::Str(s.clone()),
            toml::Value::Integer(i) => Self::Int(*i),
            toml::Value::Boolean(b) => Self::Bool(*b),
            toml::Value::Array(a) => Self::List(a.iter().map(Self::from_toml).collect()),
            toml::Value::Table(t) => Self::Map(
                t.iter()
                    .map(|(k, v)| (k.clone(), Self::from_toml(v)))
                    .collect(),
            ),
            other => Self::Opaque(other.clone()),
        }
    }

    /// The TOML value; `None` for null (the key is left out). A deferred
    /// value has none yet.
    pub fn to_toml(&self) -> Result<Option<toml::Value>, String> {
        Ok(Some(match self {
            Self::Null => return Ok(None),
            Self::Bool(b) => toml::Value::Boolean(*b),
            Self::Int(i) => toml::Value::Integer(*i),
            Self::Str(s) => toml::Value::String(s.clone()),
            Self::Opaque(v) => v.clone(),
            Self::List(l) => {
                let mut out = Vec::with_capacity(l.len());
                for v in l {
                    out.push(v.to_toml()?.ok_or("a list can't hold null")?);
                }
                toml::Value::Array(out)
            }
            Self::Map(m) => {
                let mut out = toml::Table::new();
                for (k, v) in m {
                    if let Some(v) = v.to_toml()? {
                        out.insert(k.clone(), v);
                    }
                }
                toml::Value::Table(out)
            }
            Self::Deferred(refs) => {
                return Err(format!(
                    "this needs {}, which isn't known yet",
                    refs.iter().cloned().collect::<Vec<_>>().join(", ")
                ))
            }
        }))
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "a boolean",
            Self::Int(_) => "a number",
            Self::Str(_) => "a string",
            Self::List(_) => "a list",
            Self::Map(_) => "a map",
            Self::Opaque(_) => "a float or date",
            Self::Deferred(_) => "a value known later",
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => write!(f, "null"),
            Self::Bool(b) => write!(f, "{b}"),
            Self::Int(i) => write!(f, "{i}"),
            Self::Str(s) => write!(f, "{s:?}"),
            Self::Opaque(v) => write!(f, "{v}"),
            Self::List(l) => {
                write!(f, "[")?;
                for (i, v) in l.iter().enumerate() {
                    write!(f, "{}{v}", if i > 0 { ", " } else { "" })?;
                }
                write!(f, "]")
            }
            Self::Map(m) => {
                write!(f, "{{")?;
                for (i, (k, v)) in m.iter().enumerate() {
                    write!(f, "{}{k} = {v}", if i > 0 { ", " } else { " " })?;
                }
                write!(f, " }}")
            }
            Self::Deferred(r) => write!(
                f,
                "(known later: {})",
                r.iter().cloned().collect::<Vec<_>>().join(", ")
            ),
        }
    }
}

/// The names an expression can start from.
pub trait Scope {
    fn get(&self, name: &str) -> Option<Value>;
    /// For "did you mean".
    fn names(&self) -> Vec<String>;
}

/// A scope from a map.
#[derive(Clone, Debug, Default)]
pub struct MapScope(pub BTreeMap<String, Value>);

impl Scope for MapScope {
    fn get(&self, name: &str) -> Option<Value> {
        self.0.get(name).cloned()
    }
    fn names(&self) -> Vec<String> {
        self.0.keys().cloned().collect()
    }
}

/// A problem in an expression, at a byte range of the string it is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExprError {
    pub message: String,
    pub at: Range<usize>,
    pub help: Option<String>,
}

impl ExprError {
    fn new(message: impl Into<String>, at: Range<usize>) -> Self {
        Self {
            message: message.into(),
            at,
            help: None,
        }
    }
}

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(h) = &self.help {
            write!(f, " ({h})")?;
        }
        Ok(())
    }
}

type R<T> = Result<T, ExprError>;

#[derive(Clone, Debug, PartialEq)]
enum Ex {
    Lit(Value),
    /// A string literal with `${…}` in it.
    Tmpl(Template),
    Name(String),
    Attr(Box<Node>, String),
    Index(Box<Node>, Box<Node>),
    Call(String, Vec<Node>),
    List(Vec<Node>),
    Map(Vec<(String, Node)>),
    For {
        key: Option<String>,
        value: String,
        over: Box<Node>,
        body: Box<Node>,
        cond: Option<Box<Node>>,
    },
    Unary(char, Box<Node>),
    Binary(&'static str, Box<Node>, Box<Node>),
    Cond(Box<Node>, Box<Node>, Box<Node>),
}

#[derive(Clone, Debug, PartialEq)]
struct Node {
    ex: Ex,
    at: Range<usize>,
}

/// One part of a string: text, or an expression.
#[derive(Clone, Debug, PartialEq)]
enum Part {
    Text(String),
    Expr(Node),
}

/// A string with its `${…}` parsed.
#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    parts: Vec<Part>,
    /// Inside an expression, a string literal is always a string.
    literal: bool,
}

/// Whether a string has an expression in it (a `${` that isn't `$${`).
pub fn has_expression(s: &str) -> bool {
    let b = s.as_bytes();
    (0..b.len().saturating_sub(1))
        .any(|i| b[i] == b'$' && b[i + 1] == b'{' && (i == 0 || b[i - 1] != b'$'))
}

impl Template {
    pub fn parse(s: &str) -> R<Self> {
        Self::parse_at(s, 0)
    }

    fn parse_at(s: &str, depth: usize) -> R<Self> {
        if depth > MAX_DEPTH {
            return Err(ExprError::new(
                "this expression is nested too deeply",
                0..s.len(),
            ));
        }
        let mut parts = Vec::new();
        let mut text = String::new();
        let mut i = 0;
        let b = s.as_bytes();
        while i < b.len() {
            if s[i..].starts_with("$${") {
                text.push_str("${");
                i += 3;
            } else if s[i..].starts_with("${") {
                if !text.is_empty() {
                    parts.push(Part::Text(std::mem::take(&mut text)));
                }
                let mut p = Parser {
                    s,
                    pos: i + 2,
                    depth,
                };
                p.ws();
                if p.peek() == Some('}') {
                    return Err(ExprError::new("an empty ${}", i..p.pos + 1));
                }
                let e = p.expr()?;
                p.ws();
                if p.peek() != Some('}') {
                    return Err(match p.peek() {
                        None => ExprError::new("this ${ is never closed with }", i..s.len()),
                        Some(c) => {
                            ExprError::new(format!("unexpected {c:?}"), p.pos..p.pos + c.len_utf8())
                        }
                    });
                }
                parts.push(Part::Expr(e));
                i = p.pos + 1;
            } else {
                let c = s[i..].chars().next().expect("in bounds");
                text.push(c);
                i += c.len_utf8();
            }
        }
        if !text.is_empty() {
            parts.push(Part::Text(text));
        }
        Ok(Self {
            parts,
            literal: false,
        })
    }

    /// The string's value: the expression's own if the string is exactly one
    /// `${…}`, else a string.
    pub fn eval(&self, scope: &dyn Scope) -> R<Value> {
        Eval {
            scope,
            locals: Vec::new(),
        }
        .template(self)
    }
}

impl Eval<'_> {
    fn template(&mut self, t: &Template) -> R<Value> {
        if let ([Part::Expr(e)], false) = (t.parts.as_slice(), t.literal) {
            return self.node(e);
        }
        let mut out = String::new();
        let mut deferred = BTreeSet::new();
        for p in &t.parts {
            match p {
                Part::Text(t) => out.push_str(t),
                Part::Expr(e) => match self.node(e)? {
                    Value::Str(s) => out.push_str(&s),
                    Value::Int(i) => out.push_str(&i.to_string()),
                    Value::Bool(b) => out.push_str(&b.to_string()),
                    Value::Deferred(r) => deferred.extend(r),
                    v => {
                        return Err(ExprError {
                            message: format!("{} can't be part of a string", v.kind()),
                            at: e.at.clone(),
                            help: Some(
                                "use join() for a list, or make the whole string one ${…}".into(),
                            ),
                        })
                    }
                },
            }
        }
        Ok(if deferred.is_empty() {
            Value::Str(out)
        } else {
            Value::Deferred(deferred)
        })
    }
}

/// Evaluates a string: its value under the type rule above.
pub fn eval_str(s: &str, scope: &dyn Scope) -> R<Value> {
    Template::parse(s)?.eval(scope)
}

struct Parser<'a> {
    s: &'a str,
    pos: usize,
    depth: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.s[self.pos..].chars().next()
    }

    fn ws(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_whitespace() {
                self.pos += c.len_utf8();
            } else {
                break;
            }
        }
    }

    fn eat(&mut self, tok: &str) -> bool {
        self.ws();
        if self.s[self.pos..].starts_with(tok) {
            self.pos += tok.len();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, tok: &str) -> R<()> {
        if self.eat(tok) {
            Ok(())
        } else {
            let at = self.pos;
            Err(match self.peek() {
                None => ExprError::new(format!("expected {tok:?} at the end"), at..at),
                Some(c) => ExprError::new(
                    format!("expected {tok:?}, found {c:?}"),
                    at..at + c.len_utf8(),
                ),
            })
        }
    }

    /// A keyword or name at the cursor, without moving.
    fn word(&self) -> Option<&str> {
        let rest = &self.s[self.pos..];
        let mut end = 0;
        for (i, c) in rest.char_indices() {
            let ok = if i == 0 {
                c.is_ascii_alphabetic() || c == '_'
            } else {
                c.is_ascii_alphanumeric() || c == '_' || c == '-'
            };
            if !ok {
                break;
            }
            end = i + c.len_utf8();
        }
        // A name doesn't end with '-' (that's `a - b` written tightly).
        let w = rest[..end].trim_end_matches('-');
        (!w.is_empty()).then_some(w)
    }

    fn ident(&mut self) -> R<String> {
        self.ws();
        match self.word() {
            Some(w) => {
                let w = w.to_string();
                self.pos += w.len();
                Ok(w)
            }
            None => {
                let at = self.pos;
                Err(ExprError::new(
                    "expected a name",
                    at..at + self.peek().map_or(0, char::len_utf8),
                ))
            }
        }
    }

    fn node(&self, ex: Ex, start: usize) -> Node {
        Node {
            ex,
            at: start..self.pos,
        }
    }

    fn expr(&mut self) -> R<Node> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(ExprError::new(
                "this expression is nested too deeply",
                self.pos..self.pos,
            ));
        }
        self.ws();
        let start = self.pos;
        let c = self.or()?;
        let r = if self.eat("?") {
            let a = self.expr()?;
            self.expect(":")?;
            let b = self.expr()?;
            Ok(self.node(Ex::Cond(Box::new(c), Box::new(a), Box::new(b)), start))
        } else {
            Ok(c)
        };
        self.depth -= 1;
        r
    }

    fn binary(
        &mut self,
        ops: &[&'static str],
        next: fn(&mut Self) -> R<Node>,
        chain: bool,
    ) -> R<Node> {
        self.ws();
        let start = self.pos;
        let mut left = next(self)?;
        let mut n = 0;
        loop {
            n += 1;
            if n > MAX_DEPTH * 4 {
                return Err(ExprError::new(
                    "this expression is too long",
                    start..self.pos,
                ));
            }
            self.ws();
            let Some(op) = ops.iter().find(|op| {
                self.s[self.pos..].starts_with(**op)
                    // `<` is not `<=`, `=` is not `==`.
                    && !(op.len() == 1 && self.s[self.pos + 1..].starts_with('='))
            }) else {
                return Ok(left);
            };
            self.pos += op.len();
            let right = next(self)?;
            left = self.node(Ex::Binary(op, Box::new(left), Box::new(right)), start);
            if !chain {
                return Ok(left);
            }
        }
    }

    fn or(&mut self) -> R<Node> {
        self.binary(&["||"], Self::and, true)
    }
    fn and(&mut self) -> R<Node> {
        self.binary(&["&&"], Self::cmp, true)
    }
    fn cmp(&mut self) -> R<Node> {
        self.binary(&["==", "!=", "<=", ">=", "<", ">"], Self::sum, false)
    }
    fn sum(&mut self) -> R<Node> {
        self.binary(&["+", "-"], Self::prod, true)
    }
    fn prod(&mut self) -> R<Node> {
        self.binary(&["*", "/", "%"], Self::unary, true)
    }

    fn unary(&mut self) -> R<Node> {
        self.ws();
        let start = self.pos;
        for op in ['!', '-'] {
            if self.peek() == Some(op) && !(op == '!' && self.s[self.pos + 1..].starts_with('=')) {
                self.pos += 1;
                self.depth += 1;
                if self.depth > MAX_DEPTH {
                    return Err(ExprError::new(
                        "this expression is nested too deeply",
                        start..self.pos,
                    ));
                }
                let e = self.unary()?;
                self.depth -= 1;
                return Ok(self.node(Ex::Unary(op, Box::new(e)), start));
            }
        }
        self.post()
    }

    fn post(&mut self) -> R<Node> {
        self.ws();
        let start = self.pos;
        let mut e = self.prim()?;
        loop {
            if self.s[self.pos..].starts_with('.') {
                self.pos += 1;
                let name = self.ident()?;
                e = self.node(Ex::Attr(Box::new(e), name), start);
            } else if self.s[self.pos..].starts_with('[') {
                self.pos += 1;
                let i = self.expr()?;
                self.expect("]")?;
                e = self.node(Ex::Index(Box::new(e), Box::new(i)), start);
            } else {
                return Ok(e);
            }
        }
    }

    fn string(&mut self, quote: char) -> R<String> {
        let start = self.pos;
        self.pos += 1;
        let mut out = String::new();
        loop {
            let Some(c) = self.peek() else {
                return Err(ExprError::new(
                    "this string is never closed",
                    start..self.pos,
                ));
            };
            self.pos += c.len_utf8();
            match c {
                c if c == quote => return Ok(out),
                '\\' => {
                    let Some(e) = self.peek() else {
                        return Err(ExprError::new(
                            "this string is never closed",
                            start..self.pos,
                        ));
                    };
                    self.pos += e.len_utf8();
                    out.push(match e {
                        'n' => '\n',
                        't' => '\t',
                        '\\' | '"' | '\'' => e,
                        _ => {
                            return Err(ExprError::new(
                                format!("unknown escape \\{e}"),
                                self.pos - 1 - e.len_utf8()..self.pos,
                            ))
                        }
                    });
                }
                c => out.push(c),
            }
        }
    }

    fn list_of(&mut self, close: &str) -> R<Vec<Node>> {
        let mut items = Vec::new();
        if self.eat(close) {
            return Ok(items);
        }
        loop {
            items.push(self.expr()?);
            if self.eat(close) {
                return Ok(items);
            }
            self.expect(",")?;
            if self.eat(close) {
                return Ok(items);
            }
        }
    }

    fn prim(&mut self) -> R<Node> {
        self.ws();
        let start = self.pos;
        let Some(c) = self.peek() else {
            return Err(ExprError::new("expected a value", start..start));
        };
        let ex = match c {
            '0'..='9' => {
                let digits: String = self.s[self.pos..]
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect();
                self.pos += digits.len();
                let v: i64 = digits
                    .parse()
                    .map_err(|_| ExprError::new("this number is too large", start..self.pos))?;
                Ex::Lit(Value::Int(v))
            }
            '"' | '\'' => {
                let text = self.string(c)?;
                if has_expression(&text) {
                    // Interpolated like any string; positions inside it are
                    // the literal's.
                    let mut inner = Template::parse_at(&text, self.depth).map_err(|mut e| {
                        e.at = start..self.pos;
                        e
                    })?;
                    inner.literal = true;
                    Ex::Tmpl(inner)
                } else {
                    Ex::Lit(Value::Str(text))
                }
            }
            '(' => {
                self.pos += 1;
                let e = self.expr()?;
                self.expect(")")?;
                return Ok(Node {
                    ex: e.ex,
                    at: start..self.pos,
                });
            }
            '[' => {
                self.pos += 1;
                self.ws();
                if self.word() == Some("for") {
                    self.pos += 3;
                    let first = self.ident()?;
                    let (key, value) = if self.eat(",") {
                        (Some(first), self.ident()?)
                    } else {
                        (None, first)
                    };
                    self.ws();
                    if self.word() != Some("in") {
                        return Err(ExprError::new("expected `in`", self.pos..self.pos));
                    }
                    self.pos += 2;
                    let over = self.expr()?;
                    self.expect(":")?;
                    let body = self.expr()?;
                    self.ws();
                    let cond = if self.word() == Some("if") {
                        self.pos += 2;
                        Some(Box::new(self.expr()?))
                    } else {
                        None
                    };
                    self.expect("]")?;
                    Ex::For {
                        key,
                        value,
                        over: Box::new(over),
                        body: Box::new(body),
                        cond,
                    }
                } else {
                    Ex::List(self.list_of("]")?)
                }
            }
            '{' => {
                self.pos += 1;
                let mut entries = Vec::new();
                if !self.eat("}") {
                    loop {
                        self.ws();
                        let key = match self.peek() {
                            Some(q @ ('"' | '\'')) => self.string(q)?,
                            _ => self.ident()?,
                        };
                        self.expect("=")?;
                        entries.push((key, self.expr()?));
                        if self.eat("}") {
                            break;
                        }
                        self.expect(",")?;
                        if self.eat("}") {
                            break;
                        }
                    }
                }
                Ex::Map(entries)
            }
            _ => {
                let Some(w) = self.word().map(String::from) else {
                    return Err(ExprError::new(
                        format!("expected a value, found {c:?}"),
                        start..start + c.len_utf8(),
                    ));
                };
                self.pos += w.len();
                match w.as_str() {
                    "true" => Ex::Lit(Value::Bool(true)),
                    "false" => Ex::Lit(Value::Bool(false)),
                    "null" => Ex::Lit(Value::Null),
                    _ if self.s[self.pos..].starts_with('(') => {
                        self.pos += 1;
                        Ex::Call(w, self.list_of(")")?)
                    }
                    _ => Ex::Name(w),
                }
            }
        };
        Ok(self.node(ex, start))
    }
}

struct Eval<'a> {
    scope: &'a dyn Scope,
    /// Comprehension variables, innermost last.
    locals: Vec<(String, Value)>,
}

fn deferred_of(vals: &[&Value]) -> Option<Value> {
    let mut refs = BTreeSet::new();
    for v in vals {
        if let Value::Deferred(r) = v {
            refs.extend(r.iter().cloned());
        }
    }
    (!refs.is_empty()).then_some(Value::Deferred(refs))
}

impl Eval<'_> {
    fn node(&mut self, n: &Node) -> R<Value> {
        let err = |m: String| ExprError::new(m, n.at.clone());
        match &n.ex {
            Ex::Lit(v) => Ok(v.clone()),
            Ex::Tmpl(t) => self.template(t),
            Ex::Name(name) => {
                if let Some((_, v)) = self.locals.iter().rev().find(|(k, _)| k == name) {
                    return Ok(v.clone());
                }
                self.scope.get(name).ok_or_else(|| {
                    let mut names = self.scope.names();
                    names.extend(self.locals.iter().map(|(k, _)| k.clone()));
                    ExprError {
                        message: format!("unknown name `{name}`"),
                        at: n.at.clone(),
                        help: crate::did_you_mean(name, names.iter().map(String::as_str))
                            .map(|m| format!("did you mean `{m}`?")),
                    }
                })
            }
            Ex::Attr(on, name) => {
                let v = self.node(on)?;
                self.attr(v, name, &n.at)
            }
            Ex::Index(on, i) => {
                let v = self.node(on)?;
                let i = self.node(i)?;
                match (v, i) {
                    (Value::Deferred(r), Value::Int(i)) => {
                        Ok(Value::Deferred(extend_refs(r, &format!("[{i}]"))))
                    }
                    (Value::Deferred(r), Value::Str(k)) => {
                        Ok(Value::Deferred(extend_refs(r, &format!("[{k:?}]"))))
                    }
                    (v, i) if deferred_of(&[&v, &i]).is_some() => {
                        Ok(deferred_of(&[&v, &i]).expect("some"))
                    }
                    (Value::List(l), Value::Int(i)) => {
                        let len = l.len();
                        usize::try_from(i)
                            .ok()
                            .and_then(|i| l.into_iter().nth(i))
                            .ok_or_else(|| {
                                err(format!("index {i} is outside the list (it has {len})"))
                            })
                    }
                    (Value::Map(m), Value::Str(k)) => self.attr(Value::Map(m), &k, &n.at),
                    (v, i) => Err(err(format!("can't index {} with {}", v.kind(), i.kind()))),
                }
            }
            Ex::Call(f, args) => {
                let mut vals = Vec::with_capacity(args.len());
                for a in args {
                    vals.push(self.node(a)?);
                }
                if f != "coalesce" {
                    if let Some(d) = deferred_of(&vals.iter().collect::<Vec<_>>()) {
                        return Ok(d);
                    }
                }
                call(f, vals).map_err(|(m, help)| ExprError {
                    message: format!("{f}(): {m}"),
                    at: n.at.clone(),
                    help,
                })
            }
            Ex::List(items) => {
                let mut out = Vec::with_capacity(items.len());
                for i in items {
                    out.push(self.node(i)?);
                }
                Ok(deferred_of(&out.iter().collect::<Vec<_>>()).unwrap_or(Value::List(out)))
            }
            Ex::Map(entries) => {
                let mut out = BTreeMap::new();
                for (k, v) in entries {
                    out.insert(k.clone(), self.node(v)?);
                }
                Ok(deferred_of(&out.values().collect::<Vec<_>>()).unwrap_or(Value::Map(out)))
            }
            Ex::For {
                key,
                value,
                over,
                body,
                cond,
            } => {
                let items: Vec<(Value, Value)> = match self.node(over)? {
                    d @ Value::Deferred(_) => return Ok(d),
                    Value::List(l) => l
                        .into_iter()
                        .enumerate()
                        .map(|(i, v)| (Value::Int(i as i64), v))
                        .collect(),
                    Value::Map(m) => m.into_iter().map(|(k, v)| (Value::Str(k), v)).collect(),
                    v => {
                        return Err(err(format!(
                            "can't go through {}: give a list or a map",
                            v.kind()
                        )))
                    }
                };
                let mut out = Vec::new();
                for (k, v) in items {
                    let pushed = 1 + usize::from(key.is_some());
                    if let Some(kname) = key {
                        self.locals.push((kname.clone(), k));
                    }
                    self.locals.push((value.clone(), v));
                    let keep = match cond {
                        None => Ok(true),
                        Some(c) => match self.node(c) {
                            Ok(Value::Bool(b)) => Ok(b),
                            Ok(other) => Err(ExprError::new(
                                format!("`if` needs a boolean, not {}", other.kind()),
                                c.at.clone(),
                            )),
                            Err(e) => Err(e),
                        },
                    };
                    let item = match keep {
                        Ok(true) => self.node(body).map(Some),
                        Ok(false) => Ok(None),
                        Err(e) => Err(e),
                    };
                    for _ in 0..pushed {
                        self.locals.pop();
                    }
                    if let Some(i) = item? {
                        out.push(i);
                    }
                    if out.len() > MAX_LEN {
                        return Err(err(format!("more than {MAX_LEN} items")));
                    }
                }
                Ok(deferred_of(&out.iter().collect::<Vec<_>>()).unwrap_or(Value::List(out)))
            }
            Ex::Unary(op, e) => match (op, self.node(e)?) {
                (_, d @ Value::Deferred(_)) => Ok(d),
                ('!', Value::Bool(b)) => Ok(Value::Bool(!b)),
                ('-', Value::Int(i)) => i
                    .checked_neg()
                    .map(Value::Int)
                    .ok_or_else(|| err("the number overflows".into())),
                (op, v) => Err(err(format!("`{op}` doesn't apply to {}", v.kind()))),
            },
            Ex::Cond(c, a, b) => match self.node(c)? {
                d @ Value::Deferred(_) => Ok(d),
                Value::Bool(true) => self.node(a),
                Value::Bool(false) => self.node(b),
                v => Err(ExprError::new(
                    format!("`?` needs a boolean, not {}", v.kind()),
                    c.at.clone(),
                )),
            },
            Ex::Binary(op, a, b) => {
                let a = self.node(a)?;
                // Short circuits.
                match (*op, &a) {
                    ("&&", Value::Bool(false)) => return Ok(Value::Bool(false)),
                    ("||", Value::Bool(true)) => return Ok(Value::Bool(true)),
                    _ => {}
                }
                let b = self.node(b)?;
                if let Some(d) = deferred_of(&[&a, &b]) {
                    return Ok(d);
                }
                binary(op, a, b).map_err(err)
            }
        }
    }

    fn attr(&self, v: Value, name: &str, at: &Range<usize>) -> R<Value> {
        match v {
            Value::Deferred(r) => Ok(Value::Deferred(extend_refs(r, &format!(".{name}")))),
            Value::Map(mut m) => m.remove(name).ok_or_else(|| ExprError {
                message: format!("no `{name}` here"),
                at: at.clone(),
                help: crate::did_you_mean(name, m.keys().map(String::as_str))
                    .map(|k| format!("did you mean `{k}`?"))
                    .or_else(|| Some(format!("it has {}", list_keys(&m)))),
            }),
            v => Err(ExprError::new(
                format!("`.{name}`: {} has no fields", v.kind()),
                at.clone(),
            )),
        }
    }
}

fn list_keys(m: &BTreeMap<String, Value>) -> String {
    if m.is_empty() {
        "nothing".into()
    } else {
        m.keys()
            .map(|k| format!("`{k}`"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// `contract.settlement` + `.address`: a single reference grows; several stay.
fn extend_refs(r: BTreeSet<String>, tail: &str) -> BTreeSet<String> {
    if r.len() == 1 {
        r.into_iter().map(|p| format!("{p}{tail}")).collect()
    } else {
        r
    }
}

fn binary(op: &str, a: Value, b: Value) -> Result<Value, String> {
    use Value::*;
    let overflow = || "the number overflows".to_string();
    Ok(match (op, a, b) {
        ("+", Int(x), Int(y)) => Int(x.checked_add(y).ok_or_else(overflow)?),
        ("-", Int(x), Int(y)) => Int(x.checked_sub(y).ok_or_else(overflow)?),
        ("*", Int(x), Int(y)) => Int(x.checked_mul(y).ok_or_else(overflow)?),
        ("/" | "%", Int(_), Int(0)) => return Err("division by zero".into()),
        ("/", Int(x), Int(y)) => Int(x.checked_div(y).ok_or_else(overflow)?),
        ("%", Int(x), Int(y)) => Int(x.checked_rem(y).ok_or_else(overflow)?),
        ("+", Str(_), Str(_)) => {
            return Err("`+` adds numbers; join strings with \"${a}${b}\" or format()".into())
        }
        ("&&", Bool(x), Bool(y)) => Bool(x && y),
        ("||", Bool(x), Bool(y)) => Bool(x || y),
        ("==", a, b) => Bool(same(&a, &b)?),
        ("!=", a, b) => Bool(!same(&a, &b)?),
        (op @ ("<" | "<=" | ">" | ">="), a, b) => {
            let o = match (&a, &b) {
                (Int(x), Int(y)) => x.cmp(y),
                (Str(x), Str(y)) => x.cmp(y),
                _ => {
                    return Err(format!(
                        "`{op}` compares two numbers or two strings, not {} and {}",
                        a.kind(),
                        b.kind()
                    ))
                }
            };
            Bool(match op {
                "<" => o.is_lt(),
                "<=" => o.is_le(),
                ">" => o.is_gt(),
                _ => o.is_ge(),
            })
        }
        (op, a, b) => {
            return Err(format!(
                "`{op}` doesn't apply to {} and {}",
                a.kind(),
                b.kind()
            ))
        }
    })
}

/// `==`: values of one kind, or anything against null.
fn same(a: &Value, b: &Value) -> Result<bool, String> {
    use Value::*;
    match (a, b) {
        (Null, _) | (_, Null) => Ok(matches!((a, b), (Null, Null))),
        (Bool(_), Bool(_))
        | (Int(_), Int(_))
        | (Str(_), Str(_))
        | (List(_), List(_))
        | (Map(_), Map(_))
        | (Opaque(_), Opaque(_)) => Ok(a == b),
        _ => Err(format!(
            "`==` compares values of one kind, not {} and {}",
            a.kind(),
            b.kind()
        )),
    }
}

type CallErr = (String, Option<String>);

fn arity(args: &[Value], n: std::ops::RangeInclusive<usize>) -> Result<(), CallErr> {
    if n.contains(&args.len()) {
        Ok(())
    } else if n.start() == n.end() {
        Err((
            format!("takes {} argument(s), not {}", n.start(), args.len()),
            None,
        ))
    } else {
        Err((
            format!(
                "takes {} to {} arguments, not {}",
                n.start(),
                n.end(),
                args.len()
            ),
            None,
        ))
    }
}

fn int(v: &Value) -> Result<i64, CallErr> {
    match v {
        Value::Int(i) => Ok(*i),
        v => Err((format!("expected a number, got {}", v.kind()), None)),
    }
}

fn string(v: &Value) -> Result<&str, CallErr> {
    match v {
        Value::Str(s) => Ok(s),
        v => Err((format!("expected a string, got {}", v.kind()), None)),
    }
}

fn list(v: Value) -> Result<Vec<Value>, CallErr> {
    match v {
        Value::List(l) => Ok(l),
        v => Err((format!("expected a list, got {}", v.kind()), None)),
    }
}

fn map(v: Value) -> Result<BTreeMap<String, Value>, CallErr> {
    match v {
        Value::Map(m) => Ok(m),
        v => Err((format!("expected a map, got {}", v.kind()), None)),
    }
}

/// The functions, by name.
pub const FUNCTIONS: [&str; 18] = [
    "coalesce", "concat", "contains", "format", "join", "keys", "length", "lookup", "lower", "max",
    "merge", "min", "range", "replace", "split", "tonumber", "tostring", "upper",
];

fn call(f: &str, mut a: Vec<Value>) -> Result<Value, CallErr> {
    use Value::*;
    Ok(match f {
        "range" => {
            arity(&a, 1..=3)?;
            let (start, end, step) = match a.len() {
                1 => (0, int(&a[0])?, 1),
                2 => (int(&a[0])?, int(&a[1])?, 1),
                _ => (int(&a[0])?, int(&a[1])?, int(&a[2])?),
            };
            if step == 0 {
                return Err(("the step can't be 0".into(), None));
            }
            let mut out = Vec::new();
            let mut i = start;
            while (step > 0 && i < end) || (step < 0 && i > end) {
                out.push(Int(i));
                if out.len() > MAX_LEN {
                    return Err((format!("more than {MAX_LEN} numbers"), None));
                }
                i = match i.checked_add(step) {
                    Some(n) => n,
                    None => break,
                };
            }
            List(out)
        }
        "length" => {
            arity(&a, 1..=1)?;
            Int(match &a[0] {
                List(l) => l.len() as i64,
                Map(m) => m.len() as i64,
                Str(s) => s.chars().count() as i64,
                v => {
                    return Err((
                        format!("expected a list, map or string, got {}", v.kind()),
                        None,
                    ))
                }
            })
        }
        "concat" => {
            let mut out = Vec::new();
            for v in a {
                out.extend(list(v)?);
            }
            List(out)
        }
        "merge" => {
            let mut out = BTreeMap::new();
            for v in a {
                out.extend(map(v)?);
            }
            Map(out)
        }
        "lookup" => {
            arity(&a, 2..=3)?;
            let default = if a.len() == 3 { a.pop() } else { None };
            let key = string(&a[1])?.to_string();
            let mut m = map(a.swap_remove(0))?;
            match (m.remove(&key), default) {
                (Some(v), _) => v,
                (None, Some(d)) => d,
                (None, None) => {
                    return Err((
                        format!("no key {key:?}"),
                        Some("give a default as the third argument".into()),
                    ))
                }
            }
        }
        "keys" => {
            arity(&a, 1..=1)?;
            List(map(a.swap_remove(0))?.into_keys().map(Str).collect())
        }
        "contains" => {
            arity(&a, 2..=2)?;
            match (&a[0], &a[1]) {
                (List(l), v) => Bool(l.contains(v)),
                (Str(s), Str(sub)) => Bool(s.contains(sub.as_str())),
                (v, _) => {
                    return Err((
                        format!("expected a list or a string, got {}", v.kind()),
                        None,
                    ))
                }
            }
        }
        "join" => {
            arity(&a, 2..=2)?;
            let sep = string(&a[0])?.to_string();
            let parts: Result<Vec<String>, CallErr> = list(a.swap_remove(1))?
                .into_iter()
                .map(|v| match v {
                    Str(s) => Ok(s),
                    Int(i) => Ok(i.to_string()),
                    Bool(b) => Ok(b.to_string()),
                    v => Err((format!("can't join {}", v.kind()), None)),
                })
                .collect();
            Str(parts?.join(&sep))
        }
        "split" => {
            arity(&a, 2..=2)?;
            let sep = string(&a[0])?;
            if sep.is_empty() {
                return Err(("the separator can't be empty".into(), None));
            }
            List(
                string(&a[1])?
                    .split(sep)
                    .map(|s| Str(s.to_string()))
                    .collect(),
            )
        }
        "replace" => {
            arity(&a, 3..=3)?;
            let from = string(&a[1])?;
            if from.is_empty() {
                return Err(("what to replace can't be empty".into(), None));
            }
            Str(string(&a[0])?.replace(from, string(&a[2])?))
        }
        "upper" => {
            arity(&a, 1..=1)?;
            Str(string(&a[0])?.to_uppercase())
        }
        "lower" => {
            arity(&a, 1..=1)?;
            Str(string(&a[0])?.to_lowercase())
        }
        "tostring" => {
            arity(&a, 1..=1)?;
            Str(match &a[0] {
                Str(s) => s.clone(),
                Int(i) => i.to_string(),
                Bool(b) => b.to_string(),
                v => return Err((format!("can't turn {} into a string", v.kind()), None)),
            })
        }
        "tonumber" => {
            arity(&a, 1..=1)?;
            match &a[0] {
                Int(i) => Int(*i),
                Str(s) => Int(s
                    .trim()
                    .parse()
                    .map_err(|_| (format!("{s:?} is not a whole number"), None))?),
                v => return Err((format!("can't turn {} into a number", v.kind()), None)),
            }
        }
        "min" | "max" => {
            let nums: Vec<Value> = if a.len() == 1 && matches!(a[0], List(_)) {
                list(a.swap_remove(0))?
            } else {
                a
            };
            if nums.is_empty() {
                return Err(("needs at least one number".into(), None));
            }
            let mut ns = Vec::with_capacity(nums.len());
            for n in &nums {
                ns.push(int(n)?);
            }
            Int(if f == "min" {
                ns.into_iter().min()
            } else {
                ns.into_iter().max()
            }
            .expect("not empty"))
        }
        "coalesce" => {
            // The first that isn't null; a value known later waits.
            for v in a {
                match v {
                    Null => continue,
                    v => return Ok(v),
                }
            }
            Null
        }
        "format" => {
            if a.is_empty() {
                return Err(("needs a format string".into(), None));
            }
            let fmt = string(&a[0])?.to_string();
            let mut args = a.into_iter().skip(1);
            let mut out = String::new();
            let mut chars = fmt.chars().peekable();
            while let Some(c) = chars.next() {
                match (c, chars.peek()) {
                    ('{', Some('{')) | ('}', Some('}')) => {
                        out.push(c);
                        chars.next();
                    }
                    ('{', Some('}')) => {
                        chars.next();
                        match args.next() {
                            Some(Str(s)) => out.push_str(&s),
                            Some(Int(i)) => out.push_str(&i.to_string()),
                            Some(Bool(b)) => out.push_str(&b.to_string()),
                            Some(v) => return Err((format!("can't format {}", v.kind()), None)),
                            None => return Err(("more {} than arguments".into(), None)),
                        }
                    }
                    ('{' | '}', _) => return Err(("a lone { or }: write {{ or }}".into(), None)),
                    (c, _) => out.push(c),
                }
            }
            if args.next().is_some() {
                return Err(("more arguments than {}".into(), None));
            }
            Str(out)
        }
        other => {
            return Err((
                format!("unknown function `{other}`"),
                crate::did_you_mean(other, FUNCTIONS).map(|m| format!("did you mean `{m}`?")),
            ))
        }
    })
}

#[cfg(test)]
mod tests;
