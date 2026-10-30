//! Extract page attestation data without executing page JavaScript.
use crate::Result;
use boa_ast::{
    Statement, StatementListItem,
    expression::{
        Expression,
        literal::{LiteralKind, PropertyDefinition},
    },
    property::PropertyName,
    scope::Scope,
};
use boa_interner::Interner;
use boa_parser::{Parser, Source};
use serde_json::{Value, json};

fn object_end(source: &str) -> Option<usize> {
    let mut depth = 0_u32;
    let mut quote = None;
    let mut escaped = false;
    for (offset, c) in source.char_indices() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
        } else {
            match c {
                '\'' | '"' => quote = Some(c),
                '{' => depth = depth.checked_add(1)?,
                '}' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(offset + 1);
                    }
                }
                _ => {}
            }
        }
    }
    None
}
fn data_value(e: &Expression, interner: &Interner, depth: usize) -> Result<Value> {
    if depth > 64 {
        return Err("attestation page data is too deeply nested".into());
    }
    match e {
        Expression::Parenthesized(p) => data_value(p.expression(), interner, depth + 1),
        Expression::Literal(l) => match l.kind() {
            LiteralKind::String(s) => Ok(Value::String(
                interner
                    .resolve(*s)
                    .ok_or("unknown string symbol in page data")?
                    .to_string(),
            )),
            LiteralKind::Int(n) => Ok(json!(n)),
            LiteralKind::Num(n) => serde_json::Number::from_f64(*n)
                .map(Value::Number)
                .ok_or_else(|| "invalid page data number".into()),
            LiteralKind::Bool(b) => Ok(json!(b)),
            LiteralKind::Null => Ok(Value::Null),
            _ => Err("unsupported page data literal".into()),
        },
        Expression::ObjectLiteral(object) => {
            let mut out = serde_json::Map::new();
            for property in object.properties() {
                let PropertyDefinition::Property(PropertyName::Literal(name), value) = property
                else {
                    return Err("executable page data is unsupported".into());
                };
                let name = interner
                    .resolve(name.sym())
                    .ok_or("unknown property symbol in page data")?
                    .to_string();
                if out
                    .insert(name, data_value(value, interner, depth + 1)?)
                    .is_some()
                {
                    return Err("duplicate page data property".into());
                }
            }
            Ok(Value::Object(out))
        }
        Expression::ArrayLiteral(array) => array
            .as_ref()
            .iter()
            .map(|v| {
                data_value(
                    v.as_ref().ok_or("sparse page data array")?,
                    interner,
                    depth + 1,
                )
            })
            .collect::<Result<Vec<_>>>()
            .map(Value::Array),
        _ => Err("executable page data is unsupported".into()),
    }
}
fn loose_object(source: &str) -> Result<Value> {
    let source = format!("({source});");
    let mut interner = Interner::default();
    let script = Parser::new(Source::from_bytes(&source))
        .parse_script(&Scope::new_global(), &mut interner)
        .map_err(|_| "invalid page data syntax")?;
    let [StatementListItem::Statement(statement)] = script.statements().statements() else {
        return Err("invalid page data statement".into());
    };
    let Statement::Expression(expression) = statement.as_ref() else {
        return Err("invalid page data expression".into());
    };
    data_value(expression, &interner, 0)
}
pub(super) fn extract(page: &str) -> Result<(Value, Value)> {
    let config = page
        .match_indices("ytcfg.set(")
        .find_map(|(i, marker)| {
            serde_json::Deserializer::from_str(&page[i + marker.len()..])
                .into_iter::<Value>()
                .next()
                .and_then(|r| r.ok())
                .filter(|v| v.get("INNERTUBE_CONTEXT").is_some())
        })
        .ok_or("attestation homepage has no client context")?;
    for (offset, marker) in page.match_indices("window.ytAtN(") {
        let tail = page[offset + marker.len()..].trim_start();
        if !tail.starts_with('{') {
            continue;
        }
        let end = object_end(tail).ok_or("incomplete page challenge object")?;
        let data = loose_object(&tail[..end])?;
        let root = data.get("R").ok_or("page challenge has no response")?;
        let root = if let Some(text) = root.as_str() {
            serde_json::from_str(text).map_err(|_| "invalid embedded page challenge")?
        } else {
            root.clone()
        };
        let challenge = root
            .get("bgChallenge")
            .filter(|v| v.is_object())
            .cloned()
            .ok_or("page has no BotGuard challenge")?;
        return Ok((challenge, config));
    }
    Err("attestation homepage has no BotGuard challenge".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_data_literals_and_rejects_executable_expressions() {
        let parsed = loose_object(
            r#"{R:'{"bgChallenge":{"program":"a\\x3db"}}',ok:true,values:[1,null,"é"],}"#,
        )
        .unwrap();
        assert!(parsed["R"].as_str().unwrap().contains("bgChallenge"));
        assert_eq!(loose_object(r#"{R:'a\x3db'}"#).unwrap()["R"], "a=b");
        for source in [
            "{R:fn()}",
            "{get R(){return 1}}",
            "{...x}",
            "{[fn()]:1}",
            "{R:1,R:2}",
        ] {
            assert!(loose_object(source).is_err(), "{source}");
        }
    }
    #[test]
    fn balanced_data_ignores_escaped_braces_and_later_script() {
        let data = r#"{R:'a\'}b',x:{y:1}});danger()"#;
        let end = object_end(data).unwrap();
        assert_eq!(&data[end..], ");danger()");
        assert!(object_end("{x:{").is_none());
    }
    #[test]
    fn extracts_a_matching_page_pair_after_boilerplate_callbacks() {
        let response =
            serde_json::to_string(&json!({"bgChallenge":{"program":"fixture","globalName":"vm"}}))
                .unwrap();
        let literal = serde_json::to_string(&response).unwrap();
        let page = format!(
            "ytcfg.set('x');ytcfg.set({{\"INNERTUBE_CONTEXT\":{{}},\"EVENT_ID\":\"fixture\"}});window.ytAtN();window.ytAtN({{R:{literal},T:'unused'}});"
        );
        let (challenge, config) = extract(&page).unwrap();
        assert_eq!(challenge["program"], "fixture");
        assert_eq!(config["EVENT_ID"], "fixture");
    }
}
