use meta_types::external_fn::UdfImport;
use crate::expr_gen::LogExpr;

#[derive(Debug, Clone)]
pub struct ResolveError {
    pub message: String,
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

pub fn resolve(expr: &LogExpr, imports: &[UdfImport]) -> Result<LogExpr, ResolveError> {
    match expr {
        LogExpr::UnresolvedCall { name, args } => {
            let matches: Vec<&UdfImport> = imports.iter()
                .filter(|imp| imp.visible_name() == name)
                .collect();
            match matches.len() {
                0 => Err(ResolveError {
                    message: format!("no import matches unresolved call '{name}'"),
                }),
                1 => {
                    let imp = matches[0];
                    let resolved_args = args.iter()
                        .map(|a| resolve(a, imports).map(Box::new))
                        .collect::<Result<Vec<_>, _>>()?;
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
                .map(|a| resolve(a, imports).map(Box::new))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(LogExpr::ExternalCall { udf: udf.clone(), args: resolved_args })
        }

        LogExpr::Literal(_)
        | LogExpr::GetFieldByName { .. }
        | LogExpr::GetFieldByIndex { .. }
        | LogExpr::CurrentTimestamp => Ok(expr.clone()),

        LogExpr::LogicalOr { lhs, rhs } => Ok(LogExpr::LogicalOr { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::LogicalAnd { lhs, rhs } => Ok(LogExpr::LogicalAnd { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::LogicalNot { operand } => Ok(LogExpr::LogicalNot { operand: r(operand, imports)? }),
        LogExpr::Equal { lhs, rhs } => Ok(LogExpr::Equal { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::NotEqual { lhs, rhs } => Ok(LogExpr::NotEqual { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::LessThan { lhs, rhs } => Ok(LogExpr::LessThan { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::LessOrEqual { lhs, rhs } => Ok(LogExpr::LessOrEqual { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::GreaterThan { lhs, rhs } => Ok(LogExpr::GreaterThan { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::GreaterOrEqual { lhs, rhs } => Ok(LogExpr::GreaterOrEqual { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::Between { arg0, arg1, arg2 } => Ok(LogExpr::Between { arg0: r(arg0, imports)?, arg1: r(arg1, imports)?, arg2: r(arg2, imports)? }),
        LogExpr::NullSafeEqual { lhs, rhs } => Ok(LogExpr::NullSafeEqual { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::NullSafeNotEqual { lhs, rhs } => Ok(LogExpr::NullSafeNotEqual { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::IsNull { operand } => Ok(LogExpr::IsNull { operand: r(operand, imports)? }),
        LogExpr::IsNotNull { operand } => Ok(LogExpr::IsNotNull { operand: r(operand, imports)? }),
        LogExpr::IsNan { operand } => Ok(LogExpr::IsNan { operand: r(operand, imports)? }),
        LogExpr::IsFinite { operand } => Ok(LogExpr::IsFinite { operand: r(operand, imports)? }),
        LogExpr::IsInfinite { operand } => Ok(LogExpr::IsInfinite { operand: r(operand, imports)? }),
        LogExpr::Coalesce { lhs, rhs } => Ok(LogExpr::Coalesce { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::TryOrElse { lhs, rhs } => Ok(LogExpr::TryOrElse { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::RaiseError { operand } => Ok(LogExpr::RaiseError { operand: r(operand, imports)? }),
        LogExpr::Least { lhs, rhs } => Ok(LogExpr::Least { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::Greatest { lhs, rhs } => Ok(LogExpr::Greatest { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::Add { lhs, rhs } => Ok(LogExpr::Add { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::Subtract { lhs, rhs } => Ok(LogExpr::Subtract { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::Multiply { lhs, rhs } => Ok(LogExpr::Multiply { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::Divide { lhs, rhs } => Ok(LogExpr::Divide { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::Negate { operand } => Ok(LogExpr::Negate { operand: r(operand, imports)? }),
        LogExpr::Modulus { lhs, rhs } => Ok(LogExpr::Modulus { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::Abs { operand } => Ok(LogExpr::Abs { operand: r(operand, imports)? }),
        LogExpr::Power { lhs, rhs } => Ok(LogExpr::Power { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::Sqrt { operand } => Ok(LogExpr::Sqrt { operand: r(operand, imports)? }),
        LogExpr::Exp { operand } => Ok(LogExpr::Exp { operand: r(operand, imports)? }),
        LogExpr::Sign { operand } => Ok(LogExpr::Sign { operand: r(operand, imports)? }),
        LogExpr::Contains { receiver, arg } => Ok(LogExpr::Contains { receiver: r(receiver, imports)?, arg: r(arg, imports)? }),
        LogExpr::StartsWith { receiver, arg } => Ok(LogExpr::StartsWith { receiver: r(receiver, imports)?, arg: r(arg, imports)? }),
        LogExpr::EndsWith { receiver, arg } => Ok(LogExpr::EndsWith { receiver: r(receiver, imports)?, arg: r(arg, imports)? }),
        LogExpr::RegexMatch { receiver, arg } => Ok(LogExpr::RegexMatch { receiver: r(receiver, imports)?, arg: r(arg, imports)? }),
        LogExpr::RegexExtract { lhs, rhs } => Ok(LogExpr::RegexExtract { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::RegexReplace { arg0, arg1, arg2 } => Ok(LogExpr::RegexReplace { arg0: r(arg0, imports)?, arg1: r(arg1, imports)?, arg2: r(arg2, imports)? }),
        LogExpr::Size { operand } => Ok(LogExpr::Size { operand: r(operand, imports)? }),
        LogExpr::Lower { operand } => Ok(LogExpr::Lower { operand: r(operand, imports)? }),
        LogExpr::Upper { operand } => Ok(LogExpr::Upper { operand: r(operand, imports)? }),
        LogExpr::Substring { arg0, arg1, arg2 } => Ok(LogExpr::Substring { arg0: r(arg0, imports)?, arg1: r(arg1, imports)?, arg2: r(arg2, imports)? }),
        LogExpr::Replace { arg0, arg1, arg2 } => Ok(LogExpr::Replace { arg0: r(arg0, imports)?, arg1: r(arg1, imports)?, arg2: r(arg2, imports)? }),
        LogExpr::Trim { operand } => Ok(LogExpr::Trim { operand: r(operand, imports)? }),
        LogExpr::StringSplit { lhs, rhs } => Ok(LogExpr::StringSplit { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::StringPosition { lhs, rhs } => Ok(LogExpr::StringPosition { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::Concat { lhs, rhs } => Ok(LogExpr::Concat { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::TimestampExtract { operand } => Ok(LogExpr::TimestampExtract { operand: r(operand, imports)? }),
        LogExpr::RoundTemporal { operand } => Ok(LogExpr::RoundTemporal { operand: r(operand, imports)? }),
        LogExpr::RoundCalendar { operand } => Ok(LogExpr::RoundCalendar { operand: r(operand, imports)? }),
        LogExpr::CastBool { operand } => Ok(LogExpr::CastBool { operand: r(operand, imports)? }),
        LogExpr::CastInt { operand } => Ok(LogExpr::CastInt { operand: r(operand, imports)? }),
        LogExpr::CastUint { operand } => Ok(LogExpr::CastUint { operand: r(operand, imports)? }),
        LogExpr::CastDouble { operand } => Ok(LogExpr::CastDouble { operand: r(operand, imports)? }),
        LogExpr::CastString { operand } => Ok(LogExpr::CastString { operand: r(operand, imports)? }),
        LogExpr::CastBytes { operand } => Ok(LogExpr::CastBytes { operand: r(operand, imports)? }),
        LogExpr::CastDuration { operand } => Ok(LogExpr::CastDuration { operand: r(operand, imports)? }),
        LogExpr::CastTimestamp { operand } => Ok(LogExpr::CastTimestamp { operand: r(operand, imports)? }),
        LogExpr::TypeOf { operand } => Ok(LogExpr::TypeOf { operand: r(operand, imports)? }),
        LogExpr::Dyn { operand } => Ok(LogExpr::Dyn { operand: r(operand, imports)? }),
        LogExpr::Conditional { condition, then_expr, else_expr } => Ok(LogExpr::Conditional {
            condition: r(condition, imports)?,
            then_expr: r(then_expr, imports)?,
            else_expr: r(else_expr, imports)?,
        }),
        LogExpr::Index { lhs, rhs } => Ok(LogExpr::Index { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::In { lhs, rhs } => Ok(LogExpr::In { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::Ln { operand } => Ok(LogExpr::Ln { operand: r(operand, imports)? }),
        LogExpr::Log10 { operand } => Ok(LogExpr::Log10 { operand: r(operand, imports)? }),
        LogExpr::Ceil { operand } => Ok(LogExpr::Ceil { operand: r(operand, imports)? }),
        LogExpr::Floor { operand } => Ok(LogExpr::Floor { operand: r(operand, imports)? }),
        LogExpr::Round { operand } => Ok(LogExpr::Round { operand: r(operand, imports)? }),
        LogExpr::JsonParse { operand } => Ok(LogExpr::JsonParse { operand: r(operand, imports)? }),
        LogExpr::JsonParseStruct { operand } => Ok(LogExpr::JsonParseStruct { operand: r(operand, imports)? }),
        LogExpr::JsonStringify { operand } => Ok(LogExpr::JsonStringify { operand: r(operand, imports)? }),
        LogExpr::JsonExtract { lhs, rhs } => Ok(LogExpr::JsonExtract { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::JsonExtractString { lhs, rhs } => Ok(LogExpr::JsonExtractString { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::CidrContains { lhs, rhs } => Ok(LogExpr::CidrContains { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::CidrMatch { lhs, rhs } => Ok(LogExpr::CidrMatch { lhs: r(lhs, imports)?, rhs: r(rhs, imports)? }),
        LogExpr::IpToInt { operand } => Ok(LogExpr::IpToInt { operand: r(operand, imports)? }),
        LogExpr::IntToIp { operand } => Ok(LogExpr::IntToIp { operand: r(operand, imports)? }),
        LogExpr::Has { operand } => Ok(LogExpr::Has { operand: r(operand, imports)? }),
        LogExpr::GetChildByName { child_name, operand } => Ok(LogExpr::GetChildByName {
            child_name: child_name.clone(),
            operand: r(operand, imports)?,
        }),
        LogExpr::GetChildByIndex { child_index, lhs, rhs } => Ok(LogExpr::GetChildByIndex {
            child_index: *child_index,
            lhs: r(lhs, imports)?,
            rhs: r(rhs, imports)?,
        }),
        LogExpr::All { collection, binding, body } => Ok(LogExpr::All { collection: r(collection, imports)?, binding: binding.clone(), body: r(body, imports)? }),
        LogExpr::Exists { collection, binding, body } => Ok(LogExpr::Exists { collection: r(collection, imports)?, binding: binding.clone(), body: r(body, imports)? }),
        LogExpr::ExistsOne { collection, binding, body } => Ok(LogExpr::ExistsOne { collection: r(collection, imports)?, binding: binding.clone(), body: r(body, imports)? }),
        LogExpr::Filter { collection, binding, body } => Ok(LogExpr::Filter { collection: r(collection, imports)?, binding: binding.clone(), body: r(body, imports)? }),
        LogExpr::MapTransform { collection, binding, body } => Ok(LogExpr::MapTransform { collection: r(collection, imports)?, binding: binding.clone(), body: r(body, imports)? }),
        LogExpr::CelFallback { source, args } => {
            let resolved_args = args.iter()
                .map(|a| resolve(a, imports).map(Box::new))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(LogExpr::CelFallback { source: source.clone(), args: resolved_args })
        }
        LogExpr::Case { arms, default } => {
            let resolved_arms = arms.iter()
                .map(|(c, v)| Ok((r(c, imports)?, r(v, imports)?)))
                .collect::<Result<Vec<_>, ResolveError>>()?;
            Ok(LogExpr::Case { arms: resolved_arms, default: r(default, imports)? })
        }
    }
}

fn r(expr: &LogExpr, imports: &[UdfImport]) -> Result<Box<LogExpr>, ResolveError> {
    resolve(expr, imports).map(Box::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Value;
    use meta_types::external_fn::ResolvedUdfRef;

    fn test_imports() -> Vec<UdfImport> {
        vec![
            UdfImport {
                namespace: "io.notochord.udf.useragent".into(),
                version: "1".into(),
                cel_name: "ua_parse".into(),
                alias: None,
            },
            UdfImport {
                namespace: "io.notochord.udf.url".into(),
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
                assert_eq!(udf.namespace, "io.notochord.udf.useragent");
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
                assert_eq!(udf.namespace, "io.notochord.udf.url");
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
    fn no_imports_passes_through_non_udf() {
        let expr = LogExpr::GreaterThan {
            lhs: Box::new(LogExpr::GetFieldByName { field_name: "age".into() }),
            rhs: Box::new(LogExpr::Literal(Value::I64(18))),
        };
        let resolved = resolve(&expr, &[]).unwrap();
        assert_eq!(resolved, expr);
    }
}
