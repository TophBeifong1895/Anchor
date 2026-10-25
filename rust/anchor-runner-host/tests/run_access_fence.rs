mod common;

use common::production_tokens;
use std::{collections::BTreeMap, path::Path};

const INTERNAL_TYPES: &[&str] = &[
    concat!("Run", "Status"),
    "GraphRunRecord",
    "RunStore",
    concat!("Fil", "eRunStore"),
    concat!("Gra", "phRunner"),
    "RunCursor",
];

fn count(source: &str, external_tests: &mut Vec<String>) -> usize {
    production_tokens(source, external_tests)
        .iter()
        .filter(|token| INTERNAL_TYPES.contains(token))
        .count()
}

fn runtime_file(path: &str) -> bool {
    matches!(
        path,
        "execution.rs" | "main.rs" | "node_host.rs" | "run_data.rs" | "application/run_access.rs"
    ) || path.starts_with("node_host/")
        || path.starts_with("application/run_access/")
}

fn check_count(
    path: &str,
    actual: usize,
    maximum: Option<usize>,
) -> Result<Option<String>, String> {
    let maximum = maximum.unwrap_or(0);
    if actual > maximum {
        Err(format!(
            "{path} {actual} (maximum {maximum}; extend the facade, not the baseline)"
        ))
    } else if actual < maximum {
        Ok(Some(format!(
            "run_access_fence: lower run-access-fence.txt: {path} {maximum} -> {actual}"
        )))
    } else {
        Ok(None)
    }
}

fn fixture(source: &str) -> String {
    source
        .replace("RetiredPhaseType", INTERNAL_TYPES[0])
        .replace("RetiredFileType", INTERNAL_TYPES[3])
        .replace("RetiredRunnerType", INTERNAL_TYPES[4])
}

#[test]
fn run_access_fence() {
    assert_eq!(
        count(
            &fixture(
                "RetiredPhaseType RetiredPhaseType::Running GraphRunRecord RunStore RetiredFileType RetiredRunnerType RunCursor"
            ),
            &mut Vec::new()
        ),
        7
    );
    assert_eq!(
        count(
            &fixture(
                r####"// RetiredPhaseType
        /* RunStore /* RetiredRunnerType */ RunCursor */
        "RetiredPhaseType \\" b"RunStore" c"RetiredRunnerType" r###"RunCursor"### br#"RetiredFileType"# cr"GraphRunRecord"
        '界' '\\' '\u{1234}' 'R' b'R'
        MyRetiredPhaseType RetiredPhaseTypeSuffix RetiredPhaseType界 界RetiredPhaseType RetiredPhaseTypé
        use example::RetiredPhaseType as State; let _: State; r#RunCursor
    "####
            ),
            &mut Vec::new()
        ),
        2
    );
    assert_eq!(
        count(
            &fixture(
                r#"
        impl Host {
            fn before(_: RetiredPhaseType) {}
            #[cfg(test)] pub(crate) fn helper(_: RunStore) { let _ = RetiredRunnerType; }
            fn after(_: GraphRunRecord) {}
        }
        #[cfg ( test )] mod first { use super::RunCursor; mod nested { type Alias = RunStore; } }
        fn between(_: RetiredFileType) {}
        #[cfg(test)] mod second { let _ = "}"; }
        fn last(_: RetiredRunnerType) {}
    "#
            ),
            &mut Vec::new()
        ),
        4
    );
    let mut external_tests = Vec::new();
    assert_eq!(
        count(
            &fixture(
                r#"
        #[cfg(test)] mod fixture;
        #[cfg(test)] use example::{RetiredPhaseType, RunStore};
        struct Host {
            #[cfg(test)] fixture: RunStore,
            production: GraphRunRecord,
        }
        #[cfg(test)] #[allow(dead_code)] fn helper() -> RunCursor { unreachable!() }
        type State = example::RetiredPhaseType;
        type Alias = State;
        check!(RetiredRunnerType<RunCursor>);
    "#
            ),
            &mut external_tests
        ),
        4
    );
    assert_eq!(external_tests, ["fixture"]);
    assert_eq!(
        count(
            &fixture("RetiredPhaseType\u{3000}RunStore\u{a0}RunCursor"),
            &mut Vec::new()
        ),
        3
    );
    assert_eq!(
        check_count("known.rs", 2, Some(1)),
        Err("known.rs 2 (maximum 1; extend the facade, not the baseline)".to_owned())
    );
    assert_eq!(
        check_count("new.rs", 1, None),
        Err("new.rs 1 (maximum 0; extend the facade, not the baseline)".to_owned())
    );
    assert_eq!(
        check_count("known.rs", 1, Some(2)),
        Ok(Some(
            "run_access_fence: lower run-access-fence.txt: known.rs 2 -> 1".to_owned()
        ))
    );
    assert_eq!(
        check_count("deleted.rs", 0, Some(2)),
        Ok(Some(
            "run_access_fence: lower run-access-fence.txt: deleted.rs 2 -> 0".to_owned()
        ))
    );
    assert_eq!(check_count("known.rs", 1, Some(1)), Ok(None));
    assert_eq!(check_count("new.rs", 0, None), Ok(None));
    for allowed in [
        "execution.rs",
        "main.rs",
        "node_host.rs",
        "node_host/worker.rs",
        "run_data.rs",
        "application/run_access.rs",
        "application/run_access/views.rs",
    ] {
        assert!(runtime_file(allowed));
    }
    for business in [
        "execution/new.rs",
        "run_data/new.rs",
        "application/run_access_extra.rs",
        "node_host_extra.rs",
    ] {
        assert!(!runtime_file(business));
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source_root = root.join("src");
    let counts = common::production_counts(&source_root, |_, tokens| {
        tokens
            .iter()
            .filter(|token| INTERNAL_TYPES.contains(token))
            .count()
    });

    let mut baseline = BTreeMap::new();
    for line in include_str!("../run-access-fence.txt").lines() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        assert_eq!(fields.len(), 2, "expected path count: {line}");
        let maximum: usize = fields[1].parse().expect("invalid baseline count");
        assert!(
            !runtime_file(fields[0]),
            "runtime file in business baseline: {line}"
        );
        assert!(
            baseline.insert(fields[0], maximum).is_none(),
            "duplicate baseline: {line}"
        );
    }
    let mut violations = Vec::new();
    let mut business_total = 0;
    let mut runtime_total = 0;
    for (path, actual) in &counts {
        if runtime_file(path) {
            runtime_total += actual;
            continue;
        }
        business_total += actual;
        if let Err(message) = check_count(path, *actual, baseline.get(path.as_str()).copied()) {
            violations.push(message);
        }
    }
    for (path, maximum) in baseline {
        let actual = counts.get(path).copied().unwrap_or(0);
        if let Ok(Some(message)) = check_count(path, actual, Some(maximum)) {
            eprintln!("{message}");
        }
    }
    eprintln!(
        "run_access_fence: {} production files; business {business_total}; runtime {runtime_total}",
        counts.len()
    );
    assert!(
        violations.is_empty(),
        "Run internals ratchet exceeded:\n{}",
        violations.join("\n")
    );
}
