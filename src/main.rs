#![feature(let_chains)]

use std::collections::HashSet;
use std::path::PathBuf;
use std::str::FromStr;

use tracing::{debug, info};

mod logging;

fn main() -> anyhow::Result<()> {
    logging::setup_logging();

    info!("compiletest -> ui_test header migration tool running");
    info!("usage: tool $PATH_TO_RUSTC_REPO");

    let rustc_repo_path = std::env::args()
        .nth(1)
        .expect("$PATH_TO_RUSTC_REPO required");
    let rustc_repo_path = PathBuf::from_str(&rustc_repo_path).expect("invalid $PATH_TO_RUSTC_REPO");
    debug!(?rustc_repo_path);
    assert!(
        rustc_repo_path.exists(),
        "$PATH_TO_RUSTC_REPO does not exist"
    );

    let walker = walkdir::WalkDir::new(rustc_repo_path.join("tests/ui"))
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| {
            !e.file_type().is_dir() && e.path().extension().map(|s| s == "rs").unwrap_or(false)
        })
        .map(|e| e.into_path());
    let test_file_paths = walker.collect::<HashSet<_>>();

    info!("test_file_paths.len() = {}", test_file_paths.len());

    Ok(())
}
