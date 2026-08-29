use anyhow::Result;
use elaborate::std::fs::{create_dir_wc, remove_dir_wc};
use std::path::Path;

const DIR: &str = "target/dir";

fn main() -> Result<()> {
    create_dir_wc(DIR)?;
    assert!(Path::new(DIR).is_dir());
    remove_dir_wc(DIR)?;
    Ok(())
}
