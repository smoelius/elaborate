use anyhow::Result;
use assert_cmd::{assert::OutputAssertExt, cargo::cargo_bin};
use similar_asserts::SimpleDiff;
use std::{
    fs::{copy, create_dir_all, read_to_string, write},
    path::Path,
    process::Command,
};

#[test]
fn fix() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("`elaborate-fix` package should be under workspace root");
    let fixture_before = workspace_root.join("fixtures/create_dir_etc_before");
    let tempdir = tempfile::tempdir().unwrap();
    let fixture = tempdir.path().join("create_dir_etc");
    copy_dir(&fixture_before, &fixture).unwrap();

    // smoelius: Verify the unmodified fixture compiles with no warnings.
    Command::new("cargo")
        .args(["check", "--deny=warnings"])
        .current_dir(&fixture)
        .assert()
        .success();

    Command::new(cargo_bin!("elaborate-fix"))
        .current_dir(&fixture)
        .assert()
        .success();

    assert!(
        expected == actual,
        "{}",
        SimpleDiff::from_str(&expected, &actual, "expected", "actual")
    );

    // smoelius: Verify the fixed fixture compiles with no warnings.
    Command::new("cargo")
        .args(["check", "--deny=warnings"])
        .current_dir(&fixture)
        .assert()
        .success();

    // smoelius: Verify the fixed fixture contains no disallowed methods.
    let mut command = elaborate::disallowed_methods();
    command.current_dir(&fixture);
    command.assert().success();

    // smoelius: Verify the fixed fixture does not need formatting.
    Command::new("cargo")
        .args(["fmt", "--check"])
        .current_dir(&fixture)
        .assert()
        .success();
}

fn copy_dir(source: &Path, destination: &Path) -> Result<()> {
    create_dir_all(destination).unwrap();
    for entry in source.read_dir().unwrap() {
        let entry = entry.unwrap();
        let file_name = entry.file_name();
        if file_name == "Cargo.lock" || file_name == "target" {
            continue;
        }
        let source_path = entry.path();
        let destination_path = destination.join(file_name);
        let metadata = source_path.metadata()?;
        let file_type = metadata.file_type();
        if file_type.is_dir() {
            copy_dir(&source_path, &destination_path)?;
        } else if file_type.is_file() {
            copy(source_path, destination_path)?;
        } else {
            panic!(
                "unexpected file of type {file_type:?} at: {}",
                source_path.display()
            );
        }
    }
    Ok(())
}

/// Replaces the fixture's repository-relative dependency path after it is copied to `/tmp`.
fn patch_fixture(workspace_root: &Path, fixture: &Path) -> Result<()> {
    let manifest_path = fixture.join("Cargo.toml");
    let manifest = read_to_string(&manifest_path)?;
    let elaborate_path = workspace_root
        .join("elaborate")
        .to_string_lossy()
        .replace('\\', "\\\\");
    let manifest = manifest.replace("../../elaborate", &elaborate_path);
    write(manifest_path, manifest)?;
    Ok(())
}
