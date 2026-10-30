//! Native AST preparation, adapted from yt-dlp/ejs's Unlicense solver core.
//! https://github.com/yt-dlp/ejs (bundled with yt-dlp 2026.08.19).
//! Parse the player with Rust rather than running a JavaScript parser in Boa.
use crate::Result;
use boa_ast::{
    Declaration, Position, Script, Spanned, Statement, StatementListItem,
    expression::{Expression, access::PropertyAccess},
    function::FunctionBody,
    scope::Scope,
    visitor::{VisitWith, Visitor},
};
use boa_interner::{Interner, ToInternedString};
use boa_parser::{Parser, Source};
use std::ops::ControlFlow;

fn unparen(mut e: &Expression) -> &Expression {
    while let Expression::Parenthesized(p) = e {
        e = p.expression();
    }
    e
}
fn statement_expression(s: &StatementListItem) -> Option<&Expression> {
    if let StatementListItem::Statement(s) = s
        && let Statement::Expression(e) = s.as_ref()
    {
        Some(unparen(e))
    } else {
        None
    }
}
fn body(script: &Script) -> Result<(&FunctionBody, bool)> {
    let items = script.statements().statements();
    let index = match items.len() {
        1 => 0,
        2 => 1,
        _ => return Err("unsupported player wrapper".into()),
    };
    let Some(Expression::Call(call)) = items.get(index).and_then(statement_expression) else {
        return Err("unsupported player call".into());
    };
    let target = unparen(call.function());
    let target = if index == 0 {
        if let Expression::PropertyAccess(PropertyAccess::Simple(p)) = target {
            unparen(p.target())
        } else {
            return Err("unsupported player receiver".into());
        }
    } else {
        target
    };
    if let Expression::FunctionExpression(f) = target {
        Ok((f.body(), index == 1))
    } else {
        Err("unsupported player function".into())
    }
}
fn is_candidate(body: &FunctionBody, interner: &Interner) -> bool {
    body.statements().iter().any(|s| {
        let Some(Expression::Call(c))=statement_expression(s) else{return false};
        let Expression::PropertyAccess(PropertyAccess::Simple(p))=unparen(c.function()) else{return false};
        if !matches!(unparen(p.target()),Expression::Identifier(_)){return false}
        c.args().len()==2 && c.args().iter().zip(["alr","yes"]).all(|(arg,want)| {
            matches!(unparen(arg),Expression::Literal(l) if l.as_string().and_then(|sym|interner.resolve(sym)).is_some_and(|value|value.to_string()==want))
        })
    })
}
fn candidates(body: &FunctionBody, interner: &Interner) -> Vec<String> {
    let mut found = Vec::new();
    for item in body.statements() {
        match item {
            StatementListItem::Declaration(d) => {
                if let Declaration::FunctionDeclaration(f) = d.as_ref()
                    && is_candidate(f.body(), interner)
                {
                    found.push(f.name().to_interned_string(interner));
                }
            }
            StatementListItem::Statement(s) => match s.as_ref() {
                Statement::Expression(Expression::Assign(a)) => {
                    if let Expression::FunctionExpression(f) = unparen(a.rhs())
                        && is_candidate(f.body(), interner)
                    {
                        found.push(a.lhs().to_interned_string(interner));
                    }
                }
                Statement::Var(v) => {
                    for v in v.0.as_ref() {
                        if let Some(Expression::FunctionExpression(f)) = v.init().map(unparen)
                            && is_candidate(f.body(), interner)
                        {
                            found.push(v.binding().to_interned_string(interner));
                        }
                    }
                }
                _ => {}
            },
        }
    }
    found
}
// Some Boa call spans cover only the argument list. Include child expressions
// so removal also covers the callee, receiver and enclosing parentheses.
fn expression_bounds(e: &Expression) -> (Position, Position) {
    struct Bounds {
        start: Position,
        end: Position,
    }
    impl<'ast> Visitor<'ast> for Bounds {
        type BreakTy = ();
        fn visit_expression(&mut self, node: &'ast Expression) -> ControlFlow<()> {
            self.start = self.start.min(node.span().start());
            let mut end = node.span().end();
            // Boa 0.22's nonempty class span ends at the closing brace rather
            // than after it (ClassTail parser). Empty class spans are correct.
            if let Expression::ClassExpression(c) = node
                && (c.constructor().is_some() || !c.elements().is_empty())
            {
                end = Position::new(end.line_number(), end.column_number() + 1);
            }
            self.end = self.end.max(end);
            node.visit_with(self)
        }
    }
    let mut bounds = Bounds {
        start: e.span().start(),
        end: e.span().end(),
    };
    let _ = bounds.visit_expression(e);
    (bounds.start, bounds.end)
}
// Boa's AST display is a diagnostic printer, not a JavaScript serializer.
// Preserve all original literals and function bodies when preparing a player.
fn byte_offset(source: &str, position: Position) -> Result<usize> {
    let mut line = 1;
    let mut column = 1;
    let mut was_cr = false;
    for (offset, ch) in source.char_indices() {
        if line == position.line_number() && column == position.column_number() {
            return Ok(offset);
        }
        match ch {
            '\r' => {
                line += 1;
                column = 1;
            }
            '\n' if was_cr => {}
            '\n' | '\u{2028}' | '\u{2029}' => {
                line += 1;
                column = 1;
            }
            _ => column += 1,
        }
        was_cr = ch == '\r';
    }
    if line == position.line_number() && column == position.column_number() {
        return Ok(source.len());
    }
    Err("player span is outside source".into())
}
const SETUP: &str = r#"
if(typeof XMLHttpRequest==='undefined')globalThis.XMLHttpRequest={prototype:{}};
globalThis.location={hash:'',host:'www.youtube.com',hostname:'www.youtube.com',href:'https://www.youtube.com/watch?v=yt-dlp-wins',origin:'https://www.youtube.com',password:'',pathname:'/watch',port:'',protocol:'https:',search:'?v=yt-dlp-wins',username:''};
for(const k of ['document','navigator'])if(typeof globalThis[k]==='undefined')globalThis[k]=Object.create(null);
for(const k of ['self','window'])if(typeof globalThis[k]==='undefined')globalThis[k]=globalThis;
"#;

pub fn prepare(source: &str) -> Result<String> {
    let mut interner = Interner::default();
    let script = Parser::new(Source::from_bytes(source))
        .parse_script(&Scope::new_global(), &mut interner)
        .map_err(|_| "player AST parse failed")?;
    let (outer, skip_first) = body(&script)?;
    let names = candidates(outer, &interner);
    if names.is_empty() || names.len() > 16 {
        return Err("unsupported player solver candidates".into());
    }
    let mut solvers = Vec::new();
    for name in names {
        solvers.push(format!(r#"n=>{{const u=({name})('https://youtube.com/watch?v=yt-dlp-wins','s',undefined);u.set('n',n);const p=Object.getPrototypeOf(u);for(const k of Object.keys(p).concat(Object.getOwnPropertyNames(p))){{if(!['constructor','set','get','clone'].includes(k)){{u[k]();break}}}}return u.get('n')??null}}"#));
    }
    let export = format!(
        r#"_result.n=n=>{{const results=new Set();for(const f of [{}]){{try{{results.add(f(n))}}catch(e){{}}}}if(results.size!==1)throw new Error('ambiguous player solver');return results.values().next().value}};"#,
        solvers.join(",")
    );
    let start = byte_offset(source, outer.span().start())?;
    let end = byte_offset(source, outer.span().end())?;
    if source.as_bytes().get(start) != Some(&b'{')
        || end == 0
        || source.as_bytes().get(end - 1) != Some(&b'}')
    {
        return Err("invalid player body span".into());
    }
    let mut removals = Vec::new();
    if skip_first {
        let Some(StatementListItem::Statement(first)) = outer.statements().first() else {
            return Err("unsupported player prologue".into());
        };
        let Statement::Var(v) = first.as_ref() else {
            return Err("unsupported player prologue".into());
        };
        let [v] = v.0.as_ref() else {
            return Err("unsupported player prologue".into());
        };
        let Some(Expression::This(this)) = v.init() else {
            return Err("unsupported player prologue".into());
        };
        let after = byte_offset(source, this.span().end())?;
        let whitespace = source[after..].len()
            - source[after..]
                .trim_start_matches(char::is_whitespace)
                .len();
        let after = after + whitespace;
        if source.as_bytes().get(after) != Some(&b';') {
            return Err("unsupported player prologue delimiter".into());
        }
        removals.push((start + 1, after + 1));
    }
    for item in outer.statements().iter().skip(usize::from(skip_first)) {
        if let StatementListItem::Statement(statement) = item
            && let Statement::Expression(e) = statement.as_ref()
            && !matches!(unparen(e), Expression::Assign(_) | Expression::Literal(_))
        {
            let (from, to) = expression_bounds(e);
            let mut from = byte_offset(source, from)?;
            let mut to = byte_offset(source, to)?;
            // The parser can discard grouping nodes around a call's callee.
            // At statement level these adjacent parentheses belong to this
            // expression, never to an enclosing argument or condition.
            loop {
                let trimmed = source[..from].trim_end();
                if trimmed.ends_with('(') {
                    from = trimmed.len() - 1;
                } else {
                    break;
                }
            }
            loop {
                let tail = &source[to..];
                let trimmed = tail.trim_start();
                if trimmed.starts_with(')') {
                    to += tail.len() - trimmed.len() + 1;
                } else {
                    break;
                }
            }
            removals.push((from, to));
        }
    }
    removals.sort_unstable();
    let mut cursor = 0;
    let mut prepared = String::with_capacity(source.len() + export.len() + SETUP.len());
    prepared.push_str(SETUP);
    for (from, to) in removals {
        if from < cursor || to < from || to > end - 1 {
            return Err("overlapping player preparation spans".into());
        }
        prepared.push_str(
            source
                .get(cursor..from)
                .ok_or("invalid player preparation boundary")?,
        );
        cursor = to;
    }
    prepared.push_str(
        source
            .get(cursor..end - 1)
            .ok_or("invalid player export boundary")?,
    );
    prepared.push(';');
    prepared.push_str(&export);
    prepared.push_str(source.get(end - 1..).ok_or("invalid player suffix")?);
    Ok(prepared)
}

#[test]
fn rejects_unknown_wrappers() {
    assert!(prepare("let x=1;").is_err());
    assert!(prepare("(function(){})()").is_err());
}

#[test]
fn prepares_declaration_assignment_and_variable_candidates() {
    let function = "function(){var u=new U();u.set('alr','yes');return u}";
    for candidate in [
        format!("var C={function};"),
        format!("C={function};"),
        "function C(){var u=new U();u.set('alr','yes');return u}".to_owned(),
    ] {
        let source = format!(
            r#"var namespace={{}};(function(g){{
            var unused=this;
            var literal="é😀a\'b";
            function U(){{this.values={{}}}}
            U.prototype.set=function(k,v){{this.values[k]=v}};
            U.prototype.get=function(k){{return this.values[k]}};
            U.prototype.transform=function(){{this.values.n+='x'}};
            {candidate}
            (function(){{unwantedStartup()}})();
            (((unwantedStartup())));
            new class {{constructor(){{unwantedStartup()}} method(){{return 0}}}};
        }})(namespace);"#
        );
        let prepared = prepare(&source).unwrap();
        let result = boa_engine::Context::default()
            .eval(boa_engine::Source::from_bytes(&format!(
                "var _result={{}};{prepared};_result.n('abc')"
            )))
            .unwrap();
        assert_eq!(result.as_string().unwrap().to_std_string_escaped(), "abcx");
    }
}
