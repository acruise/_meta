use crate::expr_gen::LogExpr;
use crate::value::{Value, ValueKind};

const _: () = assert!(
    crate::expr_gen::EXPR_GEN_HASH == 0x6ebdb63aea14ca1f,
    "type_check.rs needs review — EXPR_GEN_HASH changed"
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoCastMode {
    Off,
    Safe,
    Aggressive,
}

#[derive(Debug, Clone)]
pub struct FieldSchema {
    pub name: String,
    pub kind: ValueKind,
    pub nullable: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ExprSchema {
    pub event_fields: Vec<FieldSchema>,
    pub enrichment_fields: Vec<FieldSchema>,
}

impl ExprSchema {
    fn lookup_event(&self, name: &str) -> Option<ValueKind> {
        self.event_fields.iter().find(|f| f.name == name).map(|f| f.kind)
    }

    fn lookup_enrichment(&self, name: &str) -> Option<ValueKind> {
        self.enrichment_fields.iter().find(|f| f.name == name).map(|f| f.kind)
    }
}

#[derive(Debug, Clone)]
pub struct TypeError {
    pub message: String,
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

type TResult = Result<(LogExpr, ValueKind), TypeError>;

pub struct TypeChecker {
    pub mode: AutoCastMode,
    pub schema: ExprSchema,
}

impl TypeChecker {
    pub fn new(mode: AutoCastMode, schema: ExprSchema) -> Self {
        Self { mode, schema }
    }

    pub fn check_all(&self, exprs: &[LogExpr]) -> Result<Vec<(LogExpr, ValueKind)>, TypeError> {
        exprs.iter().map(|e| self.check(e)).collect()
    }

    pub fn check(&self, expr: &LogExpr) -> TResult {
        match expr {
            LogExpr::Literal(v) => Ok((expr.clone(), v.kind())),

            LogExpr::GetFieldByName { field_name } => {
                if let Some(kind) = self.schema.lookup_event(field_name) {
                    Ok((expr.clone(), kind))
                } else {
                    Err(TypeError {
                        message: format!("unknown event field: '{field_name}'"),
                    })
                }
            }

            LogExpr::GetFieldByIndex { .. } => Ok((expr.clone(), ValueKind::Null)),
            LogExpr::GetColumn { .. } => Ok((expr.clone(), ValueKind::Null)),
            LogExpr::GetChildByName { .. } => Ok((expr.clone(), ValueKind::Null)),
            LogExpr::GetChildByIndex { .. } => Ok((expr.clone(), ValueKind::Null)),
            LogExpr::CurrentTimestamp => Ok((expr.clone(), ValueKind::Timestamp)),

            LogExpr::LogicalAnd { lhs, rhs } => {
                let (lhs, _) = self.expect_type(lhs, ValueKind::Bool, "&&")?;
                let (rhs, _) = self.expect_type(rhs, ValueKind::Bool, "&&")?;
                Ok((LogExpr::LogicalAnd { lhs: Box::new(lhs), rhs: Box::new(rhs) }, ValueKind::Bool))
            }
            LogExpr::LogicalOr { lhs, rhs } => {
                let (lhs, _) = self.expect_type(lhs, ValueKind::Bool, "||")?;
                let (rhs, _) = self.expect_type(rhs, ValueKind::Bool, "||")?;
                Ok((LogExpr::LogicalOr { lhs: Box::new(lhs), rhs: Box::new(rhs) }, ValueKind::Bool))
            }
            LogExpr::LogicalNot { operand } => {
                let (operand, _) = self.expect_type(operand, ValueKind::Bool, "!")?;
                Ok((LogExpr::LogicalNot { operand: Box::new(operand) }, ValueKind::Bool))
            }

            LogExpr::Equal { lhs, rhs } => self.check_comparison(lhs, rhs, "==", |l, r| LogExpr::Equal { lhs: l, rhs: r }),
            LogExpr::NotEqual { lhs, rhs } => self.check_comparison(lhs, rhs, "!=", |l, r| LogExpr::NotEqual { lhs: l, rhs: r }),
            LogExpr::LessThan { lhs, rhs } => self.check_comparison(lhs, rhs, "<", |l, r| LogExpr::LessThan { lhs: l, rhs: r }),
            LogExpr::LessOrEqual { lhs, rhs } => self.check_comparison(lhs, rhs, "<=", |l, r| LogExpr::LessOrEqual { lhs: l, rhs: r }),
            LogExpr::GreaterThan { lhs, rhs } => self.check_comparison(lhs, rhs, ">", |l, r| LogExpr::GreaterThan { lhs: l, rhs: r }),
            LogExpr::GreaterOrEqual { lhs, rhs } => self.check_comparison(lhs, rhs, ">=", |l, r| LogExpr::GreaterOrEqual { lhs: l, rhs: r }),

            LogExpr::Add { lhs, rhs } => self.check_numeric_binop(lhs, rhs, "+", |l, r| LogExpr::Add { lhs: l, rhs: r }),
            LogExpr::Subtract { lhs, rhs } => self.check_numeric_binop(lhs, rhs, "-", |l, r| LogExpr::Subtract { lhs: l, rhs: r }),
            LogExpr::Multiply { lhs, rhs } => self.check_numeric_binop(lhs, rhs, "*", |l, r| LogExpr::Multiply { lhs: l, rhs: r }),
            LogExpr::Divide { lhs, rhs } => self.check_numeric_binop(lhs, rhs, "/", |l, r| LogExpr::Divide { lhs: l, rhs: r }),
            LogExpr::Modulus { lhs, rhs } => self.check_numeric_binop(lhs, rhs, "%", |l, r| LogExpr::Modulus { lhs: l, rhs: r }),
            LogExpr::Negate { operand } => {
                let (operand, kind) = self.check(operand)?;
                if !is_numeric(kind) {
                    return Err(TypeError { message: format!("cannot negate {kind:?}") });
                }
                Ok((LogExpr::Negate { operand: Box::new(operand) }, kind))
            }

            LogExpr::Contains { receiver, arg } => self.check_string_method(receiver, arg, "contains", |r, a| LogExpr::Contains { receiver: r, arg: a }),
            LogExpr::StartsWith { receiver, arg } => self.check_string_method(receiver, arg, "startsWith", |r, a| LogExpr::StartsWith { receiver: r, arg: a }),
            LogExpr::EndsWith { receiver, arg } => self.check_string_method(receiver, arg, "endsWith", |r, a| LogExpr::EndsWith { receiver: r, arg: a }),
            LogExpr::RegexMatch { receiver, arg } => self.check_string_method(receiver, arg, "matches", |r, a| LogExpr::RegexMatch { receiver: r, arg: a }),
            LogExpr::Concat { lhs, rhs } => {
                let (lhs, _) = self.expect_type(lhs, ValueKind::String, "+")?;
                let (rhs, _) = self.expect_type(rhs, ValueKind::String, "+")?;
                Ok((LogExpr::Concat { lhs: Box::new(lhs), rhs: Box::new(rhs) }, ValueKind::String))
            }
            LogExpr::Upper { operand } => self.check_string_unary(operand, |o| LogExpr::Upper { operand: o }),
            LogExpr::Lower { operand } => self.check_string_unary(operand, |o| LogExpr::Lower { operand: o }),
            LogExpr::Trim { operand } => self.check_string_unary(operand, |o| LogExpr::Trim { operand: o }),

            LogExpr::Size { operand } => {
                let (operand, kind) = self.check(operand)?;
                if !is_sizable(kind) {
                    return Err(TypeError { message: format!("size() not applicable to {kind:?}") });
                }
                Ok((LogExpr::Size { operand: Box::new(operand) }, ValueKind::I64))
            }

            LogExpr::CastInt { operand } => {
                let (operand, _) = self.check(operand)?;
                Ok((LogExpr::CastInt { operand: Box::new(operand) }, ValueKind::I64))
            }
            LogExpr::CastDouble { operand } => {
                let (operand, _) = self.check(operand)?;
                Ok((LogExpr::CastDouble { operand: Box::new(operand) }, ValueKind::F64))
            }
            LogExpr::CastString { operand } => {
                let (operand, _) = self.check(operand)?;
                Ok((LogExpr::CastString { operand: Box::new(operand) }, ValueKind::String))
            }
            LogExpr::CastBool { operand } => {
                let (operand, _) = self.check(operand)?;
                Ok((LogExpr::CastBool { operand: Box::new(operand) }, ValueKind::Bool))
            }
            LogExpr::CastUint { operand } => {
                let (operand, _) = self.check(operand)?;
                Ok((LogExpr::CastUint { operand: Box::new(operand) }, ValueKind::U64))
            }
            LogExpr::CastBytes { operand } => {
                let (operand, _) = self.check(operand)?;
                Ok((LogExpr::CastBytes { operand: Box::new(operand) }, ValueKind::Blob))
            }
            LogExpr::CastTimestamp { operand } => {
                let (operand, _) = self.check(operand)?;
                Ok((LogExpr::CastTimestamp { operand: Box::new(operand) }, ValueKind::Timestamp))
            }
            LogExpr::CastDuration { operand } => {
                let (operand, _) = self.check(operand)?;
                Ok((LogExpr::CastDuration { operand: Box::new(operand) }, ValueKind::Timestamp))
            }

            LogExpr::Conditional { condition, then_expr, else_expr } => {
                let (condition, _) = self.expect_type(condition, ValueKind::Bool, "?:")?;
                let (then_expr, then_kind) = self.check(then_expr)?;
                let (else_expr, else_kind) = self.check(else_expr)?;
                let (then_expr, else_expr, result_kind) = self.unify_pair(then_expr, then_kind, else_expr, else_kind, "?:")?;
                Ok((
                    LogExpr::Conditional {
                        condition: Box::new(condition),
                        then_expr: Box::new(then_expr),
                        else_expr: Box::new(else_expr),
                    },
                    result_kind,
                ))
            }

            LogExpr::Case { arms, default } => {
                let mut checked_arms = Vec::with_capacity(arms.len());
                let (default, mut result_kind) = self.check(default)?;

                for (cond, result) in arms {
                    let (cond, _) = self.expect_type(cond, ValueKind::Bool, "CASE")?;
                    let (result, arm_kind) = self.check(result)?;
                    let (_, result, unified) = self.unify_pair(
                        LogExpr::Literal(Value::Null), result_kind,
                        result, arm_kind,
                        "CASE",
                    )?;
                    result_kind = unified;
                    checked_arms.push((Box::new(cond), Box::new(result)));
                }

                Ok((
                    LogExpr::Case { arms: checked_arms, default: Box::new(default) },
                    result_kind,
                ))
            }

            LogExpr::IsNull { operand } => {
                let (operand, _) = self.check(operand)?;
                Ok((LogExpr::IsNull { operand: Box::new(operand) }, ValueKind::Bool))
            }
            LogExpr::IsNotNull { operand } => {
                let (operand, _) = self.check(operand)?;
                Ok((LogExpr::IsNotNull { operand: Box::new(operand) }, ValueKind::Bool))
            }
            LogExpr::IsNan { operand } => {
                let (operand, _) = self.expect_type(operand, ValueKind::F64, "isNaN")?;
                Ok((LogExpr::IsNan { operand: Box::new(operand) }, ValueKind::Bool))
            }
            LogExpr::IsFinite { operand } => {
                let (operand, _) = self.expect_type(operand, ValueKind::F64, "isFinite")?;
                Ok((LogExpr::IsFinite { operand: Box::new(operand) }, ValueKind::Bool))
            }
            LogExpr::IsInfinite { operand } => {
                let (operand, _) = self.expect_type(operand, ValueKind::F64, "isInfinite")?;
                Ok((LogExpr::IsInfinite { operand: Box::new(operand) }, ValueKind::Bool))
            }

            LogExpr::Coalesce { lhs, rhs } => {
                let (lhs, lhs_kind) = self.check(lhs)?;
                let (rhs, rhs_kind) = self.check(rhs)?;
                let (lhs, rhs, kind) = self.unify_pair(lhs, lhs_kind, rhs, rhs_kind, "coalesce")?;
                Ok((LogExpr::Coalesce { lhs: Box::new(lhs), rhs: Box::new(rhs) }, kind))
            }

            LogExpr::CelUdf { source, args } => {
                let checked_args: Vec<Box<LogExpr>> = args.iter()
                    .map(|a| self.check(a).map(|(e, _)| Box::new(e)))
                    .collect::<Result<_, _>>()?;
                Ok((LogExpr::CelUdf { source: source.clone(), args: checked_args }, ValueKind::Null))
            }

            _ => Ok((expr.clone(), ValueKind::Null)),
        }
    }

    fn check_comparison(
        &self,
        lhs: &LogExpr,
        rhs: &LogExpr,
        op: &str,
        build: impl Fn(Box<LogExpr>, Box<LogExpr>) -> LogExpr,
    ) -> TResult {
        let (lhs, lhs_kind) = self.check(lhs)?;
        let (rhs, rhs_kind) = self.check(rhs)?;
        let (lhs, rhs, _) = self.unify_pair(lhs, lhs_kind, rhs, rhs_kind, op)?;
        Ok((build(Box::new(lhs), Box::new(rhs)), ValueKind::Bool))
    }

    fn check_numeric_binop(
        &self,
        lhs: &LogExpr,
        rhs: &LogExpr,
        op: &str,
        build: impl Fn(Box<LogExpr>, Box<LogExpr>) -> LogExpr,
    ) -> TResult {
        let (lhs, lhs_kind) = self.check(lhs)?;
        let (rhs, rhs_kind) = self.check(rhs)?;
        if !is_numeric(lhs_kind) {
            return Err(TypeError { message: format!("left operand of '{op}' is {lhs_kind:?}, expected numeric") });
        }
        if !is_numeric(rhs_kind) {
            return Err(TypeError { message: format!("right operand of '{op}' is {rhs_kind:?}, expected numeric") });
        }
        let (lhs, rhs, kind) = self.unify_pair(lhs, lhs_kind, rhs, rhs_kind, op)?;
        Ok((build(Box::new(lhs), Box::new(rhs)), kind))
    }

    fn check_string_method(
        &self,
        receiver: &LogExpr,
        arg: &LogExpr,
        method: &str,
        build: impl Fn(Box<LogExpr>, Box<LogExpr>) -> LogExpr,
    ) -> TResult {
        let (receiver, _) = self.expect_type(receiver, ValueKind::String, method)?;
        let (arg, _) = self.expect_type(arg, ValueKind::String, method)?;
        Ok((build(Box::new(receiver), Box::new(arg)), ValueKind::Bool))
    }

    fn check_string_unary(
        &self,
        operand: &LogExpr,
        build: impl Fn(Box<LogExpr>) -> LogExpr,
    ) -> TResult {
        let (operand, _) = self.expect_type(operand, ValueKind::String, "string op")?;
        Ok((build(Box::new(operand)), ValueKind::String))
    }

    fn expect_type(&self, expr: &LogExpr, expected: ValueKind, context: &str) -> TResult {
        let (expr, actual) = self.check(expr)?;
        if actual == expected {
            return Ok((expr, actual));
        }
        if let Some(cast) = self.try_cast(&expr, actual, expected) {
            return Ok((cast, expected));
        }
        Err(TypeError {
            message: format!("type mismatch in '{context}': expected {expected:?}, got {actual:?}"),
        })
    }

    fn unify_pair(
        &self,
        lhs: LogExpr,
        lhs_kind: ValueKind,
        rhs: LogExpr,
        rhs_kind: ValueKind,
        context: &str,
    ) -> Result<(LogExpr, LogExpr, ValueKind), TypeError> {
        if lhs_kind == rhs_kind {
            return Ok((lhs, rhs, lhs_kind));
        }
        if let Some(common) = common_type(lhs_kind, rhs_kind, self.mode) {
            let lhs = if lhs_kind != common {
                self.try_cast(&lhs, lhs_kind, common).unwrap_or(lhs)
            } else {
                lhs
            };
            let rhs = if rhs_kind != common {
                self.try_cast(&rhs, rhs_kind, common).unwrap_or(rhs)
            } else {
                rhs
            };
            return Ok((lhs, rhs, common));
        }
        Err(TypeError {
            message: format!(
                "incompatible types in '{context}': {lhs_kind:?} vs {rhs_kind:?}"
            ),
        })
    }

    fn try_cast(&self, expr: &LogExpr, from: ValueKind, to: ValueKind) -> Option<LogExpr> {
        if from == to {
            return Some(expr.clone());
        }
        let allowed = match self.mode {
            AutoCastMode::Off => return None,
            AutoCastMode::Safe => is_safe_widening(from, to),
            AutoCastMode::Aggressive => is_safe_widening(from, to) || is_aggressive_cast(from, to),
        };
        if !allowed {
            return None;
        }
        let boxed = Box::new(expr.clone());
        match to {
            ValueKind::I64 => Some(LogExpr::CastInt { operand: boxed }),
            ValueKind::U64 => Some(LogExpr::CastUint { operand: boxed }),
            ValueKind::F64 => Some(LogExpr::CastDouble { operand: boxed }),
            ValueKind::String => Some(LogExpr::CastString { operand: boxed }),
            ValueKind::Bool => Some(LogExpr::CastBool { operand: boxed }),
            ValueKind::Timestamp => Some(LogExpr::CastTimestamp { operand: boxed }),
            _ => None,
        }
    }
}

fn is_numeric(k: ValueKind) -> bool {
    matches!(
        k,
        ValueKind::I8 | ValueKind::I16 | ValueKind::I32 | ValueKind::I64
            | ValueKind::U8 | ValueKind::U16 | ValueKind::U32 | ValueKind::U64
            | ValueKind::F32 | ValueKind::F64
    )
}

fn is_sizable(k: ValueKind) -> bool {
    matches!(k, ValueKind::String | ValueKind::Blob | ValueKind::Array)
}

fn is_safe_widening(from: ValueKind, to: ValueKind) -> bool {
    matches!(
        (from, to),
        (ValueKind::I8, ValueKind::I16 | ValueKind::I32 | ValueKind::I64)
            | (ValueKind::I16, ValueKind::I32 | ValueKind::I64)
            | (ValueKind::I32, ValueKind::I64)
            | (ValueKind::U8, ValueKind::U16 | ValueKind::U32 | ValueKind::U64 | ValueKind::I16 | ValueKind::I32 | ValueKind::I64)
            | (ValueKind::U16, ValueKind::U32 | ValueKind::U64 | ValueKind::I32 | ValueKind::I64)
            | (ValueKind::U32, ValueKind::U64 | ValueKind::I64)
            | (ValueKind::F32, ValueKind::F64)
            | (ValueKind::I8 | ValueKind::I16 | ValueKind::I32, ValueKind::F64)
            | (ValueKind::U8 | ValueKind::U16 | ValueKind::U32, ValueKind::F64)
            | (ValueKind::I64 | ValueKind::U64, ValueKind::F64)
    )
}

fn is_aggressive_cast(from: ValueKind, to: ValueKind) -> bool {
    matches!(
        (from, to),
        (ValueKind::I64, ValueKind::I32 | ValueKind::I16 | ValueKind::I8)
            | (ValueKind::I32, ValueKind::I16 | ValueKind::I8)
            | (ValueKind::I16, ValueKind::I8)
            | (ValueKind::U64, ValueKind::U32 | ValueKind::U16 | ValueKind::U8)
            | (ValueKind::U32, ValueKind::U16 | ValueKind::U8)
            | (ValueKind::U16, ValueKind::U8)
            | (ValueKind::F64, ValueKind::I64 | ValueKind::I32)
            | (ValueKind::F32, ValueKind::I64 | ValueKind::I32)
            | (ValueKind::I64, ValueKind::U64)
            | (ValueKind::U64, ValueKind::I64)
            | (ValueKind::String, ValueKind::I64 | ValueKind::F64 | ValueKind::Bool)
    )
}

fn common_type(a: ValueKind, b: ValueKind, mode: AutoCastMode) -> Option<ValueKind> {
    if a == b {
        return Some(a);
    }

    let check_widening = |from, to| match mode {
        AutoCastMode::Off => false,
        AutoCastMode::Safe => is_safe_widening(from, to),
        AutoCastMode::Aggressive => is_safe_widening(from, to) || is_aggressive_cast(from, to),
    };

    if check_widening(a, b) {
        return Some(b);
    }
    if check_widening(b, a) {
        return Some(a);
    }

    if is_numeric(a) && is_numeric(b) && mode != AutoCastMode::Off {
        return Some(ValueKind::F64);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(fields: Vec<(&str, ValueKind)>) -> ExprSchema {
        ExprSchema {
            event_fields: fields
                .into_iter()
                .map(|(name, kind)| FieldSchema {
                    name: name.to_string(),
                    kind,
                    nullable: false,
                })
                .collect(),
            enrichment_fields: vec![],
        }
    }

    fn checker(mode: AutoCastMode, fields: Vec<(&str, ValueKind)>) -> TypeChecker {
        TypeChecker::new(mode, schema(fields))
    }

    #[test]
    fn same_type_comparison() {
        let tc = checker(AutoCastMode::Safe, vec![("age", ValueKind::I64)]);
        let expr = LogExpr::GreaterThan {
            lhs: Box::new(LogExpr::GetFieldByName { field_name: "age".into() }),
            rhs: Box::new(LogExpr::Literal(Value::I64(18))),
        };
        let (result, kind) = tc.check(&expr).unwrap();
        assert_eq!(kind, ValueKind::Bool);
        match &result {
            LogExpr::GreaterThan { lhs, rhs } => {
                assert!(matches!(**lhs, LogExpr::GetFieldByName { .. }));
                assert!(matches!(**rhs, LogExpr::Literal(Value::I64(18))));
            }
            _ => panic!("expected GreaterThan"),
        }
    }

    #[test]
    fn safe_widening_int_to_double() {
        let tc = checker(
            AutoCastMode::Safe,
            vec![("qty", ValueKind::I64), ("price", ValueKind::F64)],
        );
        let expr = LogExpr::Multiply {
            lhs: Box::new(LogExpr::GetFieldByName { field_name: "qty".into() }),
            rhs: Box::new(LogExpr::GetFieldByName { field_name: "price".into() }),
        };
        let (result, kind) = tc.check(&expr).unwrap();
        assert_eq!(kind, ValueKind::F64);
        match &result {
            LogExpr::Multiply { lhs, .. } => {
                assert!(matches!(**lhs, LogExpr::CastDouble { .. }));
            }
            _ => panic!("expected Multiply"),
        }
    }

    #[test]
    fn off_mode_rejects_mismatch() {
        let tc = checker(
            AutoCastMode::Off,
            vec![("qty", ValueKind::I64), ("price", ValueKind::F64)],
        );
        let expr = LogExpr::Multiply {
            lhs: Box::new(LogExpr::GetFieldByName { field_name: "qty".into() }),
            rhs: Box::new(LogExpr::GetFieldByName { field_name: "price".into() }),
        };
        assert!(tc.check(&expr).is_err());
    }

    #[test]
    fn unknown_field_error() {
        let tc = checker(AutoCastMode::Safe, vec![]);
        let expr = LogExpr::GetFieldByName { field_name: "missing".into() };
        assert!(tc.check(&expr).is_err());
    }

    #[test]
    fn conditional_branch_unification() {
        let tc = checker(
            AutoCastMode::Safe,
            vec![("flag", ValueKind::Bool)],
        );
        let expr = LogExpr::Conditional {
            condition: Box::new(LogExpr::GetFieldByName { field_name: "flag".into() }),
            then_expr: Box::new(LogExpr::Literal(Value::I64(1))),
            else_expr: Box::new(LogExpr::Literal(Value::F64(2.5))),
        };
        let (result, kind) = tc.check(&expr).unwrap();
        assert_eq!(kind, ValueKind::F64);
        match &result {
            LogExpr::Conditional { then_expr, .. } => {
                assert!(matches!(**then_expr, LogExpr::CastDouble { .. }));
            }
            _ => panic!("expected Conditional"),
        }
    }

    #[test]
    fn incompatible_comparison() {
        let tc = checker(
            AutoCastMode::Safe,
            vec![("name", ValueKind::String), ("age", ValueKind::I64)],
        );
        let expr = LogExpr::Equal {
            lhs: Box::new(LogExpr::GetFieldByName { field_name: "name".into() }),
            rhs: Box::new(LogExpr::GetFieldByName { field_name: "age".into() }),
        };
        assert!(tc.check(&expr).is_err());
    }

    #[test]
    fn aggressive_allows_string_to_int_cast() {
        let tc = checker(
            AutoCastMode::Aggressive,
            vec![("code", ValueKind::String)],
        );
        let expr = LogExpr::Equal {
            lhs: Box::new(LogExpr::GetFieldByName { field_name: "code".into() }),
            rhs: Box::new(LogExpr::Literal(Value::I64(404))),
        };
        let (result, kind) = tc.check(&expr).unwrap();
        assert_eq!(kind, ValueKind::Bool);
        match &result {
            LogExpr::Equal { lhs, .. } => {
                assert!(matches!(**lhs, LogExpr::CastInt { .. }));
            }
            _ => panic!("expected Equal"),
        }
    }

    #[test]
    fn safe_rejects_string_int_comparison() {
        let tc = checker(
            AutoCastMode::Safe,
            vec![("code", ValueKind::String)],
        );
        let expr = LogExpr::Equal {
            lhs: Box::new(LogExpr::GetFieldByName { field_name: "code".into() }),
            rhs: Box::new(LogExpr::Literal(Value::I64(404))),
        };
        assert!(tc.check(&expr).is_err());
    }

    #[test]
    fn integer_widening_chain() {
        let tc = checker(
            AutoCastMode::Safe,
            vec![("small", ValueKind::I32), ("big", ValueKind::I64)],
        );
        let expr = LogExpr::Add {
            lhs: Box::new(LogExpr::GetFieldByName { field_name: "small".into() }),
            rhs: Box::new(LogExpr::GetFieldByName { field_name: "big".into() }),
        };
        let (result, kind) = tc.check(&expr).unwrap();
        assert_eq!(kind, ValueKind::I64);
        match &result {
            LogExpr::Add { lhs, .. } => {
                assert!(matches!(**lhs, LogExpr::CastInt { .. }));
            }
            _ => panic!("expected Add"),
        }
    }

    #[test]
    fn logical_requires_bool() {
        let tc = checker(
            AutoCastMode::Safe,
            vec![("x", ValueKind::I64)],
        );
        let expr = LogExpr::LogicalAnd {
            lhs: Box::new(LogExpr::GetFieldByName { field_name: "x".into() }),
            rhs: Box::new(LogExpr::Literal(Value::Bool(true))),
        };
        assert!(tc.check(&expr).is_err());
    }

    #[test]
    fn size_on_string() {
        let tc = checker(AutoCastMode::Safe, vec![("name", ValueKind::String)]);
        let expr = LogExpr::Size {
            operand: Box::new(LogExpr::GetFieldByName { field_name: "name".into() }),
        };
        let (_, kind) = tc.check(&expr).unwrap();
        assert_eq!(kind, ValueKind::I64);
    }

    #[test]
    fn size_on_int_fails() {
        let tc = checker(AutoCastMode::Safe, vec![("x", ValueKind::I64)]);
        let expr = LogExpr::Size {
            operand: Box::new(LogExpr::GetFieldByName { field_name: "x".into() }),
        };
        assert!(tc.check(&expr).is_err());
    }
}
