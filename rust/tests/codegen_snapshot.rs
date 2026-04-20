use std::process::Command;

const SNAPSHOT_MARKER: &str = "--- EXPECTED OUTPUT BELOW THIS LINE ---\n";

fn strip_hash_lines(s: &str) -> String {
    s.lines()
        .filter(|line| {
            !line.starts_with("// CATALOG_HASH:")
                && !line.starts_with("// CODEGEN_HASH:")
                && !line.starts_with("pub const EXPR_GEN_HASH")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn codegen_snapshot() {
    let output = Command::new(env!("CARGO_BIN_EXE_meta-codegen"))
        .arg("../function-catalog.yaml")
        .output()
        .expect("failed to run codegen binary");

    assert!(output.status.success(), "codegen exited with error: {}", String::from_utf8_lossy(&output.stderr));

    let actual = strip_hash_lines(&String::from_utf8_lossy(&output.stdout));

    let snapshot_raw = include_str!("expected_codegen_output.txt");
    let expected = snapshot_raw
        .split_once(SNAPSHOT_MARKER)
        .expect("snapshot file missing marker line")
        .1;

    if actual.trim() != expected.trim() {
        // Print a unified-ish diff for diagnosis
        let actual_lines: Vec<&str> = actual.trim().lines().collect();
        let expected_lines: Vec<&str> = expected.trim().lines().collect();

        let mut diff = String::new();
        let max = actual_lines.len().max(expected_lines.len());
        for i in 0..max {
            let a = actual_lines.get(i).unwrap_or(&"<missing>");
            let e = expected_lines.get(i).unwrap_or(&"<missing>");
            if a != e {
                diff.push_str(&format!("line {}: \n  expected: {e}\n  actual:   {a}\n", i + 1));
            }
        }

        panic!(
            "Codegen output does not match snapshot.\n\
             Update tests/expected_codegen_output.txt if the change is intentional.\n\n\
             {diff}"
        );
    }
}
