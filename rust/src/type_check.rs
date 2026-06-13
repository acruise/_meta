use crate::expr_gen::LogExpr;
use crate::value::{Value, ValueType};

#[derive(Debug, Clone)]
pub struct ExternalUdfMeta {
    pub namespace: String,
    pub version: String,
    pub cel_name: String,
    pub param_types: Vec<ValueType>,
    pub return_type: ValueType,
}

impl From<&meta_types::external_fn::UdfCatalogEntry> for ExternalUdfMeta {
    fn from(entry: &meta_types::external_fn::UdfCatalogEntry) -> Self {
        Self {
            namespace: entry.namespace.clone(),
            version: entry.version.clone(),
            cel_name: entry.cel_name.clone(),
            param_types: entry.params.iter().map(|p| p.value_type.clone()).collect(),
            return_type: entry.return_type.clone(),
        }
    }
}

const _: () = assert!(
    crate::expr_gen::EXPR_GEN_HASH == 0x4c5dfe8c3da1b6cd,
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
    pub value_type: ValueType,
    pub nullable: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ExprSchema {
    pub event_fields: Vec<FieldSchema>,
    pub enrichment_fields: Vec<FieldSchema>,
    pub external_udfs: Vec<ExternalUdfMeta>,
}

impl ExprSchema {
    fn lookup_event(&self, name: &str) -> Option<&ValueType> {
        self.event_fields.iter().find(|f| f.name == name).map(|f| &f.value_type)
    }

    pub fn lookup_external_udf(&self, udf: &meta_types::external_fn::ResolvedUdfRef) -> Option<&ExternalUdfMeta> {
        self.external_udfs.iter().find(|u| {
            u.namespace == udf.namespace && u.version == udf.version && u.cel_name == udf.cel_name
        })
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

type TResult = Result<(LogExpr, ValueType), TypeError>;

pub struct TypeChecker {
    pub mode: AutoCastMode,
    pub schema: ExprSchema,
}

impl TypeChecker {
    pub fn new(mode: AutoCastMode, schema: ExprSchema) -> Self {
        Self { mode, schema }
    }

    pub fn check_all(&self, exprs: &[LogExpr]) -> Result<Vec<(LogExpr, ValueType)>, TypeError> {
        exprs.iter().map(|e| self.check(e)).collect()
    }

    pub fn check(&self, expr: &LogExpr) -> TResult {
        match expr {
            LogExpr::Literal(v) => Ok((expr.clone(), value_to_type(v))),

            LogExpr::GetFieldByName { field_name } => {
                if let Some(vt) = self.schema.lookup_event(field_name) {
                    Ok((expr.clone(), vt.clone()))
                } else {
                    Err(TypeError {
                        message: format!("unknown event field: '{field_name}'"),
                    })
                }
            }

            LogExpr::GetFieldByIndex { .. } => Ok((expr.clone(), ValueType::Null)),

            LogExpr::GetChildByName { child_name, operand } => {
                let (operand_checked, operand_type) = self.check(operand)?;
                if let ValueType::Struct { fields } = &operand_type {
                    if let Some((idx, field)) = fields.iter().enumerate()
                        .find(|(_, f)| f.name == *child_name)
                    {
                        let field_type = field.value_type.clone();
                        return Ok((
                            LogExpr::GetChildByIndex {
                                child_index: idx as u32,
                                lhs: Box::new(operand_checked),
                                rhs: Box::new(LogExpr::Literal(Value::U32(idx as u32))),
                            },
                            field_type,
                        ));
                    }
                    return Err(TypeError {
                        message: format!(
                            "struct has no field '{child_name}'; available: {}",
                            fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", ")
                        ),
                    });
                }
                Ok((
                    LogExpr::GetChildByName {
                        child_name: child_name.clone(),
                        operand: Box::new(operand_checked),
                    },
                    ValueType::Null,
                ))
            }

            LogExpr::GetChildByIndex { child_index, lhs, rhs } => {
                let (lhs_checked, lhs_type) = self.check(lhs)?;
                let (rhs_checked, _) = self.check(rhs)?;
                let field_type = if let ValueType::Struct { fields } = &lhs_type {
                    fields.get(*child_index as usize)
                        .map(|f| f.value_type.clone())
                        .unwrap_or(ValueType::Null)
                } else {
                    ValueType::Null
                };
                Ok((
                    LogExpr::GetChildByIndex {
                        child_index: *child_index,
                        lhs: Box::new(lhs_checked),
                        rhs: Box::new(rhs_checked),
                    },
                    field_type,
                ))
            }
            LogExpr::CurrentTimestamp => Ok((expr.clone(), ValueType::Timestamp {
                precision: crate::value::TimestampPrecision::Millis,
                timezone: crate::value::TimestampTimezone::None,
            })),

            LogExpr::LogicalAnd { lhs, rhs } => {
                let (lhs, _) = self.expect_type(lhs, &ValueType::Bool, "&&")?;
                let (rhs, _) = self.expect_type(rhs, &ValueType::Bool, "&&")?;
                Ok((LogExpr::LogicalAnd { lhs: Box::new(lhs), rhs: Box::new(rhs) }, ValueType::Bool))
            }
            LogExpr::LogicalOr { lhs, rhs } => {
                let (lhs, _) = self.expect_type(lhs, &ValueType::Bool, "||")?;
                let (rhs, _) = self.expect_type(rhs, &ValueType::Bool, "||")?;
                Ok((LogExpr::LogicalOr { lhs: Box::new(lhs), rhs: Box::new(rhs) }, ValueType::Bool))
            }
            LogExpr::LogicalNot { operand } => {
                let (operand, _) = self.expect_type(operand, &ValueType::Bool, "!")?;
                Ok((LogExpr::LogicalNot { operand: Box::new(operand) }, ValueType::Bool))
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
                let (operand, vt) = self.check(operand)?;
                if !is_numeric(&vt) {
                    return Err(TypeError { message: format!("cannot negate {vt:?}") });
                }
                Ok((LogExpr::Negate { operand: Box::new(operand) }, vt))
            }

            LogExpr::Contains { receiver, arg } => self.check_string_method(receiver, arg, "contains", |r, a| LogExpr::Contains { receiver: r, arg: a }),
            LogExpr::StartsWith { receiver, arg } => self.check_string_method(receiver, arg, "startsWith", |r, a| LogExpr::StartsWith { receiver: r, arg: a }),
            LogExpr::EndsWith { receiver, arg } => self.check_string_method(receiver, arg, "endsWith", |r, a| LogExpr::EndsWith { receiver: r, arg: a }),
            LogExpr::RegexMatch { receiver, arg } => self.check_string_method(receiver, arg, "matches", |r, a| LogExpr::RegexMatch { receiver: r, arg: a }),
            LogExpr::Concat { lhs, rhs } => {
                let (lhs, _) = self.expect_type(lhs, &ValueType::String, "+")?;
                let (rhs, _) = self.expect_type(rhs, &ValueType::String, "+")?;
                Ok((LogExpr::Concat { lhs: Box::new(lhs), rhs: Box::new(rhs) }, ValueType::String))
            }
            LogExpr::UrlPathSegment { arg0, arg1, arg2 } => {
                let (arg0, _) = self.expect_type(arg0, &ValueType::String, "url_path_segment")?;
                let (arg1, _) = self.expect_type(arg1, &ValueType::I64, "url_path_segment")?;
                // The mode is an optional trailing parameter: a Null
                // literal (padded or explicit) means the default mode.
                let (arg2, _) = self.check(arg2)?;
                Ok((LogExpr::UrlPathSegment {
                    arg0: Box::new(arg0), arg1: Box::new(arg1), arg2: Box::new(arg2),
                }, ValueType::String))
            }
            LogExpr::UrlQueryParam { arg0, arg1, arg2 } => {
                let (arg0, _) = self.expect_type(arg0, &ValueType::String, "url_query_param")?;
                let (arg1, _) = self.expect_type(arg1, &ValueType::String, "url_query_param")?;
                let (arg2, _) = self.check(arg2)?;
                Ok((LogExpr::UrlQueryParam {
                    arg0: Box::new(arg0), arg1: Box::new(arg1), arg2: Box::new(arg2),
                }, ValueType::String))
            }
            LogExpr::Upper { operand } => self.check_string_unary(operand, |o| LogExpr::Upper { operand: o }),
            LogExpr::Lower { operand } => self.check_string_unary(operand, |o| LogExpr::Lower { operand: o }),
            LogExpr::Trim { operand } => self.check_string_unary(operand, |o| LogExpr::Trim { operand: o }),

            LogExpr::Size { operand } => {
                let (operand, vt) = self.check(operand)?;
                if !is_sizable(&vt) {
                    return Err(TypeError { message: format!("size() not applicable to {vt:?}") });
                }
                Ok((LogExpr::Size { operand: Box::new(operand) }, ValueType::I64))
            }

            LogExpr::CastInt { operand } => {
                let (operand, _) = self.check(operand)?;
                Ok((LogExpr::CastInt { operand: Box::new(operand) }, ValueType::I64))
            }
            LogExpr::CastDouble { operand } => {
                let (operand, _) = self.check(operand)?;
                Ok((LogExpr::CastDouble { operand: Box::new(operand) }, ValueType::F64))
            }
            LogExpr::CastString { operand } => {
                let (operand, _) = self.check(operand)?;
                Ok((LogExpr::CastString { operand: Box::new(operand) }, ValueType::String))
            }
            LogExpr::CastBool { operand } => {
                let (operand, _) = self.check(operand)?;
                Ok((LogExpr::CastBool { operand: Box::new(operand) }, ValueType::Bool))
            }
            LogExpr::CastUint { operand } => {
                let (operand, _) = self.check(operand)?;
                Ok((LogExpr::CastUint { operand: Box::new(operand) }, ValueType::U64))
            }
            LogExpr::CastBytes { operand } => {
                let (operand, _) = self.check(operand)?;
                Ok((LogExpr::CastBytes { operand: Box::new(operand) }, ValueType::Blob))
            }
            LogExpr::CastTimestamp { operand } => {
                let (operand, _) = self.check(operand)?;
                Ok((LogExpr::CastTimestamp { operand: Box::new(operand) }, ValueType::Timestamp {
                    precision: crate::value::TimestampPrecision::Unspecified,
                    timezone: crate::value::TimestampTimezone::None,
                }))
            }
            LogExpr::CastDuration { operand } => {
                let (operand, _) = self.check(operand)?;
                Ok((LogExpr::CastDuration { operand: Box::new(operand) }, ValueType::Timestamp {
                    precision: crate::value::TimestampPrecision::Unspecified,
                    timezone: crate::value::TimestampTimezone::None,
                }))
            }

            LogExpr::Conditional { condition, then_expr, else_expr } => {
                let (condition, _) = self.expect_type(condition, &ValueType::Bool, "?:")?;
                let (then_expr, then_type) = self.check(then_expr)?;
                let (else_expr, else_type) = self.check(else_expr)?;
                let (then_expr, else_expr, result_type) = self.unify_pair(then_expr, then_type, else_expr, else_type, "?:")?;
                Ok((
                    LogExpr::Conditional {
                        condition: Box::new(condition),
                        then_expr: Box::new(then_expr),
                        else_expr: Box::new(else_expr),
                    },
                    result_type,
                ))
            }

            LogExpr::Case { arms, default } => {
                let mut checked_arms = Vec::with_capacity(arms.len());
                let (default, mut result_type) = self.check(default)?;

                for (cond, result) in arms {
                    let (cond, _) = self.expect_type(cond, &ValueType::Bool, "CASE")?;
                    let (result, arm_type) = self.check(result)?;
                    let (_, result, unified) = self.unify_pair(
                        LogExpr::Literal(Value::Null), result_type,
                        result, arm_type,
                        "CASE",
                    )?;
                    result_type = unified;
                    checked_arms.push((Box::new(cond), Box::new(result)));
                }

                Ok((
                    LogExpr::Case { arms: checked_arms, default: Box::new(default) },
                    result_type,
                ))
            }

            LogExpr::IsNull { operand } => {
                let (operand, _) = self.check(operand)?;
                Ok((LogExpr::IsNull { operand: Box::new(operand) }, ValueType::Bool))
            }
            LogExpr::IsNotNull { operand } => {
                let (operand, _) = self.check(operand)?;
                Ok((LogExpr::IsNotNull { operand: Box::new(operand) }, ValueType::Bool))
            }
            LogExpr::IsNan { operand } => {
                let (operand, _) = self.expect_type(operand, &ValueType::F64, "isNaN")?;
                Ok((LogExpr::IsNan { operand: Box::new(operand) }, ValueType::Bool))
            }
            LogExpr::IsFinite { operand } => {
                let (operand, _) = self.expect_type(operand, &ValueType::F64, "isFinite")?;
                Ok((LogExpr::IsFinite { operand: Box::new(operand) }, ValueType::Bool))
            }
            LogExpr::IsInfinite { operand } => {
                let (operand, _) = self.expect_type(operand, &ValueType::F64, "isInfinite")?;
                Ok((LogExpr::IsInfinite { operand: Box::new(operand) }, ValueType::Bool))
            }

            LogExpr::Coalesce { lhs, rhs } => {
                let (lhs, lt) = self.check(lhs)?;
                let (rhs, rt) = self.check(rhs)?;
                let (lhs, rhs, vt) = self.unify_pair(lhs, lt, rhs, rt, "coalesce")?;
                Ok((LogExpr::Coalesce { lhs: Box::new(lhs), rhs: Box::new(rhs) }, vt))
            }

            LogExpr::TryOrElse { lhs, rhs } => {
                let (lhs, lt) = self.check(lhs)?;
                let (rhs, rt) = self.check(rhs)?;
                let (lhs, rhs, vt) = self.unify_pair(lhs, lt, rhs, rt, "try_or_else")?;
                Ok((LogExpr::TryOrElse { lhs: Box::new(lhs), rhs: Box::new(rhs) }, vt))
            }

            LogExpr::RaiseError { operand } => {
                let (operand, _) = self.expect_type(operand, &ValueType::String, "raise_error")?;
                Ok((LogExpr::RaiseError { operand: Box::new(operand) }, ValueType::Null))
            }

            LogExpr::CelFallback { source, args } => {
                let checked_args: Vec<Box<LogExpr>> = args.iter()
                    .map(|a| self.check(a).map(|(e, _)| Box::new(e)))
                    .collect::<Result<_, _>>()?;
                Ok((LogExpr::CelFallback { source: source.clone(), args: checked_args }, ValueType::Null))
            }

            LogExpr::UnresolvedCall { name, .. } => {
                Err(TypeError {
                    message: format!(
                        "unresolved external call '{name}': run the UDF resolver before type checking"
                    ),
                })
            }

            LogExpr::ExternalCall { udf, args } => {
                let checked_args: Vec<Box<LogExpr>> = args.iter()
                    .map(|a| self.check(a).map(|(e, _)| Box::new(e)))
                    .collect::<Result<_, _>>()?;
                let return_type = self.schema.lookup_external_udf(udf)
                    .map(|m| m.return_type.clone())
                    .unwrap_or(ValueType::Null);
                Ok((LogExpr::ExternalCall {
                    udf: udf.clone(),
                    args: checked_args,
                }, return_type))
            }

            _ => Ok((expr.clone(), ValueType::Null)),
        }
    }

    fn check_comparison(
        &self,
        lhs: &LogExpr,
        rhs: &LogExpr,
        op: &str,
        build: impl Fn(Box<LogExpr>, Box<LogExpr>) -> LogExpr,
    ) -> TResult {
        let (lhs, lt) = self.check(lhs)?;
        let (rhs, rt) = self.check(rhs)?;
        let (lhs, rhs, _) = self.unify_pair(lhs, lt, rhs, rt, op)?;
        Ok((build(Box::new(lhs), Box::new(rhs)), ValueType::Bool))
    }

    fn check_numeric_binop(
        &self,
        lhs: &LogExpr,
        rhs: &LogExpr,
        op: &str,
        build: impl Fn(Box<LogExpr>, Box<LogExpr>) -> LogExpr,
    ) -> TResult {
        let (lhs, lt) = self.check(lhs)?;
        let (rhs, rt) = self.check(rhs)?;
        if !is_numeric(&lt) {
            return Err(TypeError { message: format!("left operand of '{op}' is {lt:?}, expected numeric") });
        }
        if !is_numeric(&rt) {
            return Err(TypeError { message: format!("right operand of '{op}' is {rt:?}, expected numeric") });
        }
        let (lhs, rhs, vt) = self.unify_pair(lhs, lt, rhs, rt, op)?;
        Ok((build(Box::new(lhs), Box::new(rhs)), vt))
    }

    fn check_string_method(
        &self,
        receiver: &LogExpr,
        arg: &LogExpr,
        method: &str,
        build: impl Fn(Box<LogExpr>, Box<LogExpr>) -> LogExpr,
    ) -> TResult {
        let (receiver, _) = self.expect_type(receiver, &ValueType::String, method)?;
        let (arg, _) = self.expect_type(arg, &ValueType::String, method)?;
        Ok((build(Box::new(receiver), Box::new(arg)), ValueType::Bool))
    }

    fn check_string_unary(
        &self,
        operand: &LogExpr,
        build: impl Fn(Box<LogExpr>) -> LogExpr,
    ) -> TResult {
        let (operand, _) = self.expect_type(operand, &ValueType::String, "string op")?;
        Ok((build(Box::new(operand)), ValueType::String))
    }

    fn expect_type(&self, expr: &LogExpr, expected: &ValueType, context: &str) -> TResult {
        let (expr, actual) = self.check(expr)?;
        if types_compatible(&actual, expected) {
            return Ok((expr, actual));
        }
        if let Some(cast) = self.try_cast(&expr, &actual, expected) {
            return Ok((cast, expected.clone()));
        }
        Err(TypeError {
            message: format!("type mismatch in '{context}': expected {expected:?}, got {actual:?}"),
        })
    }

    fn unify_pair(
        &self,
        lhs: LogExpr,
        lt: ValueType,
        rhs: LogExpr,
        rt: ValueType,
        context: &str,
    ) -> Result<(LogExpr, LogExpr, ValueType), TypeError> {
        if types_compatible(&lt, &rt) {
            return Ok((lhs, rhs, lt));
        }
        if let Some(common) = common_type(&lt, &rt, self.mode) {
            let lhs = if !types_compatible(&lt, &common) {
                self.try_cast(&lhs, &lt, &common).unwrap_or(lhs)
            } else {
                lhs
            };
            let rhs = if !types_compatible(&rt, &common) {
                self.try_cast(&rhs, &rt, &common).unwrap_or(rhs)
            } else {
                rhs
            };
            return Ok((lhs, rhs, common));
        }
        Err(TypeError {
            message: format!("incompatible types in '{context}': {lt:?} vs {rt:?}"),
        })
    }

    fn try_cast(&self, expr: &LogExpr, from: &ValueType, to: &ValueType) -> Option<LogExpr> {
        if types_compatible(from, to) {
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
            ValueType::I64 => Some(LogExpr::CastInt { operand: boxed }),
            ValueType::U64 => Some(LogExpr::CastUint { operand: boxed }),
            ValueType::F64 => Some(LogExpr::CastDouble { operand: boxed }),
            ValueType::String => Some(LogExpr::CastString { operand: boxed }),
            ValueType::Bool => Some(LogExpr::CastBool { operand: boxed }),
            ValueType::Timestamp { .. } => Some(LogExpr::CastTimestamp { operand: boxed }),
            _ => None,
        }
    }
}

fn types_compatible(a: &ValueType, b: &ValueType) -> bool {
    use ValueType::*;
    match (a, b) {
        (Timestamp { .. }, Timestamp { .. }) => true,
        (Decimal { .. }, Decimal { .. }) => true,
        _ => a == b,
    }
}

fn value_to_type(v: &Value) -> ValueType {
    match v {
        Value::Null => ValueType::Null,
        Value::Bool(_) => ValueType::Bool,
        Value::I8(_) => ValueType::I8,
        Value::I16(_) => ValueType::I16,
        Value::I32(_) => ValueType::I32,
        Value::I64(_) => ValueType::I64,
        Value::U8(_) => ValueType::U8,
        Value::U16(_) => ValueType::U16,
        Value::U32(_) => ValueType::U32,
        Value::U64(_) => ValueType::U64,
        Value::F32(_) => ValueType::F32,
        Value::F64(_) => ValueType::F64,
        Value::Date(_) => ValueType::Date,
        Value::Uuid(_) => ValueType::Uuid,
        Value::Ipv4(_) => ValueType::Ipv4,
        Value::Ipv6(_) => ValueType::Ipv6,
        Value::Blob(_) => ValueType::Blob,
        Value::Clob(_) => ValueType::Clob,
        Value::String(_) => ValueType::String,
        Value::DecimalI64(_) => ValueType::Decimal { precision: 18, scale: 0 },
        Value::DecimalI128(_) => ValueType::Decimal { precision: 38, scale: 0 },
        Value::Timestamp(_) => ValueType::Timestamp {
            precision: crate::value::TimestampPrecision::Unspecified,
            timezone: crate::value::TimestampTimezone::None,
        },
        Value::TimestampTz(_, _) => ValueType::Timestamp {
            precision: crate::value::TimestampPrecision::Unspecified,
            timezone: crate::value::TimestampTimezone::UtcOffset,
        },
        Value::Enum(_) => ValueType::Enum { values: vec![] },
        Value::Array(_) => ValueType::Array { element_type: Box::new(ValueType::Null), elements_nullable: true },
        Value::Map(_) => ValueType::Map { key_type: Box::new(ValueType::Null), value_type: Box::new(ValueType::Null), values_nullable: true },
        Value::Struct(_) => ValueType::Struct { fields: vec![] },
    }
}

fn is_numeric(vt: &ValueType) -> bool {
    matches!(
        vt,
        ValueType::I8 | ValueType::I16 | ValueType::I32 | ValueType::I64
            | ValueType::U8 | ValueType::U16 | ValueType::U32 | ValueType::U64
            | ValueType::F32 | ValueType::F64
    )
}

fn is_sizable(vt: &ValueType) -> bool {
    matches!(vt, ValueType::String | ValueType::Blob | ValueType::Array { .. })
}

fn is_safe_widening(from: &ValueType, to: &ValueType) -> bool {
    use ValueType::*;
    matches!(
        (from, to),
        (I8, I16 | I32 | I64)
            | (I16, I32 | I64)
            | (I32, I64)
            | (U8, U16 | U32 | U64 | I16 | I32 | I64)
            | (U16, U32 | U64 | I32 | I64)
            | (U32, U64 | I64)
            | (F32, F64)
            | (I8 | I16 | I32, F64)
            | (U8 | U16 | U32, F64)
            | (I64 | U64, F64)
    )
}

fn is_aggressive_cast(from: &ValueType, to: &ValueType) -> bool {
    use ValueType::*;
    matches!(
        (from, to),
        (I64, I32 | I16 | I8)
            | (I32, I16 | I8)
            | (I16, I8)
            | (U64, U32 | U16 | U8)
            | (U32, U16 | U8)
            | (U16, U8)
            | (F64, I64 | I32)
            | (F32, I64 | I32)
            | (I64, U64)
            | (U64, I64)
            | (String, I64 | F64 | Bool)
    )
}

fn common_type(a: &ValueType, b: &ValueType, mode: AutoCastMode) -> Option<ValueType> {
    if types_compatible(a, b) {
        return Some(a.clone());
    }

    let check_widening = |from: &ValueType, to: &ValueType| match mode {
        AutoCastMode::Off => false,
        AutoCastMode::Safe => is_safe_widening(from, to),
        AutoCastMode::Aggressive => is_safe_widening(from, to) || is_aggressive_cast(from, to),
    };

    if check_widening(a, b) {
        return Some(b.clone());
    }
    if check_widening(b, a) {
        return Some(a.clone());
    }

    if is_numeric(a) && is_numeric(b) && mode != AutoCastMode::Off {
        return Some(ValueType::F64);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::StructField;

    fn schema(fields: Vec<(&str, ValueType)>) -> ExprSchema {
        ExprSchema {
            event_fields: fields
                .into_iter()
                .map(|(name, value_type)| FieldSchema {
                    name: name.to_string(),
                    value_type,
                    nullable: false,
                })
                .collect(),
            enrichment_fields: vec![],
            external_udfs: vec![],
        }
    }

    fn checker(mode: AutoCastMode, fields: Vec<(&str, ValueType)>) -> TypeChecker {
        TypeChecker::new(mode, schema(fields))
    }

    #[test]
    fn same_type_comparison() {
        let tc = checker(AutoCastMode::Safe, vec![("age", ValueType::I64)]);
        let expr = LogExpr::GreaterThan {
            lhs: Box::new(LogExpr::GetFieldByName { field_name: "age".into() }),
            rhs: Box::new(LogExpr::Literal(Value::I64(18))),
        };
        let (result, vt) = tc.check(&expr).unwrap();
        assert_eq!(vt, ValueType::Bool);
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
            vec![("qty", ValueType::I64), ("price", ValueType::F64)],
        );
        let expr = LogExpr::Multiply {
            lhs: Box::new(LogExpr::GetFieldByName { field_name: "qty".into() }),
            rhs: Box::new(LogExpr::GetFieldByName { field_name: "price".into() }),
        };
        let (result, vt) = tc.check(&expr).unwrap();
        assert_eq!(vt, ValueType::F64);
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
            vec![("qty", ValueType::I64), ("price", ValueType::F64)],
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
            vec![("flag", ValueType::Bool)],
        );
        let expr = LogExpr::Conditional {
            condition: Box::new(LogExpr::GetFieldByName { field_name: "flag".into() }),
            then_expr: Box::new(LogExpr::Literal(Value::I64(1))),
            else_expr: Box::new(LogExpr::Literal(Value::F64(2.5))),
        };
        let (result, vt) = tc.check(&expr).unwrap();
        assert_eq!(vt, ValueType::F64);
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
            vec![("name", ValueType::String), ("age", ValueType::I64)],
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
            vec![("code", ValueType::String)],
        );
        let expr = LogExpr::Equal {
            lhs: Box::new(LogExpr::GetFieldByName { field_name: "code".into() }),
            rhs: Box::new(LogExpr::Literal(Value::I64(404))),
        };
        let (result, vt) = tc.check(&expr).unwrap();
        assert_eq!(vt, ValueType::Bool);
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
            vec![("code", ValueType::String)],
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
            vec![("small", ValueType::I32), ("big", ValueType::I64)],
        );
        let expr = LogExpr::Add {
            lhs: Box::new(LogExpr::GetFieldByName { field_name: "small".into() }),
            rhs: Box::new(LogExpr::GetFieldByName { field_name: "big".into() }),
        };
        let (result, vt) = tc.check(&expr).unwrap();
        assert_eq!(vt, ValueType::I64);
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
            vec![("x", ValueType::I64)],
        );
        let expr = LogExpr::LogicalAnd {
            lhs: Box::new(LogExpr::GetFieldByName { field_name: "x".into() }),
            rhs: Box::new(LogExpr::Literal(Value::Bool(true))),
        };
        assert!(tc.check(&expr).is_err());
    }

    #[test]
    fn size_on_string() {
        let tc = checker(AutoCastMode::Safe, vec![("name", ValueType::String)]);
        let expr = LogExpr::Size {
            operand: Box::new(LogExpr::GetFieldByName { field_name: "name".into() }),
        };
        let (_, vt) = tc.check(&expr).unwrap();
        assert_eq!(vt, ValueType::I64);
    }

    #[test]
    fn size_on_int_fails() {
        let tc = checker(AutoCastMode::Safe, vec![("x", ValueType::I64)]);
        let expr = LogExpr::Size {
            operand: Box::new(LogExpr::GetFieldByName { field_name: "x".into() }),
        };
        assert!(tc.check(&expr).is_err());
    }

    fn struct_schema() -> ExprSchema {
        ExprSchema {
            event_fields: vec![
                FieldSchema {
                    name: "payload".into(),
                    value_type: ValueType::Struct {
                        fields: vec![
                            StructField {
                                name: "browser".into(),
                                human_name: "Browser".into(),
                                value_type: ValueType::String,
                                nullable: false,
                            },
                            StructField {
                                name: "os".into(),
                                human_name: "OS".into(),
                                value_type: ValueType::String,
                                nullable: false,
                            },
                            StructField {
                                name: "status_code".into(),
                                human_name: "Status Code".into(),
                                value_type: ValueType::I64,
                                nullable: false,
                            },
                        ],
                    },
                    nullable: false,
                },
                FieldSchema {
                    name: "ts".into(),
                    value_type: ValueType::I64,
                    nullable: false,
                },
            ],
            enrichment_fields: vec![],
            external_udfs: vec![],
        }
    }

    #[test]
    fn get_child_by_name_resolves_to_index() {
        let tc = TypeChecker::new(AutoCastMode::Safe, struct_schema());
        let expr = LogExpr::GetChildByName {
            child_name: "os".into(),
            operand: Box::new(LogExpr::GetFieldByName { field_name: "payload".into() }),
        };
        let (result, vt) = tc.check(&expr).unwrap();
        assert_eq!(vt, ValueType::String);
        match &result {
            LogExpr::GetChildByIndex { child_index, lhs, .. } => {
                assert_eq!(*child_index, 1, "os is at index 1");
                assert!(matches!(**lhs, LogExpr::GetFieldByName { .. }));
            }
            other => panic!("expected GetChildByIndex, got {other:?}"),
        }
    }

    #[test]
    fn get_child_by_name_numeric_field() {
        let tc = TypeChecker::new(AutoCastMode::Safe, struct_schema());
        let expr = LogExpr::GetChildByName {
            child_name: "status_code".into(),
            operand: Box::new(LogExpr::GetFieldByName { field_name: "payload".into() }),
        };
        let (_, vt) = tc.check(&expr).unwrap();
        assert_eq!(vt, ValueType::I64);
    }

    #[test]
    fn get_child_by_name_unknown_field_errors() {
        let tc = TypeChecker::new(AutoCastMode::Safe, struct_schema());
        let expr = LogExpr::GetChildByName {
            child_name: "missing".into(),
            operand: Box::new(LogExpr::GetFieldByName { field_name: "payload".into() }),
        };
        let err = tc.check(&expr).unwrap_err();
        assert!(err.message.contains("no field 'missing'"), "got: {}", err.message);
    }

    #[test]
    fn get_child_by_name_non_struct_passes_through() {
        let tc = checker(AutoCastMode::Safe, vec![("data", ValueType::String)]);
        let expr = LogExpr::GetChildByName {
            child_name: "x".into(),
            operand: Box::new(LogExpr::GetFieldByName { field_name: "data".into() }),
        };
        let (result, vt) = tc.check(&expr).unwrap();
        assert_eq!(vt, ValueType::Null);
        assert!(matches!(result, LogExpr::GetChildByName { .. }));
    }
}
