use assert_cmd::Command;

#[test]
fn version_flag_prints_crate_version() {
    let expected = format!("ts-bridge {}\n", env!("CARGO_PKG_VERSION"));
    let assert = Command::new(assert_cmd::cargo::cargo_bin!("ts-bridge"))
        .arg("--version")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone())
        .expect("stdout should be valid UTF-8");
    assert_eq!(stdout, expected);
}
