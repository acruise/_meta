use std::collections::HashMap;

use meta_types::external_fn::UdfImport;
use crate::expr_gen::LogExpr;
use crate::value::Value;

#[derive(Debug, Clone)]
pub struct ResolveError {
    pub message: String,
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Resolution context: the import list plus, when known, each visible
/// name's full parameter count (from the catalog). Calls with fewer
/// args than the count are padded with trailing Null literals: the
/// lowering that resolves arity overloads (optional trailing
/// parameters) to the single physical ExternalCall shape.
struct Ctx<'a> {
    imports: &'a [UdfImport],
    arities: &'a HashMap<String, usize>,
}

pub fn resolve(expr: &LogExpr, imports: &[UdfImport]) -> Result<LogExpr, ResolveError> {
    resolve_with_arities(expr, imports, &HashMap::new())
}

pub fn resolve_with_arities(
    expr: &LogExpr,
    imports: &[UdfImport],
    arities: &HashMap<String, usize>,
) -> Result<LogExpr, ResolveError> {
    resolve_inner(expr, &Ctx { imports, arities })
}

fn resolve_inner(expr: &LogExpr, ctx: &Ctx<'_>) -> Result<LogExpr, ResolveError> {
    match expr {
        LogExpr::UnresolvedCall { name, args } => {
            let matches: Vec<&UdfImport> = ctx.imports.iter()
                .filter(|imp| imp.visible_name() == name)
                .collect();
            match matches.len() {
                0 => Err(ResolveError {
                    message: format!("no import matches unresolved call '{name}'"),
                }),
                1 => {
                    let imp = matches[0];
                    let mut resolved_args = args.iter()
                        .map(|a| resolve_inner(a, ctx).map(Box::new))
                        .collect::<Result<Vec<_>, _>>()?;
                    if let Some(&arity) = ctx.arities.get(name) {
                        while resolved_args.len() < arity {
                            resolved_args.push(Box::new(LogExpr::Literal(Value::Null)));
                        }
                    }
                    Ok(LogExpr::ExternalCall {
                        udf: imp.to_resolved(),
                        args: resolved_args,
                    })
                }
                n => Err(ResolveError {
                    message: format!(
                        "ambiguous call '{name}': {n} imports match ({})",
                        matches.iter()
                            .map(|m| format!("{}:{}:{}", m.namespace, m.version, m.cel_name))
                            .collect::<Vec<_>>()
                            .join(", "),
                    ),
                }),
            }
        }

        LogExpr::ExternalCall { udf, args } => {
            let resolved_args = args.iter()
                .map(|a| resolve_inner(a, ctx).map(Box::new))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(LogExpr::ExternalCall { udf: udf.clone(), args: resolved_args })
        }

        LogExpr::Literal(_)
        | LogExpr::GetFieldByName { .. }
        | LogExpr::GetFieldByIndex { .. }
        | LogExpr::CurrentTimestamp => Ok(expr.clone()),

        LogExpr::LogicalOr { lhs, rhs } => Ok(LogExpr::LogicalOr { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::LogicalAnd { lhs, rhs } => Ok(LogExpr::LogicalAnd { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::LogicalNot { operand } => Ok(LogExpr::LogicalNot { operand: r(operand, ctx)? }),
        LogExpr::Equal { lhs, rhs } => Ok(LogExpr::Equal { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::NotEqual { lhs, rhs } => Ok(LogExpr::NotEqual { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::LessThan { lhs, rhs } => Ok(LogExpr::LessThan { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::LessOrEqual { lhs, rhs } => Ok(LogExpr::LessOrEqual { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::GreaterThan { lhs, rhs } => Ok(LogExpr::GreaterThan { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::GreaterOrEqual { lhs, rhs } => Ok(LogExpr::GreaterOrEqual { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::Between { arg0, arg1, arg2 } => Ok(LogExpr::Between { arg0: r(arg0, ctx)?, arg1: r(arg1, ctx)?, arg2: r(arg2, ctx)? }),
        LogExpr::NullSafeEqual { lhs, rhs } => Ok(LogExpr::NullSafeEqual { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::NullSafeNotEqual { lhs, rhs } => Ok(LogExpr::NullSafeNotEqual { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::IsNull { operand } => Ok(LogExpr::IsNull { operand: r(operand, ctx)? }),
        LogExpr::IsNotNull { operand } => Ok(LogExpr::IsNotNull { operand: r(operand, ctx)? }),
        LogExpr::IsNan { operand } => Ok(LogExpr::IsNan { operand: r(operand, ctx)? }),
        LogExpr::IsFinite { operand } => Ok(LogExpr::IsFinite { operand: r(operand, ctx)? }),
        LogExpr::IsInfinite { operand } => Ok(LogExpr::IsInfinite { operand: r(operand, ctx)? }),
        LogExpr::Coalesce { lhs, rhs } => Ok(LogExpr::Coalesce { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::TryOrElse { lhs, rhs } => Ok(LogExpr::TryOrElse { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::RaiseError { operand } => Ok(LogExpr::RaiseError { operand: r(operand, ctx)? }),
        LogExpr::Least { lhs, rhs } => Ok(LogExpr::Least { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::Greatest { lhs, rhs } => Ok(LogExpr::Greatest { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::Add { lhs, rhs } => Ok(LogExpr::Add { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::Subtract { lhs, rhs } => Ok(LogExpr::Subtract { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::Multiply { lhs, rhs } => Ok(LogExpr::Multiply { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::Divide { lhs, rhs } => Ok(LogExpr::Divide { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::Negate { operand } => Ok(LogExpr::Negate { operand: r(operand, ctx)? }),
        LogExpr::Modulus { lhs, rhs } => Ok(LogExpr::Modulus { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::Abs { operand } => Ok(LogExpr::Abs { operand: r(operand, ctx)? }),
        LogExpr::Power { lhs, rhs } => Ok(LogExpr::Power { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::Sqrt { operand } => Ok(LogExpr::Sqrt { operand: r(operand, ctx)? }),
        LogExpr::Exp { operand } => Ok(LogExpr::Exp { operand: r(operand, ctx)? }),
        LogExpr::Sign { operand } => Ok(LogExpr::Sign { operand: r(operand, ctx)? }),
        LogExpr::Contains { receiver, arg } => Ok(LogExpr::Contains { receiver: r(receiver, ctx)?, arg: r(arg, ctx)? }),
        LogExpr::StartsWith { receiver, arg } => Ok(LogExpr::StartsWith { receiver: r(receiver, ctx)?, arg: r(arg, ctx)? }),
        LogExpr::EndsWith { receiver, arg } => Ok(LogExpr::EndsWith { receiver: r(receiver, ctx)?, arg: r(arg, ctx)? }),
        LogExpr::RegexMatch { receiver, arg } => Ok(LogExpr::RegexMatch { receiver: r(receiver, ctx)?, arg: r(arg, ctx)? }),
        LogExpr::RegexExtract { lhs, rhs } => Ok(LogExpr::RegexExtract { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::RegexReplace { arg0, arg1, arg2 } => Ok(LogExpr::RegexReplace { arg0: r(arg0, ctx)?, arg1: r(arg1, ctx)?, arg2: r(arg2, ctx)? }),
        LogExpr::UrlPathSegment { arg0, arg1, arg2 } => Ok(LogExpr::UrlPathSegment { arg0: r(arg0, ctx)?, arg1: r(arg1, ctx)?, arg2: r(arg2, ctx)? }),
        LogExpr::UrlQueryParam { arg0, arg1, arg2 } => Ok(LogExpr::UrlQueryParam { arg0: r(arg0, ctx)?, arg1: r(arg1, ctx)?, arg2: r(arg2, ctx)? }),
        LogExpr::Size { operand } => Ok(LogExpr::Size { operand: r(operand, ctx)? }),
        LogExpr::Lower { operand } => Ok(LogExpr::Lower { operand: r(operand, ctx)? }),
        LogExpr::Upper { operand } => Ok(LogExpr::Upper { operand: r(operand, ctx)? }),
        LogExpr::Substring { arg0, arg1, arg2 } => Ok(LogExpr::Substring { arg0: r(arg0, ctx)?, arg1: r(arg1, ctx)?, arg2: r(arg2, ctx)? }),
        LogExpr::Replace { arg0, arg1, arg2 } => Ok(LogExpr::Replace { arg0: r(arg0, ctx)?, arg1: r(arg1, ctx)?, arg2: r(arg2, ctx)? }),
        LogExpr::Trim { operand } => Ok(LogExpr::Trim { operand: r(operand, ctx)? }),
        LogExpr::StringSplit { lhs, rhs } => Ok(LogExpr::StringSplit { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::StringPosition { lhs, rhs } => Ok(LogExpr::StringPosition { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::Concat { lhs, rhs } => Ok(LogExpr::Concat { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::TimestampExtract { operand } => Ok(LogExpr::TimestampExtract { operand: r(operand, ctx)? }),
        LogExpr::RoundTemporal { operand } => Ok(LogExpr::RoundTemporal { operand: r(operand, ctx)? }),
        LogExpr::RoundCalendar { operand } => Ok(LogExpr::RoundCalendar { operand: r(operand, ctx)? }),
        LogExpr::CastBool { operand } => Ok(LogExpr::CastBool { operand: r(operand, ctx)? }),
        LogExpr::CastInt { operand } => Ok(LogExpr::CastInt { operand: r(operand, ctx)? }),
        LogExpr::CastUint { operand } => Ok(LogExpr::CastUint { operand: r(operand, ctx)? }),
        LogExpr::CastDouble { operand } => Ok(LogExpr::CastDouble { operand: r(operand, ctx)? }),
        LogExpr::CastString { operand } => Ok(LogExpr::CastString { operand: r(operand, ctx)? }),
        LogExpr::CastBytes { operand } => Ok(LogExpr::CastBytes { operand: r(operand, ctx)? }),
        LogExpr::CastDuration { operand } => Ok(LogExpr::CastDuration { operand: r(operand, ctx)? }),
        LogExpr::CastTimestamp { operand } => Ok(LogExpr::CastTimestamp { operand: r(operand, ctx)? }),
        LogExpr::TypeOf { operand } => Ok(LogExpr::TypeOf { operand: r(operand, ctx)? }),
        LogExpr::Dyn { operand } => Ok(LogExpr::Dyn { operand: r(operand, ctx)? }),
        LogExpr::Conditional { condition, then_expr, else_expr } => Ok(LogExpr::Conditional {
            condition: r(condition, ctx)?,
            then_expr: r(then_expr, ctx)?,
            else_expr: r(else_expr, ctx)?,
        }),
        LogExpr::Index { lhs, rhs } => Ok(LogExpr::Index { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::In { lhs, rhs } => Ok(LogExpr::In { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::Ln { operand } => Ok(LogExpr::Ln { operand: r(operand, ctx)? }),
        LogExpr::Log10 { operand } => Ok(LogExpr::Log10 { operand: r(operand, ctx)? }),
        LogExpr::Ceil { operand } => Ok(LogExpr::Ceil { operand: r(operand, ctx)? }),
        LogExpr::Floor { operand } => Ok(LogExpr::Floor { operand: r(operand, ctx)? }),
        LogExpr::Round { operand } => Ok(LogExpr::Round { operand: r(operand, ctx)? }),
        LogExpr::JsonParse { operand } => Ok(LogExpr::JsonParse { operand: r(operand, ctx)? }),
        LogExpr::JsonParseStruct { operand } => Ok(LogExpr::JsonParseStruct { operand: r(operand, ctx)? }),
        LogExpr::JsonStringify { operand } => Ok(LogExpr::JsonStringify { operand: r(operand, ctx)? }),
        LogExpr::JsonExtract { lhs, rhs } => Ok(LogExpr::JsonExtract { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::JsonExtractString { lhs, rhs } => Ok(LogExpr::JsonExtractString { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::CidrContains { lhs, rhs } => Ok(LogExpr::CidrContains { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::CidrMatch { lhs, rhs } => Ok(LogExpr::CidrMatch { lhs: r(lhs, ctx)?, rhs: r(rhs, ctx)? }),
        LogExpr::IpToInt { operand } => Ok(LogExpr::IpToInt { operand: r(operand, ctx)? }),
        LogExpr::IntToIp { operand } => Ok(LogExpr::IntToIp { operand: r(operand, ctx)? }),
        LogExpr::Has { operand } => Ok(LogExpr::Has { operand: r(operand, ctx)? }),
        LogExpr::GetChildByName { child_name, operand } => Ok(LogExpr::GetChildByName {
            child_name: child_name.clone(),
            operand: r(operand, ctx)?,
        }),
        LogExpr::GetChildByIndex { child_index, lhs, rhs } => Ok(LogExpr::GetChildByIndex {
            child_index: *child_index,
            lhs: r(lhs, ctx)?,
            rhs: r(rhs, ctx)?,
        }),
        LogExpr::All { collection, binding, body } => Ok(LogExpr::All { collection: r(collection, ctx)?, binding: binding.clone(), body: r(body, ctx)? }),
        LogExpr::Exists { collection, binding, body } => Ok(LogExpr::Exists { collection: r(collection, ctx)?, binding: binding.clone(), body: r(body, ctx)? }),
        LogExpr::ExistsOne { collection, binding, body } => Ok(LogExpr::ExistsOne { collection: r(collection, ctx)?, binding: binding.clone(), body: r(body, ctx)? }),
        LogExpr::Filter { collection, binding, body } => Ok(LogExpr::Filter { collection: r(collection, ctx)?, binding: binding.clone(), body: r(body, ctx)? }),
        LogExpr::MapTransform { collection, binding, body } => Ok(LogExpr::MapTransform { collection: r(collection, ctx)?, binding: binding.clone(), body: r(body, ctx)? }),
        LogExpr::CelFallback { source, args } => {
            let resolved_args = args.iter()
                .map(|a| resolve_inner(a, ctx).map(Box::new))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(LogExpr::CelFallback { source: source.clone(), args: resolved_args })
        }
        LogExpr::Case { arms, default } => {
            let resolved_arms = arms.iter()
                .map(|(c, v)| Ok((r(c, ctx)?, r(v, ctx)?)))
                .collect::<Result<Vec<_>, ResolveError>>()?;
            Ok(LogExpr::Case { arms: resolved_arms, default: r(default, ctx)? })
        }
    }
}

fn r(expr: &LogExpr, ctx: &Ctx<'_>) -> Result<Box<LogExpr>, ResolveError> {
    resolve_inner(expr, ctx).map(Box::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Value;
    use meta_types::external_fn::ResolvedUdfRef;

    fn test_imports() -> Vec<UdfImport> {
        vec![
            UdfImport {
                namespace: "io.example.udf.useragent".into(),
                version: "1".into(),
                cel_name: "ua_parse".into(),
                alias: None,
            },
            UdfImport {
                namespace: "io.example.udf.url".into(),
                version: "1".into(),
                cel_name: "url_parse".into(),
                alias: Some("parse_url".into()),
            },
        ]
    }

    #[test]
    fn resolves_by_cel_name() {
        let imports = test_imports();
        let expr = LogExpr::UnresolvedCall {
            name: "ua_parse".into(),
            args: vec![Box::new(LogExpr::GetFieldByName { field_name: "ua".into() })],
        };
        let resolved = resolve(&expr, &imports).unwrap();
        match &resolved {
            LogExpr::ExternalCall { udf, args } => {
                assert_eq!(udf.namespace, "io.example.udf.useragent");
                assert_eq!(udf.version, "1");
                assert_eq!(udf.cel_name, "ua_parse");
                assert_eq!(args.len(), 1);
            }
            other => panic!("expected ExternalCall, got {other:?}"),
        }
    }

    #[test]
    fn resolves_by_alias() {
        let imports = test_imports();
        let expr = LogExpr::UnresolvedCall {
            name: "parse_url".into(),
            args: vec![Box::new(LogExpr::Literal(Value::String("http://x".into())))],
        };
        let resolved = resolve(&expr, &imports).unwrap();
        match &resolved {
            LogExpr::ExternalCall { udf, .. } => {
                assert_eq!(udf.namespace, "io.example.udf.url");
                assert_eq!(udf.cel_name, "url_parse");
            }
            other => panic!("expected ExternalCall, got {other:?}"),
        }
    }

    #[test]
    fn unresolved_name_errors() {
        let imports = test_imports();
        let expr = LogExpr::UnresolvedCall {
            name: "unknown_fn".into(),
            args: vec![],
        };
        let err = resolve(&expr, &imports).unwrap_err();
        assert!(err.message.contains("no import"), "got: {}", err.message);
    }

    #[test]
    fn ambiguous_name_errors() {
        let imports = vec![
            UdfImport {
                namespace: "ns1".into(),
                version: "1".into(),
                cel_name: "foo".into(),
                alias: None,
            },
            UdfImport {
                namespace: "ns2".into(),
                version: "1".into(),
                cel_name: "foo".into(),
                alias: None,
            },
        ];
        let expr = LogExpr::UnresolvedCall { name: "foo".into(), args: vec![] };
        let err = resolve(&expr, &imports).unwrap_err();
        assert!(err.message.contains("ambiguous"), "got: {}", err.message);
    }

    #[test]
    fn resolves_nested() {
        let imports = test_imports();
        let expr = LogExpr::GetChildByName {
            child_name: "ua_family".into(),
            operand: Box::new(LogExpr::UnresolvedCall {
                name: "ua_parse".into(),
                args: vec![Box::new(LogExpr::GetFieldByName { field_name: "ua".into() })],
            }),
        };
        let resolved = resolve(&expr, &imports).unwrap();
        match &resolved {
            LogExpr::GetChildByName { operand, .. } => {
                assert!(matches!(operand.as_ref(), LogExpr::ExternalCall { .. }));
            }
            other => panic!("expected GetChildByName, got {other:?}"),
        }
    }

    #[test]
    fn pads_missing_trailing_args_to_arity() {
        let imports = test_imports();
        let mut arities = HashMap::new();
        arities.insert("ua_parse".to_string(), 3usize);
        let expr = LogExpr::UnresolvedCall {
            name: "ua_parse".into(),
            args: vec![Box::new(LogExpr::GetFieldByName { field_name: "ua".into() })],
        };
        let resolved = resolve_with_arities(&expr, &imports, &arities).unwrap();
        match &resolved {
            LogExpr::ExternalCall { args, .. } => {
                assert_eq!(args.len(), 3);
                assert_eq!(args[1].as_ref(), &LogExpr::Literal(Value::Null));
                assert_eq!(args[2].as_ref(), &LogExpr::Literal(Value::Null));
            }
            other => panic!("expected ExternalCall, got {other:?}"),
        }
        // Without arity info, no padding.
        let resolved = resolve(&expr, &imports).unwrap();
        match &resolved {
            LogExpr::ExternalCall { args, .. } => assert_eq!(args.len(), 1),
            other => panic!("expected ExternalCall, got {other:?}"),
        }
    }

    #[test]
    fn no_imports_passes_through_non_udf() {
        let expr = LogExpr::GreaterThan {
            lhs: Box::new(LogExpr::GetFieldByName { field_name: "age".into() }),
            rhs: Box::new(LogExpr::Literal(Value::I64(18))),
        };
        let resolved = resolve(&expr, &[]).unwrap();
        assert_eq!(resolved, expr);
    }
}
