use boa_engine::{Context, Source};
use serde_json::{Value, json};

fn check_cases() -> bool {
    let cases = [
        (
            "eval_created_function_must_not_retain_another_calls_var_eval",
            "const f=eval('0,function(p){return eval(p)}');f('var eval=1;eval');f('1+1')",
            json!(2.0),
        ),
        (
            "direct_eval_must_see_arguments",
            "(function(){return function(p){return eval(p)}('arguments[0]')})()",
            json!("arguments[0]"),
        ),
        (
            "explicit_eval_alias_must_remain_callable",
            "(function(){var eval=globalThis.eval;return function(p){return eval(p)}})()('1+1')",
            json!(2.0),
        ),
        (
            "dynamic_name_does_not_leak",
            "let x=7;const f=eval('(function(p){return eval(p)})');f('var x=1;x');f('x')",
            json!(7.0),
        ),
        (
            "same_call_keeps_dynamic_binding",
            "const f=eval('(function(){eval(\"var x=3\");return eval(\"x\")})');f()",
            json!(3.0),
        ),
        (
            "dynamic_binding_can_be_redeclared",
            "const f=eval('(function(p){eval(p);return eval(\"x\")})');f('var x=1');f('var x=2')",
            json!(2.0),
        ),
        (
            "static_var_still_hoisted",
            "const f=eval('(function(){let a=eval(\"typeof x\");var x=3;return a})');f()",
            json!("undefined"),
        ),
        (
            "static_alias_across_calls",
            "const f=eval('(function(){var eval=globalThis.eval;return eval(\"1+1\")})');f();f()",
            json!(2.0),
        ),
        (
            "parameter_eval_alias",
            "(function(eval){return eval('1+1')})(globalThis.eval)",
            json!(2.0),
        ),
        (
            "lexical_eval_alias",
            "(function(){let eval=globalThis.eval;return eval('1+1')})()",
            json!(2.0),
        ),
        (
            "lexical_tdz_preserved",
            "(function(){try{return eval('x');let x=1}catch(e){return e.name}})()",
            json!("ReferenceError"),
        ),
        (
            "parameter_tdz_preserved",
            "(()=>{try{(function(a=eval('b'),b=1){})()}catch(e){return e.name}})()",
            json!("ReferenceError"),
        ),
        (
            "dynamic_delete_and_redeclare",
            "const f=eval('(function(){eval(\"var x=1\");eval(\"delete x\");eval(\"var x=2\");return eval(\"x\")})');f();f()",
            json!(2.0),
        ),
        (
            "separate_closures_keep_values",
            "const f=eval('(function(p){eval(p);return ()=>eval(\"x\")})');let a=f('var x=1'),b=f('var x=2');a()+b()",
            json!(3.0),
        ),
        (
            "recursive_calls_keep_values",
            "const f=eval('(function(n){eval(\"var x=n\");if(n)f(n-1);return eval(\"x\")})');f(2)",
            json!(2.0),
        ),
        (
            "strict_eval_does_not_leak",
            "(function(){eval('\"use strict\";var x=1');return typeof x})()",
            json!("undefined"),
        ),
        (
            "arrow_eval_uses_outer_arguments",
            "(function(a){return (()=>eval('arguments[0]'))()})(9)",
            json!(9.0),
        ),
        (
            "mapped_arguments_eval_updates_parameter",
            "(function(a){eval('arguments[0]=4');return a})(1)",
            json!(4.0),
        ),
        (
            "unmapped_arguments_eval_keeps_parameter",
            "(function(a=1){eval('arguments[0]=4');return a})(2)",
            json!(2.0),
        ),
        (
            "arguments_parameter_shadows_object",
            "(function(arguments){return eval('arguments')})(8)",
            json!(8.0),
        ),
        (
            "lexical_arguments_shadows_object",
            "(function(){let arguments=8;return eval('arguments')})()",
            json!(8.0),
        ),
        (
            "annex_b_block_function_is_visible",
            "(function(){{function f(){return 2}}return f()})()",
            json!(2.0),
        ),
        (
            "closure_before_schema_growth_is_isolated",
            "var x=7;const f=eval('(function(p){if(p)eval(\"var x=1\");return ()=>eval(\"x\")})');let a=f(false);f(true);a()",
            json!(7.0),
        ),
        (
            "with_receiver_ignores_absent_dynamic_binding",
            "const o={m(){return this===o?2:0}};with(o){const f=eval('(function(p){if(p){eval(\"var m=1\");return 0}return m()})');f(true);f(false)}",
            json!(2.0),
        ),
    ];
    let mut failed = false;
    for (name, source, expected) in cases {
        let result = Context::default().eval(Source::from_bytes(source));
        let (actual, error) = match result {
            Ok(value) => (
                value.as_number().map_or_else(
                    || {
                        value
                            .as_string()
                            .map_or(Value::Null, |s| json!(s.to_std_string_escaped()))
                    },
                    |n| json!(n),
                ),
                None,
            ),
            Err(error) => (Value::Null, Some(error.to_string())),
        };
        let passed = actual == expected;
        failed |= !passed;
        println!(
            "{}",
            json!({"case":name,"passed":passed,"expected":expected,"actual":actual,"error":error})
        );
    }
    !failed
}

fn main() {
    if !check_cases() {
        std::process::exit(1);
    }
}

#[test]
fn eval_semantics() {
    assert!(check_cases());
}
