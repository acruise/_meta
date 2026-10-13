//! JSONPath (RFC 9535) to `LogExpr` translation.
//!
//! A JSONPath query denotes a *nodelist*: the sequence of values it selects
//! from a document. The translation produces a `LogExpr` of type
//! `list(dyn)` whose elements are exactly those values, in RFC order, built
//! from the total-over-`dyn` collection operators in the function catalog
//! (`get_member_by_key`, `get_element_by_index`, `list_elements`,
//! `map_values`, `descendants`, `list_slice`, `list_concat`, `flatten`).
//!
//! Variables, all read with `GetFieldByName`:
//!
//! - [`ROOT`] (`$`) is the document root; the evaluator binds it.
//! - `{name}` placeholders (an extension, not RFC 9535) are parameters bound
//!   at evaluation time. They may appear wherever a value may (comparison
//!   operands, function arguments) and as a bracketed selector, where they
//!   select an array element or an object member by the parameter's value.
//!   They are reported in [`JsonPathTranslation::params`].
//! - Iteration and filter variables get generated names `$1`, `$2`, ...,
//!   which cannot collide with a parameter (parameters are identifiers) or
//!   with the root.
//!
//! No casts are inserted: a parameter keeps its own type, and adapting it to
//! a position (an integer parameter used as an object member name, say) is
//! left to the type checker's auto-cast pass, which knows the parameter's
//! declared type and the document's, if declared.
//!
//! Filter-expression semantics follow the RFC rather than SQL-style null
//! propagation. Each comparison operand is an optional value, represented as
//! a 0-or-1-element list: absent (`Nothing` in the RFC) is the empty list,
//! a present `null` is a list holding null. Equality is true when both are
//! absent, or both present and null-safe-equal. Ordering is true only when
//! both are present and ordered (`LessThan` yields null, hence false, for
//! values that are not both numbers or both strings).
//!
//! Known deviation:
//! - A regex given as a parameter or query (not a string literal) is passed
//!   to the regex engine as written; a literal is translated from I-Regexp
//!   (`.` excludes CR and LF, and `match()` anchors the whole string).

use crate::expr_gen::LogExpr;
use crate::value::Value;

/// Variable name the document root is read from.
pub const ROOT: &str = "$";

/// The result of translating a JSONPath query.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonPathTranslation {
    /// An expression of type `list(dyn)`: the selected nodelist.
    pub expr: LogExpr,
    /// Parameter names referenced by `{name}` placeholders, in order of first
    /// reference.
    pub params: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonPathError {
    /// Byte offset into the query where the error was detected.
    pub position: usize,
    pub message: String,
}

impl std::fmt::Display for JsonPathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid JSONPath at offset {}: {}", self.position, self.message)
    }
}

impl std::error::Error for JsonPathError {}

/// Translate a JSONPath query (RFC 9535, plus `{param}` placeholders).
pub fn jsonpath_to_log_expr(src: &str) -> Result<JsonPathTranslation, JsonPathError> {
    let query = Parser { src, pos: 0 }.parse_query_document()?;
    let mut t = Translator { next_var: 1, params: Vec::new(), current: Vec::new() };
    let expr = crate::bindings::canonicalize_bindings(&t.query(&query));
    Ok(JsonPathTranslation { expr, params: t.params })
}

// ---------------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Query {
    relative: bool,
    segments: Vec<Segment>,
}

impl Query {
    /// RFC 9535 singular query: only child segments, each a single name or
    /// index selector.
    fn is_singular(&self) -> bool {
        self.segments.iter().all(|s| match s {
            Segment::Child(sels) => {
                sels.len() == 1 && matches!(sels[0], Selector::Name(_) | Selector::Index(_))
            }
            Segment::Descendant(_) => false,
        })
    }
}

#[derive(Debug, Clone)]
enum Segment {
    Child(Vec<Selector>),
    Descendant(Vec<Selector>),
}

#[derive(Debug, Clone)]
enum Selector {
    Name(String),
    Wildcard,
    Index(i64),
    Slice(Option<i64>, Option<i64>, Option<i64>),
    Filter(Box<Logical>),
    Param(String),
}

#[derive(Debug, Clone)]
enum Logical {
    Or(Vec<Logical>),
    And(Vec<Logical>),
    Not(Box<Logical>),
    Compare(Operand, CmpOp, Operand),
    /// Existence test on a query.
    Exists(Query),
    /// A function returning LogicalType (`match`, `search`).
    Call(Call),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// A value-typed operand: something that is either a value or Nothing.
#[derive(Debug, Clone)]
enum Operand {
    Literal(Value),
    Param(String),
    /// Must be singular when used as a value.
    Query(Query),
    /// A function returning ValueType (`length`, `count`, `value`).
    Call(Call),
}

#[derive(Debug, Clone)]
struct Call {
    name: Func,
    args: Vec<Operand>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Func {
    Length,
    Count,
    Match,
    Search,
    Value,
}

#[derive(Clone, Copy, PartialEq)]
enum ArgKind {
    /// ValueType: literal, parameter, singular query, or value-returning call.
    Value,
    /// NodesType: any query.
    Nodes,
}

impl Func {
    fn lookup(name: &str) -> Option<Func> {
        Some(match name {
            "length" => Func::Length,
            "count" => Func::Count,
            "match" => Func::Match,
            "search" => Func::Search,
            "value" => Func::Value,
            _ => return None,
        })
    }

    fn params(self) -> &'static [ArgKind] {
        match self {
            Func::Length => &[ArgKind::Value],
            Func::Count | Func::Value => &[ArgKind::Nodes],
            Func::Match | Func::Search => &[ArgKind::Value, ArgKind::Value],
        }
    }

    /// LogicalType result (usable as a test) vs ValueType (usable as a value).
    fn is_logical(self) -> bool {
        matches!(self, Func::Match | Func::Search)
    }
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

const MAX_INT: i64 = (1 << 53) - 1;

struct Parser<'a> {
    src: &'a str,
    pos: usize,
}

type PResult<T> = Result<T, JsonPathError>;

impl<'a> Parser<'a> {
    fn err<T>(&self, message: impl Into<String>) -> PResult<T> {
        Err(JsonPathError { position: self.pos, message: message.into() })
    }

    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    fn eat(&mut self, s: &str) -> bool {
        if self.rest().starts_with(s) {
            self.pos += s.len();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, s: &str) -> PResult<()> {
        if self.eat(s) { Ok(()) } else { self.err(format!("expected '{s}'")) }
    }

    /// RFC 9535 blank space: space, tab, LF, CR.
    fn skip_blank(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
            self.pos += 1;
        }
    }

    fn parse_query_document(mut self) -> PResult<Query> {
        if !self.eat("$") {
            return self.err("query must start with '$'");
        }
        let segments = self.parse_segments()?;
        if self.pos != self.src.len() {
            return self.err("unexpected trailing input");
        }
        Ok(Query { relative: false, segments })
    }

    /// `*(S segment)`. Blank space is consumed only if a segment follows, so
    /// trailing blanks remain for the caller to judge.
    fn parse_segments(&mut self) -> PResult<Vec<Segment>> {
        let mut segments = Vec::new();
        loop {
            let save = self.pos;
            self.skip_blank();
            match self.peek() {
                Some('[') | Some('.') => segments.push(self.parse_segment()?),
                _ => {
                    self.pos = save;
                    return Ok(segments);
                }
            }
        }
    }

    fn parse_segment(&mut self) -> PResult<Segment> {
        if self.eat("..") {
            let sels = match self.peek() {
                Some('[') => self.parse_bracketed()?,
                Some('*') => {
                    self.bump();
                    vec![Selector::Wildcard]
                }
                _ => vec![Selector::Name(self.parse_member_name()?)],
            };
            return Ok(Segment::Descendant(sels));
        }
        if self.eat(".") {
            let sel = if self.eat("*") {
                Selector::Wildcard
            } else {
                Selector::Name(self.parse_member_name()?)
            };
            return Ok(Segment::Child(vec![sel]));
        }
        Ok(Segment::Child(self.parse_bracketed()?))
    }

    fn parse_member_name(&mut self) -> PResult<String> {
        let start = self.pos;
        match self.peek() {
            Some(c) if is_name_first(c) => {
                self.bump();
            }
            _ => return self.err("expected member name"),
        }
        while matches!(self.peek(), Some(c) if is_name_first(c) || c.is_ascii_digit()) {
            self.bump();
        }
        Ok(self.src[start..self.pos].to_string())
    }

    fn parse_bracketed(&mut self) -> PResult<Vec<Selector>> {
        self.expect("[")?;
        let mut sels = Vec::new();
        loop {
            self.skip_blank();
            sels.push(self.parse_selector()?);
            self.skip_blank();
            if self.eat(",") {
                continue;
            }
            self.expect("]")?;
            return Ok(sels);
        }
    }

    fn parse_selector(&mut self) -> PResult<Selector> {
        match self.peek() {
            Some('\'') | Some('"') => Ok(Selector::Name(self.parse_string()?)),
            Some('*') => {
                self.bump();
                Ok(Selector::Wildcard)
            }
            Some('?') => {
                self.bump();
                self.skip_blank();
                Ok(Selector::Filter(Box::new(self.parse_or()?)))
            }
            Some('{') => Ok(Selector::Param(self.parse_param()?)),
            Some(c) if c == ':' || c == '-' || c.is_ascii_digit() => self.parse_index_or_slice(),
            _ => self.err("expected selector"),
        }
    }

    fn parse_index_or_slice(&mut self) -> PResult<Selector> {
        let start = self.parse_opt_int()?;
        self.skip_blank();
        if !self.eat(":") {
            return match start {
                Some(i) => Ok(Selector::Index(i)),
                None => self.err("expected index or slice"),
            };
        }
        self.skip_blank();
        let end = self.parse_opt_int()?;
        self.skip_blank();
        let step = if self.eat(":") {
            self.skip_blank();
            self.parse_opt_int()?
        } else {
            None
        };
        Ok(Selector::Slice(start, end, step))
    }

    fn parse_opt_int(&mut self) -> PResult<Option<i64>> {
        match self.peek() {
            Some(c) if c == '-' || c.is_ascii_digit() => self.parse_int().map(Some),
            _ => Ok(None),
        }
    }

    /// RFC 9535 `int`: "0" / ["-"] DIGIT1 *DIGIT, within +/-(2^53-1).
    fn parse_int(&mut self) -> PResult<i64> {
        let start = self.pos;
        let neg = self.eat("-");
        let digits_start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.bump();
        }
        let digits = &self.src[digits_start..self.pos];
        if digits.is_empty() {
            return self.err("expected integer");
        }
        if digits.len() > 1 && digits.starts_with('0') {
            self.pos = start;
            return self.err("leading zero in integer");
        }
        if neg && digits == "0" {
            self.pos = start;
            return self.err("'-0' is not a valid integer");
        }
        match self.src[start..self.pos].parse::<i64>() {
            Ok(n) if (-MAX_INT..=MAX_INT).contains(&n) => Ok(n),
            _ => {
                self.pos = start;
                self.err("integer out of range")
            }
        }
    }

    fn parse_param(&mut self) -> PResult<String> {
        self.expect("{")?;
        let start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_alphanumeric() || c == '_') {
            self.bump();
        }
        if self.pos == start {
            return self.err("expected parameter name");
        }
        let name = self.src[start..self.pos].to_string();
        self.expect("}")?;
        Ok(name)
    }

    fn parse_string(&mut self) -> PResult<String> {
        let quote = self.bump().unwrap();
        let mut out = String::new();
        loop {
            let c = match self.bump() {
                Some(c) => c,
                None => return self.err("unterminated string literal"),
            };
            if c == quote {
                return Ok(out);
            }
            match c {
                '\\' => out.push(self.parse_escape(quote)?),
                c if (c as u32) < 0x20 => return self.err("unescaped control character in string"),
                c => out.push(c),
            }
        }
    }

    fn parse_escape(&mut self, quote: char) -> PResult<char> {
        let c = match self.bump() {
            Some(c) => c,
            None => return self.err("unterminated escape"),
        };
        Ok(match c {
            'b' => '\u{8}',
            'f' => '\u{c}',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            '/' => '/',
            '\\' => '\\',
            'u' => return self.parse_unicode_escape(),
            c if c == quote => c,
            _ => return self.err(format!("invalid escape '\\{c}'")),
        })
    }

    fn parse_hex4(&mut self) -> PResult<u32> {
        let s = self.rest().get(..4).unwrap_or("");
        if s.len() != 4 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
            return self.err("expected four hex digits");
        }
        self.pos += 4;
        Ok(u32::from_str_radix(s, 16).unwrap())
    }

    fn parse_unicode_escape(&mut self) -> PResult<char> {
        let hi = self.parse_hex4()?;
        let code = match hi {
            0xD800..=0xDBFF => {
                if !self.eat("\\u") {
                    return self.err("high surrogate not followed by a low surrogate");
                }
                let lo = self.parse_hex4()?;
                if !(0xDC00..=0xDFFF).contains(&lo) {
                    return self.err("high surrogate not followed by a low surrogate");
                }
                0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
            }
            0xDC00..=0xDFFF => return self.err("unpaired low surrogate"),
            c => c,
        };
        Ok(char::from_u32(code).unwrap())
    }

    // --- filter expressions -------------------------------------------------

    fn parse_or(&mut self) -> PResult<Logical> {
        let mut terms = vec![self.parse_and()?];
        loop {
            let save = self.pos;
            self.skip_blank();
            if self.eat("||") {
                self.skip_blank();
                terms.push(self.parse_and()?);
            } else {
                self.pos = save;
                break;
            }
        }
        Ok(if terms.len() == 1 { terms.pop().unwrap() } else { Logical::Or(terms) })
    }

    fn parse_and(&mut self) -> PResult<Logical> {
        let mut terms = vec![self.parse_basic()?];
        loop {
            let save = self.pos;
            self.skip_blank();
            if self.eat("&&") {
                self.skip_blank();
                terms.push(self.parse_basic()?);
            } else {
                self.pos = save;
                break;
            }
        }
        Ok(if terms.len() == 1 { terms.pop().unwrap() } else { Logical::And(terms) })
    }

    fn parse_basic(&mut self) -> PResult<Logical> {
        if self.eat("!") {
            self.skip_blank();
            if self.peek() == Some('(') {
                return Ok(Logical::Not(Box::new(self.parse_paren()?)));
            }
            let start = self.pos;
            let operand = self.parse_operand()?;
            return Ok(Logical::Not(Box::new(self.as_test(operand, start)?)));
        }
        if self.peek() == Some('(') {
            return self.parse_paren();
        }
        let start = self.pos;
        let lhs = self.parse_operand()?;
        let save = self.pos;
        self.skip_blank();
        if let Some(op) = self.parse_cmp_op() {
            self.skip_blank();
            let rhs_start = self.pos;
            let rhs = self.parse_operand()?;
            self.check_comparable(&lhs, start)?;
            self.check_comparable(&rhs, rhs_start)?;
            return Ok(Logical::Compare(lhs, op, rhs));
        }
        self.pos = save;
        self.as_test(lhs, start)
    }

    fn parse_paren(&mut self) -> PResult<Logical> {
        self.expect("(")?;
        self.skip_blank();
        let inner = self.parse_or()?;
        self.skip_blank();
        self.expect(")")?;
        Ok(inner)
    }

    fn parse_cmp_op(&mut self) -> Option<CmpOp> {
        for (s, op) in [
            ("==", CmpOp::Eq),
            ("!=", CmpOp::Ne),
            ("<=", CmpOp::Le),
            (">=", CmpOp::Ge),
            ("<", CmpOp::Lt),
            (">", CmpOp::Gt),
        ] {
            if self.eat(s) {
                return Some(op);
            }
        }
        None
    }

    /// An operand in test position must be a query (existence) or a
    /// LogicalType function.
    fn as_test(&self, operand: Operand, start: usize) -> PResult<Logical> {
        match operand {
            Operand::Query(q) => Ok(Logical::Exists(q)),
            Operand::Call(c) if c.name.is_logical() => Ok(Logical::Call(c)),
            _ => Err(JsonPathError {
                position: start,
                message: "a test expression must be a query or a logical function".into(),
            }),
        }
    }

    /// A comparison operand must denote a single value: a literal, a
    /// parameter, a singular query, or a ValueType function.
    fn check_comparable(&self, operand: &Operand, start: usize) -> PResult<()> {
        let ok = match operand {
            Operand::Literal(_) | Operand::Param(_) => true,
            Operand::Query(q) => q.is_singular(),
            Operand::Call(c) => !c.name.is_logical(),
        };
        if ok {
            Ok(())
        } else {
            Err(JsonPathError {
                position: start,
                message: "comparison operand must be a literal, singular query, or value function".into(),
            })
        }
    }

    fn parse_operand(&mut self) -> PResult<Operand> {
        match self.peek() {
            Some('@') => {
                self.bump();
                Ok(Operand::Query(Query { relative: true, segments: self.parse_segments()? }))
            }
            Some('$') => {
                self.bump();
                Ok(Operand::Query(Query { relative: false, segments: self.parse_segments()? }))
            }
            Some('{') => Ok(Operand::Param(self.parse_param()?)),
            Some('\'') | Some('"') => Ok(Operand::Literal(Value::String(self.parse_string()?))),
            Some(c) if c == '-' || c.is_ascii_digit() => Ok(Operand::Literal(self.parse_number()?)),
            Some(c) if c.is_ascii_lowercase() => self.parse_word(),
            _ => self.err("expected a query, literal, or function"),
        }
    }

    /// `true`, `false`, `null`, or a function call.
    fn parse_word(&mut self) -> PResult<Operand> {
        let start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
            self.bump();
        }
        let word = &self.src[start..self.pos];
        if self.peek() == Some('(') {
            return self.parse_call(word, start).map(Operand::Call);
        }
        match word {
            "true" => Ok(Operand::Literal(Value::Bool(true))),
            "false" => Ok(Operand::Literal(Value::Bool(false))),
            "null" => Ok(Operand::Literal(Value::Null)),
            _ => {
                self.pos = start;
                self.err(format!("unexpected '{word}'"))
            }
        }
    }

    fn parse_call(&mut self, word: &str, start: usize) -> PResult<Call> {
        let name = match Func::lookup(word) {
            Some(f) => f,
            None => {
                self.pos = start;
                return self.err(format!("unknown function '{word}'"));
            }
        };
        self.expect("(")?;
        let mut args = Vec::new();
        self.skip_blank();
        if !self.eat(")") {
            loop {
                let arg_start = self.pos;
                let arg = self.parse_operand()?;
                // A logical expression (comparison or connective) as an
                // argument: no standard function takes LogicalType.
                let save = self.pos;
                self.skip_blank();
                if self.parse_cmp_op().is_some() || self.eat("&&") || self.eat("||") {
                    self.pos = arg_start;
                    return self.err("logical expression is not a valid function argument here");
                }
                self.pos = save;
                args.push((arg, arg_start));
                self.skip_blank();
                if self.eat(",") {
                    self.skip_blank();
                    continue;
                }
                self.expect(")")?;
                break;
            }
        }
        let kinds = name.params();
        if args.len() != kinds.len() {
            self.pos = start;
            return self.err(format!("{word}() takes {} argument(s), got {}", kinds.len(), args.len()));
        }
        let mut checked = Vec::new();
        for ((arg, at), kind) in args.into_iter().zip(kinds) {
            let ok = match (kind, &arg) {
                (ArgKind::Value, Operand::Literal(_) | Operand::Param(_)) => true,
                (ArgKind::Value, Operand::Query(q)) => q.is_singular(),
                (ArgKind::Value, Operand::Call(c)) => !c.name.is_logical(),
                (ArgKind::Nodes, Operand::Query(_)) => true,
                (ArgKind::Nodes, _) => false,
            };
            if !ok {
                return Err(JsonPathError {
                    position: at,
                    message: format!("argument to {word}() has the wrong type"),
                });
            }
            checked.push(arg);
        }
        Ok(Call { name, args: checked })
    }

    /// RFC 9535 `number`: (int / "-0") [frac] [exp].
    fn parse_number(&mut self) -> PResult<Value> {
        let start = self.pos;
        self.eat("-");
        let int_start = self.pos;
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.bump();
        }
        let int_digits = &self.src[int_start..self.pos];
        if int_digits.is_empty() || (int_digits.len() > 1 && int_digits.starts_with('0')) {
            self.pos = start;
            return self.err("invalid number");
        }
        let mut is_float = false;
        if self.peek() == Some('.') {
            self.bump();
            let frac_start = self.pos;
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.bump();
            }
            if self.pos == frac_start {
                return self.err("expected digits after '.'");
            }
            is_float = true;
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            self.bump();
            if matches!(self.peek(), Some('+' | '-')) {
                self.bump();
            }
            let exp_start = self.pos;
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.bump();
            }
            if self.pos == exp_start {
                return self.err("expected exponent digits");
            }
            is_float = true;
        }
        let text = &self.src[start..self.pos];
        if !is_float {
            if let Ok(n) = text.parse::<i64>() {
                return Ok(Value::I64(n));
            }
        }
        match text.parse::<f64>() {
            Ok(x) => Ok(Value::F64(x)),
            Err(_) => {
                self.pos = start;
                self.err("invalid number")
            }
        }
    }
}

fn is_name_first(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || (c as u32) >= 0x80
}

// ---------------------------------------------------------------------------
// Translation
// ---------------------------------------------------------------------------

fn b(e: LogExpr) -> Box<LogExpr> {
    Box::new(e)
}

fn var(name: &str) -> LogExpr {
    LogExpr::GetFieldByName { field_name: name.to_string() }
}

fn lit(v: Value) -> LogExpr {
    LogExpr::Literal(v)
}

fn empty() -> LogExpr {
    lit(Value::Array(vec![]))
}

fn int(n: i64) -> LogExpr {
    lit(Value::I64(n))
}

fn singleton(e: LogExpr) -> LogExpr {
    LogExpr::ListAppend { lhs: b(empty()), rhs: b(e) }
}

fn concat_all(mut parts: Vec<LogExpr>) -> LogExpr {
    if parts.is_empty() {
        return empty();
    }
    let first = parts.remove(0);
    parts.into_iter().fold(first, |acc, p| LogExpr::ListConcat { lhs: b(acc), rhs: b(p) })
}

fn size(e: LogExpr) -> LogExpr {
    LogExpr::Size { operand: b(e) }
}

fn size_is(e: LogExpr, n: i64) -> LogExpr {
    LogExpr::Equal { lhs: b(size(e)), rhs: b(int(n)) }
}

fn first(e: LogExpr) -> LogExpr {
    LogExpr::Index { lhs: b(e), rhs: b(int(0)) }
}

fn and(a: LogExpr, c: LogExpr) -> LogExpr {
    LogExpr::LogicalAnd { lhs: b(a), rhs: b(c) }
}

fn or(a: LogExpr, c: LogExpr) -> LogExpr {
    LogExpr::LogicalOr { lhs: b(a), rhs: b(c) }
}

fn not(a: LogExpr) -> LogExpr {
    LogExpr::LogicalNot { operand: b(a) }
}

/// An operand reduced to what the comparison needs: either a value that is
/// certainly present (no list wrapper needed), or an optional value as a
/// 0-or-1-element list.
enum Opt {
    Present(LogExpr),
    Maybe(LogExpr),
}

impl Opt {
    /// The condition that the operand is present, if not certain.
    fn present_cond(&self) -> Option<LogExpr> {
        match self {
            Opt::Present(_) => None,
            Opt::Maybe(l) => Some(size_is(l.clone(), 1)),
        }
    }

    fn value(&self) -> LogExpr {
        match self {
            Opt::Present(v) => v.clone(),
            Opt::Maybe(l) => first(l.clone()),
        }
    }
}

fn conj(conds: impl IntoIterator<Item = Option<LogExpr>>, last: LogExpr) -> LogExpr {
    let mut conds: Vec<LogExpr> = conds.into_iter().flatten().collect();
    conds.push(last);
    let first = conds.remove(0);
    conds.into_iter().fold(first, and)
}

struct Translator {
    next_var: usize,
    params: Vec<String>,
    /// Stack of `@` bindings, innermost last.
    current: Vec<String>,
}

impl Translator {
    fn fresh(&mut self) -> String {
        let name = format!("${}", self.next_var);
        self.next_var += 1;
        name
    }

    fn param(&mut self, name: &str) -> LogExpr {
        if !self.params.iter().any(|p| p == name) {
            self.params.push(name.to_string());
        }
        var(name)
    }

    /// The nodelist a query selects.
    fn query(&mut self, q: &Query) -> LogExpr {
        let start = if q.relative {
            var(self.current.last().expect("relative query outside a filter"))
        } else {
            var(ROOT)
        };
        // Track whether the current nodelist is a known singleton, to apply
        // the next segment directly instead of mapping over a one-element
        // list.
        let mut node: Option<LogExpr> = Some(start);
        let mut list: Option<LogExpr> = None;
        for seg in &q.segments {
            let next = match node.take() {
                Some(x) => self.segment(&x, seg),
                None => {
                    let v = self.fresh();
                    let body = self.segment(&var(&v), seg);
                    LogExpr::Flatten {
                        operand: b(LogExpr::MapTransform {
                            collection: b(list.take().unwrap()),
                            binding: v,
                            body: b(body),
                        }),
                    }
                }
            };
            list = Some(next);
        }
        match node {
            Some(x) => singleton(x),
            None => list.unwrap(),
        }
    }

    /// The nodelist one segment selects from one node.
    fn segment(&mut self, x: &LogExpr, seg: &Segment) -> LogExpr {
        match seg {
            Segment::Child(sels) => self.selectors(x, sels),
            Segment::Descendant(sels) => {
                let d = self.fresh();
                let body = self.selectors(&var(&d), sels);
                LogExpr::Flatten {
                    operand: b(LogExpr::MapTransform {
                        collection: b(LogExpr::Descendants { operand: b(x.clone()) }),
                        binding: d,
                        body: b(body),
                    }),
                }
            }
        }
    }

    fn selectors(&mut self, x: &LogExpr, sels: &[Selector]) -> LogExpr {
        let parts = sels.iter().map(|s| self.selector(x, s)).collect();
        concat_all(parts)
    }

    fn children(x: &LogExpr) -> LogExpr {
        LogExpr::ListConcat {
            lhs: b(LogExpr::ListElements { operand: b(x.clone()) }),
            rhs: b(LogExpr::MapValues { operand: b(x.clone()) }),
        }
    }

    fn selector(&mut self, x: &LogExpr, sel: &Selector) -> LogExpr {
        match sel {
            Selector::Name(k) => LogExpr::GetMemberByKey { lhs: b(x.clone()), rhs: b(lit(Value::String(k.clone()))) },
            Selector::Index(i) => LogExpr::GetElementByIndex { lhs: b(x.clone()), rhs: b(int(*i)) },
            Selector::Wildcard => Self::children(x),
            Selector::Slice(start, end, step) => {
                let opt = |v: &Option<i64>| v.map(int).unwrap_or(lit(Value::Null));
                LogExpr::ListSlice {
                    collection: b(LogExpr::ListElements { operand: b(x.clone()) }),
                    start: b(opt(start)),
                    end: b(opt(end)),
                    step: b(opt(step)),
                }
            }
            Selector::Param(p) => {
                // Element of an array or member of an object, whichever the
                // node is; at most one side is non-empty.
                let pv = self.param(p);
                LogExpr::ListConcat {
                    lhs: b(LogExpr::GetElementByIndex { lhs: b(x.clone()), rhs: b(pv.clone()) }),
                    rhs: b(LogExpr::GetMemberByKey { lhs: b(x.clone()), rhs: b(pv) }),
                }
            }
            Selector::Filter(cond) => {
                let at = self.fresh();
                self.current.push(at.clone());
                let body = self.logical(cond);
                self.current.pop();
                LogExpr::Filter { collection: b(Self::children(x)), binding: at, body: b(body) }
            }
        }
    }

    fn logical(&mut self, l: &Logical) -> LogExpr {
        match l {
            Logical::Or(ts) => {
                let mut it = ts.iter().map(|t| self.logical(t)).collect::<Vec<_>>().into_iter();
                let first = it.next().unwrap();
                it.fold(first, or)
            }
            Logical::And(ts) => {
                let mut it = ts.iter().map(|t| self.logical(t)).collect::<Vec<_>>().into_iter();
                let first = it.next().unwrap();
                it.fold(first, and)
            }
            Logical::Not(inner) => not(self.logical(inner)),
            Logical::Exists(q) => {
                let nodes = self.query(q);
                LogExpr::GreaterThan { lhs: b(size(nodes)), rhs: b(int(0)) }
            }
            Logical::Call(c) => self.logical_call(c),
            Logical::Compare(lhs, op, rhs) => {
                let a = self.opt(lhs);
                let c = self.opt(rhs);
                match op {
                    CmpOp::Eq => Self::eq(&a, &c),
                    CmpOp::Ne => not(Self::eq(&a, &c)),
                    CmpOp::Lt => Self::lt(&a, &c),
                    CmpOp::Gt => Self::lt(&c, &a),
                    CmpOp::Le => or(Self::lt(&a, &c), Self::eq(&a, &c)),
                    CmpOp::Ge => or(Self::lt(&c, &a), Self::eq(&a, &c)),
                }
            }
        }
    }

    /// Equal: both absent, or both present and null-safe-equal.
    fn eq(a: &Opt, c: &Opt) -> LogExpr {
        let both_present = conj(
            [a.present_cond(), c.present_cond()],
            LogExpr::NullSafeEqual { lhs: b(a.value()), rhs: b(c.value()) },
        );
        match (a, c) {
            (Opt::Maybe(x), Opt::Maybe(y)) => or(and(size_is(x.clone(), 0), size_is(y.clone(), 0)), both_present),
            _ => both_present,
        }
    }

    /// Less-than: both present and ordered.
    fn lt(a: &Opt, c: &Opt) -> LogExpr {
        conj([a.present_cond(), c.present_cond()], LogExpr::LessThan { lhs: b(a.value()), rhs: b(c.value()) })
    }

    fn opt(&mut self, o: &Operand) -> Opt {
        match o {
            Operand::Literal(v) => Opt::Present(lit(v.clone())),
            Operand::Param(p) => Opt::Present(self.param(p)),
            Operand::Query(q) => Opt::Maybe(self.query(q)),
            Operand::Call(c) => self.value_call(c),
        }
    }

    fn value_call(&mut self, c: &Call) -> Opt {
        match c.name {
            Func::Count => {
                let Operand::Query(q) = &c.args[0] else { unreachable!() };
                Opt::Present(size(self.query(q)))
            }
            Func::Value => {
                let Operand::Query(q) = &c.args[0] else { unreachable!() };
                let nodes = self.query(q);
                Opt::Maybe(LogExpr::Conditional {
                    condition: b(size_is(nodes.clone(), 1)),
                    then_expr: b(nodes),
                    else_expr: b(empty()),
                })
            }
            // Nothing unless the argument is present and has a length (a
            // string, array, or object); `size` of anything else is null.
            Func::Length => {
                let (present, v) = match self.opt(&c.args[0]) {
                    Opt::Present(v) => (None, v),
                    Opt::Maybe(l) => (Some(size_is(l.clone(), 1)), first(l)),
                };
                let n = size(v);
                let has_length = LogExpr::IsNotNull { operand: b(n.clone()) };
                Opt::Maybe(LogExpr::Conditional {
                    condition: b(conj([present], has_length)),
                    then_expr: b(singleton(n)),
                    else_expr: b(empty()),
                })
            }
            Func::Match | Func::Search => unreachable!("logical function in value position"),
        }
    }

    fn logical_call(&mut self, c: &Call) -> LogExpr {
        let anchored = c.name == Func::Match;
        let subject = self.opt(&c.args[0]);
        let pattern = match &c.args[1] {
            Operand::Literal(Value::String(re)) => {
                let re = iregexp_to_rust(re);
                Opt::Present(lit(Value::String(if anchored { format!("^(?:{re})$") } else { re })))
            }
            other => {
                let p = self.opt(other);
                if anchored {
                    let wrap = |e: LogExpr| LogExpr::Concat {
                        lhs: b(LogExpr::Concat { lhs: b(lit(Value::String("^(?:".into()))), rhs: b(e) }),
                        rhs: b(lit(Value::String(")$".into()))),
                    };
                    match p {
                        Opt::Present(v) => Opt::Present(wrap(v)),
                        Opt::Maybe(l) => {
                            let cond = size_is(l.clone(), 1);
                            return conj(
                                [subject.present_cond(), Some(cond)],
                                LogExpr::RegexMatch { receiver: b(subject.value()), arg: b(wrap(first(l))) },
                            );
                        }
                    }
                } else {
                    p
                }
            }
        };
        conj(
            [subject.present_cond(), pattern.present_cond()],
            LogExpr::RegexMatch { receiver: b(subject.value()), arg: b(pattern.value()) },
        )
    }
}

/// Translate an I-Regexp (RFC 9485) into the regex engine's syntax. The
/// only difference that matters for well-formed I-Regexps is `.`, which in
/// I-Regexp matches any character except CR and LF.
fn iregexp_to_rust(re: &str) -> String {
    let mut out = String::with_capacity(re.len());
    let mut in_class = false;
    let mut chars = re.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                out.push(c);
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            }
            '[' if !in_class => {
                in_class = true;
                out.push(c);
            }
            ']' if in_class => {
                in_class = false;
                out.push(c);
            }
            '.' if !in_class => out.push_str("[^\\n\\r]"),
            c => out.push(c),
        }
    }
    out
}

/// Variables the translation reads, for callers building an evaluation
/// environment: the root and every parameter.
pub fn free_variables(t: &JsonPathTranslation) -> Vec<String> {
    let mut v = vec![ROOT.to_string()];
    v.extend(t.params.iter().cloned());
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tr(q: &str) -> LogExpr {
        jsonpath_to_log_expr(q).unwrap_or_else(|e| panic!("{q}: {e}")).expr
    }

    fn rejects(q: &str) {
        assert!(jsonpath_to_log_expr(q).is_err(), "should reject {q:?}");
    }

    fn member(x: LogExpr, k: &str) -> LogExpr {
        LogExpr::GetMemberByKey { lhs: b(x), rhs: b(lit(Value::String(k.into()))) }
    }

    #[test]
    fn root_alone_is_singleton_of_root() {
        assert_eq!(tr("$"), singleton(var(ROOT)));
    }

    #[test]
    fn first_segment_applies_to_root_directly() {
        assert_eq!(tr("$.a"), member(var(ROOT), "a"));
        assert_eq!(tr("$['a']"), member(var(ROOT), "a"));
    }

    #[test]
    fn later_segments_flat_map() {
        // Written with a raw binder name; the translation's binders are
        // canonical, so compare against the canonicalized form.
        let expected = LogExpr::Flatten {
            operand: b(LogExpr::MapTransform {
                collection: b(member(var(ROOT), "a")),
                binding: "v".into(),
                body: b(member(var("v"), "b")),
            }),
        };
        assert_eq!(tr("$.a.b"), crate::bindings::canonicalize_bindings(&expected));
    }

    #[test]
    fn index_and_negative_index() {
        assert_eq!(tr("$[-1]"), LogExpr::GetElementByIndex { lhs: b(var(ROOT)), rhs: b(int(-1)) });
    }

    #[test]
    fn param_selector_reports_param() {
        let t = jsonpath_to_log_expr("$.users[{id}]").unwrap();
        assert_eq!(t.params, vec!["id"]);
        assert_eq!(free_variables(&t), vec!["$", "id"]);
    }

    #[test]
    fn params_in_filters_and_first_reference_order() {
        let t = jsonpath_to_log_expr("$.a[?@.x == {b} && @.y == {a} || @.z == {b}]").unwrap();
        assert_eq!(t.params, vec!["b", "a"]);
    }

    #[test]
    fn filter_binds_current_node() {
        match tr("$[?@.x]") {
            LogExpr::Filter { binding, body, .. } => {
                let at = LogExpr::GetBinding { binding_name: binding.clone() };
                assert_eq!(
                    *body,
                    LogExpr::GreaterThan { lhs: b(size(member(at, "x"))), rhs: b(int(0)) }
                );
            }
            other => panic!("expected Filter, got {other:?}"),
        }
    }

    #[test]
    fn literal_regex_is_translated_and_anchored() {
        assert_eq!(iregexp_to_rust("a.b[.]"), "a[^\\n\\r]b[.]");
        let e = tr("$[?match(@.s, 'a.c')]");
        let LogExpr::Filter { body, .. } = e else { panic!() };
        let text = format!("{body:?}");
        assert!(text.contains("^(?:a[^\\\\n\\\\r]c)$"), "{text}");
    }

    #[test]
    fn rejects_malformed() {
        for q in [
            "", " $", "$ ", "a", "$.", "$..", "$[", "$[]", "$[01]", "$[-0]", "$['a'",
            "$[?true]", "$[?@.a == @..b]", "$[?count(@.a) ]", "$[?length(@.*) > 1]",
            "$[?foo(@)]", "$[?match(@.a)]", "$[?!@.a == 1]", "$.a b", "$[9007199254740992]",
            "$['\\x']", "$[?@.a == 01]", "$[{}]", "$['\u{1}']",
        ] {
            rejects(q);
        }
    }

    #[test]
    fn accepts_rfc_examples() {
        for q in [
            "$.store.book[*].author", "$..author", "$.store.*", "$.store..price", "$..book[2]",
            "$..book[-1]", "$..book[0,1]", "$..book[:2]", "$..book[?@.isbn]",
            "$..book[?@.price<10]", "$..*", "$[1:3]", "$[5:]", "$[1:5:2]", "$[5:1:-2]", "$[::-1]",
            "$[?@.a == 'b' && (@.c > 1 || !@.d)]", "$[?length(@.authors) >= 5]",
            "$[?count(@.*.author) >= 5]", "$[?match(@.date, '1974-05-..')]",
            "$[?search(@.author, '[BR]ob')]", "$[?value(@..color) == 'red']", "$ .a",
            "$[ 'a' , 0 ]", "$.\u{e9}t\u{e9}", "$['\\u00e9\\ud83d\\ude00']", "$[?@.a==-0.5e+3]",
        ] {
            if let Err(e) = jsonpath_to_log_expr(q) {
                panic!("{q}: {e}");
            }
        }
    }

    /// Runs the parse/reject half of the JSONPath Compliance Test Suite when
    /// a copy is available: `$JSONPATH_CTS` pointing at cts.json, else a
    /// sibling checkout. Result comparison needs an evaluator and lives with
    /// one; this checks only that valid queries parse and invalid ones do not.
    #[test]
    fn compliance_suite_parse_and_reject() {
        let path = std::env::var("JSONPATH_CTS")
            .unwrap_or_else(|_| "../../jsonpath-compliance-test-suite/cts.json".into());
        let Ok(text) = std::fs::read_to_string(&path) else { return };
        let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
        let tests = doc["tests"].as_array().unwrap();
        assert!(!tests.is_empty(), "no tests in {path}");
        eprintln!("JSONPath CTS: checking {} queries from {path}", tests.len());
        let mut failures = Vec::new();
        for t in tests {
            let sel = t["selector"].as_str().unwrap();
            let invalid = t.get("invalid_selector").and_then(|v| v.as_bool()).unwrap_or(false);
            let ok = jsonpath_to_log_expr(sel).is_ok();
            if ok == invalid {
                failures.push(format!("{} ({}): {sel:?}", t["name"], if invalid { "accepted" } else { "rejected" }));
            }
        }
        assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
    }
}
