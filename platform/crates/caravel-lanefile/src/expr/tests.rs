use std::collections::{BTreeMap, BTreeSet};

use proptest::prelude::*;

use super::*;

fn scope() -> MapScope {
    let mut m = BTreeMap::new();
    m.insert(
        "lane".into(),
        Value::Map(BTreeMap::from([("name".into(), Value::Str("acme".into()))])),
    );
    m.insert(
        "var".into(),
        Value::Map(BTreeMap::from([
            ("n".into(), Value::Int(3)),
            (
                "validators".into(),
                Value::List(vec![
                    Value::Str("1".into()),
                    Value::Str("2".into()),
                    Value::Str("4".into()),
                ]),
            ),
            (
                "ports".into(),
                Value::Map(BTreeMap::from([
                    ("a".into(), Value::Int(1)),
                    ("b".into(), Value::Int(2)),
                ])),
            ),
            ("host".into(), Value::Null),
            ("name-with-dash".into(), Value::Str("ok".into())),
        ])),
    );
    m.insert(
        "contract".into(),
        Value::Deferred(BTreeSet::from(["contract".to_string()])),
    );
    MapScope(m)
}

fn ev(s: &str) -> Value {
    eval_str(s, &scope()).unwrap_or_else(|e| panic!("{s}: {e}"))
}

fn bad(s: &str) -> String {
    match eval_str(s, &scope()) {
        Ok(v) => panic!("{s} gave {v}"),
        Err(e) => e.to_string(),
    }
}

fn s(x: &str) -> Value {
    Value::Str(x.into())
}

#[test]
fn the_type_rule() {
    assert_eq!(ev("plain"), s("plain"));
    assert_eq!(ev("${var.n}"), Value::Int(3));
    assert_eq!(ev("${lane.name}-admin"), s("acme-admin"));
    assert_eq!(ev("v${var.n}"), s("v3"));
    assert_eq!(ev("${ var.n }"), Value::Int(3));
    assert_eq!(ev("$${not} an expression"), s("${not} an expression"));
    assert_eq!(
        ev("${var.validators}"),
        Value::List(vec![s("1"), s("2"), s("4")])
    );
    assert_eq!(ev("${null}"), Value::Null);
    assert!(bad("x${var.validators}").contains("a list can't be part of a string"));
    assert!(has_expression("a ${b}") && !has_expression("a $${b}") && !has_expression("plain"));
}

#[test]
fn arithmetic() {
    for (e, want) in [
        ("${1 + 2 * 3}", 7),
        ("${(1 + 2) * 3}", 9),
        ("${length(var.validators) * 2 / 3 + 1}", 3),
        ("${7 / 2}", 3),
        ("${-7 / 2}", -3),
        ("${7 % 3}", 1),
        ("${-var.n}", -3),
        ("${10 - 2 - 3}", 5),
        ("${var.n-1}", 2),
    ] {
        // `var.n-1` is not a name: a name doesn't end in `-`, but `n-1`… is.
        if e == "${var.n-1}" {
            assert!(
                eval_str(e, &scope()).is_err(),
                "{e}: a name may have dashes"
            );
            continue;
        }
        assert_eq!(ev(e), Value::Int(want), "{e}");
    }
    assert_eq!(ev("${var.n - 1}"), Value::Int(2));
    assert!(bad("${1 / 0}").contains("division by zero"));
    assert!(bad("${1 % 0}").contains("division by zero"));
    assert!(bad("${9223372036854775807 + 1}").contains("overflows"));
    assert!(bad("${99999999999999999999}").contains("too large"));
    assert!(bad("${'a' + 'b'}").contains("adds numbers"));
    assert!(bad("${1 + true}").contains("doesn't apply"));
}

#[test]
fn logic_and_comparison() {
    for (e, want) in [
        ("${1 < 2}", true),
        ("${2 <= 2}", true),
        ("${'b' > 'a'}", true),
        ("${var.n == 3 && !(var.n != 3)}", true),
        ("${var.host == null}", true),
        ("${var.n == null}", false),
        ("${true || 1 / 0 == 0}", true),
        ("${false && 1 / 0 == 0}", false),
        ("${[1, 2] == [1, 2]}", true),
    ] {
        assert_eq!(ev(e), Value::Bool(want), "{e}");
    }
    assert_eq!(ev("${var.n > 2 ? 'many' : 'few'}"), s("many"));
    assert_eq!(ev("${var.n > 5 ? 1 / 0 : 'lazy'}"), s("lazy"));
    assert!(bad("${1 == '1'}").contains("one kind"));
    assert!(bad("${1 < 'a'}").contains("two numbers or two strings"));
    assert!(bad("${var.n ? 1 : 2}").contains("needs a boolean"));
}

#[test]
fn names_fields_and_indexes() {
    assert_eq!(ev("${var.validators[2]}"), s("4"));
    assert_eq!(ev("${var.ports['b']}"), Value::Int(2));
    assert_eq!(ev("${var.name-with-dash}"), s("ok"));
    let e = bad("${vra.n}");
    assert!(
        e.contains("unknown name `vra`") && e.contains("did you mean `var`?"),
        "{e}"
    );
    let e = bad("${var.vlaidators}");
    assert!(
        e.contains("no `vlaidators`") && e.contains("did you mean `validators`?"),
        "{e}"
    );
    assert!(bad("${var.validators[3]}").contains("outside the list"));
    assert!(bad("${var.n.x}").contains("has no fields"));
}

#[test]
fn literals_and_comprehensions() {
    assert_eq!(
        ev("${[for v in var.validators : 'acme-v${v}']}"),
        Value::List(vec![s("acme-v1"), s("acme-v2"), s("acme-v4")])
    );
    assert_eq!(
        ev("${[for i, v in var.validators : i if v != '2']}"),
        Value::List(vec![Value::Int(0), Value::Int(2)])
    );
    assert_eq!(
        ev("${[for k, v in var.ports : '${k}=${v}']}"),
        Value::List(vec![s("a=1"), s("b=2")])
    );
    assert_eq!(
        ev("${{ name = lane.name, 'n-2' = var.n * 2, }}"),
        Value::Map(BTreeMap::from([
            ("name".into(), s("acme")),
            ("n-2".into(), Value::Int(6))
        ]))
    );
    assert_eq!(ev("${[]}"), Value::List(vec![]));
    assert_eq!(ev(r#"${"a\"b\\c"}"#), s("a\"b\\c"));
    assert!(bad("${[for v in 3 : v]}").contains("a list or a map"));
    assert!(bad("${[for v in var.validators : v if 1]}").contains("needs a boolean"));
}

#[test]
fn functions() {
    let list = |v: Vec<i64>| Value::List(v.into_iter().map(Value::Int).collect());
    assert_eq!(ev("${range(3)}"), list(vec![0, 1, 2]));
    assert_eq!(ev("${range(1, 4)}"), list(vec![1, 2, 3]));
    assert_eq!(ev("${range(6, 0, -2)}"), list(vec![6, 4, 2]));
    assert_eq!(ev("${length('héllo')}"), Value::Int(5));
    assert_eq!(ev("${length(var.ports)}"), Value::Int(2));
    assert_eq!(ev("${concat([1], [2, 3])}"), list(vec![1, 2, 3]));
    assert_eq!(ev("${merge(var.ports, { b = 9 }).b}"), Value::Int(9));
    assert_eq!(ev("${lookup(var.ports, 'z', 0)}"), Value::Int(0));
    assert_eq!(ev("${keys(var.ports)}"), Value::List(vec![s("a"), s("b")]));
    assert_eq!(ev("${contains(var.validators, '4')}"), Value::Bool(true));
    assert_eq!(ev("${contains('acme-pay', 'pay')}"), Value::Bool(true));
    assert_eq!(ev("${join(',', var.validators)}"), s("1,2,4"));
    assert_eq!(
        ev("${split('-', 'a-b')}"),
        Value::List(vec![s("a"), s("b")])
    );
    assert_eq!(ev("${replace('a.b', '.', '-')}"), s("a-b"));
    assert_eq!(ev("${upper('ab')}${lower('CD')}"), s("ABcd"));
    assert_eq!(ev("${tostring(var.n)}"), s("3"));
    assert_eq!(ev("${tonumber('42')}"), Value::Int(42));
    assert_eq!(ev("${min(3, 1, 2)} ${max([3, 9])}"), s("1 9"));
    assert_eq!(ev("${coalesce(var.host, 'box-1')}"), s("box-1"));
    assert_eq!(ev("${format('{}-v{}', lane.name, 2)}"), s("acme-v2"));
    assert_eq!(ev("${format('{{}}')}"), s("{}"));
    assert!(bad("${lookup(var.ports, 'z')}").contains("no key"));
    assert!(bad("${range(0, 5, 0)}").contains("step can't be 0"));
    assert!(bad("${range(100000)}").contains("more than"));
    assert!(bad("${format('{}')}").contains("more {} than arguments"));
    assert!(bad("${tonumber('1.5')}").contains("whole number"));
    let e = bad("${lenght(var.ports)}");
    assert!(
        e.contains("unknown function") && e.contains("did you mean `length`?"),
        "{e}"
    );
}

#[test]
fn values_known_later() {
    let d = |refs: &[&str]| Value::Deferred(refs.iter().map(|r| r.to_string()).collect());
    assert_eq!(
        ev("${contract.settlement.address}"),
        d(&["contract.settlement.address"])
    );
    assert_eq!(
        ev("C:${contract.oracle.address}"),
        d(&["contract.oracle.address"])
    );
    assert_eq!(
        ev("${[contract.a.address, contract.b.address]}"),
        d(&["contract.a.address", "contract.b.address"])
    );
    assert_eq!(ev("${length(contract.list)}"), d(&["contract.list"]));
    assert_eq!(ev("${contract.x[0]}"), d(&["contract.x[0]"]));
    assert!(ev("${contract.a.address}")
        .to_toml()
        .unwrap_err()
        .contains("contract.a.address"));
}

#[test]
fn parse_errors_point_at_the_problem() {
    for (src, want) in [
        ("${}", "an empty ${}"),
        ("${var.n", "never closed"),
        ("${var.n +}", "expected a value, found '}'"),
        ("${var.n ]}", "unexpected ']'"),
        ("${'abc}", "never closed"),
        ("${[1, 2}", "expected \",\", found '}'"),
        ("${{ a 1 }}", "expected \"=\""),
        ("${'\\q'}", "unknown escape"),
    ] {
        let e = Template::parse(src).unwrap_err();
        assert!(e.message.contains(want), "{src}: {}", e.message);
        assert!(e.at.end <= src.len(), "{src}: {:?}", e.at);
    }
    let deep = format!("${{{}1{}}}", "(".repeat(100), ")".repeat(100));
    assert!(Template::parse(&deep)
        .unwrap_err()
        .message
        .contains("nested too deeply"));
    let long = format!("${{1{}}}", " + 1".repeat(1000));
    assert!(Template::parse(&long)
        .unwrap_err()
        .message
        .contains("too long"));
}

#[test]
fn values_to_and_from_toml() {
    let t: toml::Table = toml::from_str("a = 1\nb = [\"x\"]\nc = { d = true }\nf = 1.5\n").unwrap();
    let v = Value::from_toml(&toml::Value::Table(t.clone()));
    assert_eq!(v.to_toml().unwrap(), Some(toml::Value::Table(t)));
    let m = Value::Map(BTreeMap::from([
        ("keep".into(), Value::Int(1)),
        ("gone".into(), Value::Null),
    ]));
    assert_eq!(
        m.to_toml()
            .unwrap()
            .unwrap()
            .as_table()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["keep"]
    );
    assert!(Value::List(vec![Value::Null]).to_toml().is_err());
}

proptest! {
    /// No input makes the parser or the evaluator panic.
    #[test]
    fn never_panics(s in "\\PC{0,60}") {
        let _ = eval_str(&s, &scope());
        let _ = eval_str(&format!("${{{s}}}"), &scope());
    }

    #[test]
    fn never_panics_on_expression_like_input(s in "[ a-z0-9_.$'\"\\[\\](){}+*/%<>=!&|?:,-]{0,40}") {
        let _ = eval_str(&format!("${{{s}}}"), &scope());
        let _ = eval_str(&s, &scope());
    }
}
