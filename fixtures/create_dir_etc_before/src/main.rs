use anyhow::Result;
use std::{
    fs::{create_dir, remove_dir},
    path::Path,
};

const DIR: &str = "target/dir";

fn main() -> Result<()> {
    create_dir(DIR)?;
    assert!(Path::new(DIR).is_dir());
    remove_dir(DIR)?;
    Ok(())
}
