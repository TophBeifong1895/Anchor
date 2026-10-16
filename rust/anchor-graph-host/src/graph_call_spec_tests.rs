use super::*;
use serde_json::json;

fn parse(value: Value) -> Result<CallSpec, serde_json::Error> {
    serde_json::from_value(value)
}

#[test]
fn accepts_session_and_rejects_unknown_fields_and_wrong_types() {
    let call = parse(json!({"graph":"c","mode":"wait","session":"ops"})).unwrap();
    assert_eq!(call.session.as_deref(), Some("ops"));
    assert!(parse(json!({"graph":"c","mode":"wait","session":1})).is_err());
    assert!(parse(json!({"graph":"c","mode":"wait","bogus":1})).is_err());
    assert!(parse(json!({"graph":"c","mode":"wait","input":{"a":1},"input_map":[]})).is_err());
    assert!(parse(json!({"graph":"c","mode":"wait","result":{"node":"n","files":"x"}})).is_err());
    assert!(parse(json!({"graph":"c","mode":"wait","files":[{"node":"n","path":"p"}]})).is_err());
}

#[test]
fn input_map_resolves_rfc6901_pointers_against_the_parent_input() {
    let call = parse(json!({
        "graph":"c","mode":"wait",
        "input":{"constant":1},
        "input_map":{"topic":"/nested/topic","escaped":"/a~1b"}
    }))
    .unwrap();
    let resolved = call
        .child_input(&json!({"input":{"nested":{"topic":"x"},"a/b":"y"}}))
        .unwrap();
    assert_eq!(resolved, json!({"constant":1,"topic":"x","escaped":"y"}));
    // An unreachable pointer must fail rather than silently drop the key.
    assert!(matches!(
        call.child_input(&json!({"input":{}})),
        Err(GraphError::InvalidSnapshot(_))
    ));
    // A non-object `input` constant is rejected before admission.
    let bad = parse(json!({"graph":"c","mode":"wait","input":[1,2]})).unwrap();
    assert!(matches!(
        bad.child_input(&json!({"input":{}})),
        Err(GraphError::InvalidSnapshot(_))
    ));
}
