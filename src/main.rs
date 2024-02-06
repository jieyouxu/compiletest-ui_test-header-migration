#![feature(let_chains)]

use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::Context;
use tracing::{debug, error, info, trace};

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
        .sort_by_file_name()
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| {
            !e.file_type().is_dir() && e.path().extension().map(|s| s == "rs").unwrap_or(false)
        })
        .map(|e| e.into_path());

    let mut test_file_paths = walker.collect::<Vec<_>>();
    test_file_paths.sort();

    info!("test_file_paths.len() = {}", test_file_paths.len());

    for path in test_file_paths.iter().nth(0) {
        trace!(?path);
        let file = File::open(path).with_context(|| format!("cannot open `{:?}`", path))?;
        let out_file = File::create(path.with_extension("rs.out"))?;
        trace!("out_file = {:?}", path.with_extension("rs.out"));
        let mut reader = BufReader::new(file);
        let mut writer = BufWriter::new(out_file);

        let mut line = String::new();

        let mut line_number = 0;

        loop {
            line_number += 1;
            line.clear();
            if reader.read_line(&mut line).unwrap() == 0 {
                break;
            }

            // Assume that any directives will be found before the first item (which may have macro
            // annotations) or e.g. `#![feature(..)]`.
            let line = line.trim();
            if line.starts_with("fn")
                || line.starts_with("mod")
                || line.starts_with("const")
                || line.starts_with("static")
                || line.starts_with("extern")
                || line.starts_with("#")
            {
                write!(writer, "{}\n", line)?;
                break;
            }

            if line.is_empty() {
                write!(writer, "\n")?;
            } else if let Some((start, rest)) = line.split_once("//") {
                if start.is_empty() {
                    // We know that the line begins with `//`, but we cannot discern if it is a
                    // comment or directive yet. In this case, a line is a comment IF it is not a
                    // directive.
                    let rest = rest.trim();

                    // Replace `//` with `//@`.
                    if is_directive(path, line_number, rest) {
                        write!(writer, "//@")?;
                        write!(writer, "{}", rest.trim_start())?;
                        write!(writer, "\n")?;
                    } else {
                        // Do not replace `//` with `//@`.
                        write!(writer, "{}\n", line)?;
                    }
                } else {
                    error!("unexpected line {} in file: `{:?}`", line_number, path);
                    error!("unexpected line {}: `{}`", line_number, line);
                    unimplemented!();
                }
            } else {
                error!("unexpected line {} in file: `{:?}`", line_number, path);
                error!("unexpected line {}: `{}`", line_number, line);
                unimplemented!();
            }
        }

        line.clear();
        while reader.read_line(&mut line).unwrap() != 0 {
            line_number += 1;
            write!(writer, "{}", line)?;
            line.clear();
        }
    }

    Ok(())
}

// TODO: incomplete
fn is_directive(_path: &Path, _line_number: usize, s: &str) -> bool {
    match s {
        "run-pass" => true,
        s if s.starts_with("ignore-") => {
            const PLATFORMS: [&'static str; 4] = ["android", "arm", "aarch64", "windows"];
            for plat in PLATFORMS {
                if s == ["ignore-", plat].concat() {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}
