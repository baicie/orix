//! Integration tests for the rpnpm CLI.

#![allow(clippy::unwrap_used)]

use assert_cmd::Command;
use predicates::str::contains;

#[test]
fn cli_help_works() {
    let mut cmd = Command::cargo_bin("rpnpm").unwrap();
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(contains("Fast, disk-space efficient"));
}
