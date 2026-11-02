// These checks must run without ever invoking the opt-in pilot.
#[test]
fn pilot_requires_explicit_opt_in() {
    let source = include_str!("pilot.rs");
    assert!(
        source.contains("#[ignore = \"requires O3_TEST_PILOT=1"),
        "pilot integration runs by default"
    );
    assert!(
        source.contains("var_os(\"O3_TEST_PILOT\")"),
        "ignored pilot still needs an explicit environment gate"
    );
}
#[test]
fn differential_children_are_bound_to_parent() {
    let source = include_str!("differential.rs");
    assert!(
        source.contains("PR_SET_PDEATHSIG"),
        "timeout can orphan differential servers"
    );
}
