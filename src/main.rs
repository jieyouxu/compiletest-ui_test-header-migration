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
    info!("usage: cargo r -- $PATH_TO_RUSTC_REPO");

    const TARGET: &str = "x86_64-apple-darwin";

    let rustc_repo_path = std::env::args()
        .nth(1)
        .expect("$PATH_TO_RUSTC_REPO required");
    let rustc_repo_path = PathBuf::from_str(&rustc_repo_path).expect("invalid $PATH_TO_RUSTC_REPO");
    debug!(?rustc_repo_path);
    assert!(
        rustc_repo_path.exists(),
        "$PATH_TO_RUSTC_REPO does not exist"
    );

    let mut collected_headers_path = PathBuf::new();
    collected_headers_path.push(&rustc_repo_path);
    collected_headers_path.push("build");
    collected_headers_path.push(TARGET);
    collected_headers_path.push("test/ui");
    collected_headers_path.push("__directive_lines");

    // Load collected headers (mainly EarlyProps)
    debug!(early_headers_path = ?collected_headers_path.with_extension("txt"));
    let early_collected_headers =
        std::fs::read_to_string(collected_headers_path.with_extension("txt"))
            .context("failed to read collected headers")?;
    let mut collected_headers = early_collected_headers
        .lines()
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<String>>();

    // Load collected headers (from TestProps collected from each ran UI test)
    let collected_headers_walker = walkdir::WalkDir::new(collected_headers_path.with_extension(""))
        .sort_by_file_name()
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| !e.file_type().is_dir())
        .map(|e| e.into_path());

    let mut collected_header_paths = collected_headers_walker.collect::<Vec<_>>();
    collected_header_paths.sort();

    info!(
        "there are {} collected header files",
        collected_header_paths.len()
    );

    for path in collected_header_paths {
        debug!(?path, "processing collected header");
        let file = std::fs::File::open(&path)?;
        let reader = std::io::BufReader::new(file);
        for line in reader.lines() {
            let line = line?;
            collected_headers.insert(line);
        }
    }

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
