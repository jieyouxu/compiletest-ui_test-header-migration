#![feature(let_chains)]

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::Context;
use tracing::*;

mod logging;

fn main() -> anyhow::Result<()> {
    logging::setup_logging();

    info!("compiletest -> ui_test header migration tool");
    info!("usage: tool $PATH_TO_COLLECTED_DIRECTIVES $PATH_TO_RUSTC_REPO");

    let collected_directives_path = std::env::args()
        .nth(1)
        .expect("$PATH_TO_COLLECTED_DIRECTIVES required");
    let collected_directives_path = PathBuf::from_str(&collected_directives_path)
        .expect("invalid $PATH_TO_COLLECTED_DIRECTIVES");
    debug!(?collected_directives_path);
    assert!(
        collected_directives_path.exists(),
        "$PATH_TO_COLLECTED_DIRECTIVES does not exist"
    );
    let rustc_repo_path = std::env::args()
        .nth(2)
        .expect("$PATH_TO_RUSTC_REPO required");
    let rustc_repo_path = PathBuf::from_str(&rustc_repo_path).expect("invalid $PATH_TO_RUSTC_REPO");
    debug!(?rustc_repo_path);
    assert!(
        rustc_repo_path.exists(),
        "$PATH_TO_RUSTC_REPO does not exist"
    );

    // Load collected headers
    let collected_headers = std::fs::read_to_string(&collected_directives_path)
        .context("failed to read collected headers")?;
    let collected_headers = collected_headers.lines().collect::<BTreeSet<&str>>();
    info!("there are {} collected headers", collected_headers.len());

    // Collect paths of ui test files
    let walker = walkdir::WalkDir::new(rustc_repo_path.join("tests/ui"))
        .sort_by_file_name()
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| {
            !e.file_type().is_dir() && e.path().extension().map(|s| s == "rs").unwrap_or(false)
        })
        .map(|e| e.into_path());

    let mut test_file_paths = walker.collect::<Vec<_>>();
    test_file_paths.sort();

    info!("there are {} ui test files", test_file_paths.len());

    for path in test_file_paths {
        debug!(?path, "processing file");
        // - Read the contents of the ui test file
        // - Open a named temporary file
        // - Process each line of the ui test:
        //     - If line starts with "//", try to match it with one of the collected directives.
        //       If a match is found, replace "//" with "//@" and append line to temp file.
        //     - Otherwise, append line verbatim to temp file.
        // - Replace original ui test with temp file.
        let ui_test_file = std::fs::File::open(&path)?;
        let reader = std::io::BufReader::new(ui_test_file);

        let mut tmp_file = tempfile::NamedTempFile::new()?;

        'line: for line in reader.lines() {
            let line = line?;

            if line.trim_start().starts_with("//") {
                let (_, rest) = line.trim_start().split_once("//").unwrap();
                let rest = rest.trim();

                for header in collected_headers.iter() {
                    if rest == *header {
                        writeln!(tmp_file, "//@{}", rest)?;
                        continue 'line;
                    }
                }

                // No matched directive, very unlikely a directive and instead just a comment
                writeln!(tmp_file, "{}", line)?;
            } else {
                writeln!(tmp_file, "{}", line)?;
            }
        }

        let tmp_path = tmp_file.into_temp_path();
        tmp_path.persist(path)?;
    }

    Ok(())
}
