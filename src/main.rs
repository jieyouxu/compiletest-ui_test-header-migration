#![feature(let_chains)]

use std::collections::BTreeSet;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use clap::{Parser, Subcommand};
use confique::{toml::FormatOptions, Config as ConfigParser};
use indicatif::{ProgressBar, ProgressIterator, ProgressStyle};
use tracing::*;

mod logging;

const TARGET: &str = env!("TARGET");

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Generate a default config file at `$CWD/migration_config.toml`.
    GenerateConfig,
    /// Using the collected headers from `rustc` repo generated by the collection tool,
    /// replace all `//` directives with `//@` directives.
    Migrate {
        /// Path to the `rustc` repo to operate the tool on. Note that this tool consumes output
        /// generated by a test directive collection script beforehand.
        #[clap(value_name = "PATH_TO_RUSTC")]
        path_to_rustc: PathBuf,
    },
    /// From the collected headers from `rustc` repo generated by the collection tool, output
    /// a Rust array consisting of directive names (does not include revisions or values or
    /// comments).
    CollectDirectiveNames {
        /// Path to the `rustc` repo to operate the tool on. Note that this tool consumes output
        /// generated by a test directive collection script beforehand.
        #[clap(value_name = "PATH_TO_RUSTC")]
        path_to_rustc: PathBuf,
    },
}

#[derive(Debug, Default, ConfigParser)]
pub(crate) struct Config {
    /// Manually specify directives. This is mostly used to override some special tests that are
    /// not properly handled by the collection script.
    #[config(default = [])]
    pub(crate) manual_directives: Vec<String>,
}

fn main() -> anyhow::Result<()> {
    logging::setup_logging();

    let config_path = PathBuf::from("migration_config.toml");
    if !config_path.exists() {
        info!("migration_config.toml does not exist, default values will be used");
    }
    let config = Config::from_file(&config_path).unwrap_or_default();
    debug!(?config);

    let cli = Cli::parse();
    debug!(?cli);

    match &cli.command {
        Command::GenerateConfig => {
            if !config_path.exists() {
                let template = confique::toml::template::<Config>(FormatOptions::default());
                std::fs::write(&config_path, template)?;
            } else {
                error!("migration_config.toml already exists");
                eprintln!("migration_config.toml already exists, no config will be generated");
                bail!("migration_config.toml already exists!");
            }
        }
        Command::Migrate { path_to_rustc } => {
            let mut collected_headers = collect_headers(path_to_rustc.as_path())?;
            collected_headers.extend(config.manual_directives);
            migrate_compiletest_tests(path_to_rustc.as_path(), &collected_headers)?;
        }
        Command::CollectDirectiveNames { path_to_rustc } => {
            let mut collected_headers = collect_headers(path_to_rustc.as_path())?;
            collected_headers.extend(config.manual_directives);
            let directive_names = extract_directive_names(&collected_headers)?;
            println!("{:?}", directive_names.iter().collect::<Vec<_>>());
        }
    }

    Ok(())
}

fn collect_headers(path_to_rustc: &Path) -> anyhow::Result<BTreeSet<String>> {
    debug!(?path_to_rustc);
    assert!(path_to_rustc.exists(), "$PATH_TO_RUSTC_REPO does not exist");

    let mut collected_headers_path = PathBuf::new();
    collected_headers_path.push(&path_to_rustc);
    collected_headers_path.push("build");
    collected_headers_path.push(TARGET);
    collected_headers_path.push("test");
    collected_headers_path.push("__directive_lines");

    // Load collected headers (mainly EarlyProps)
    debug!(headers_path = ?collected_headers_path.with_extension("txt"));
    let collected_headers = std::fs::read_to_string(collected_headers_path.with_extension("txt"))
        .context("failed to read collected headers")?;
    let mut collected_headers = collected_headers
        .lines()
        .map(ToOwned::to_owned)
        .collect::<BTreeSet<String>>();

    collected_headers.retain(|header| {
        !header.trim().is_empty() // skip empty header
        && header.trim() != "//" // skip empty comment
        && !header.trim().starts_with('#') // skip makefile headers
        && header.split_once("//").map(|(_, post)| {
            !post.trim().starts_with("ignore-tidy")
        }).unwrap_or(true)
    });

    info!("there are {} collected headers", collected_headers.len());

    Ok(collected_headers)
}

fn migrate_compiletest_tests(
    path_to_rustc: &Path,
    collected_headers: &BTreeSet<String>,
) -> anyhow::Result<()> {
    // Collect paths of compiletest test files
    let walker = walkdir::WalkDir::new(path_to_rustc.join("tests"))
        .sort_by_file_name()
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| {
            !e.file_type().is_dir()
                && e.path()
                    .extension()
                    .map(|s| s == "rs" || s == "fixed")
                    .unwrap_or(false)
                // We already migrated ui test suite tests
                && !e.path().starts_with(path_to_rustc.join("tests").join("ui"))
        })
        .map(|e| e.into_path());

    let mut test_file_paths = walker.collect::<Vec<_>>();
    test_file_paths.sort();

    info!("there are {} compiletest test files", test_file_paths.len());

    let pb = ProgressBar::new(test_file_paths.len() as u64);
    pb.set_style(
        ProgressStyle::with_template(
            "migrating compiletest tests: {spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] ({pos}/{len}, ETA {eta})",
        )
        .unwrap(),
    );

    for path in test_file_paths.iter().progress_with(pb) {
        debug!(?path, "processing file");
        // - Read the contents of the compiletest test file
        // - Open a named temporary file
        // - Process each line of the compiletest test:
        //     - If line starts with "//", try to match it with one of the collected directives.
        //       If a match is found, replace "//" with "//@" and append line to temp file.
        //     - Otherwise, append line verbatim to temp file.
        // - Replace original compiletest test with temp file.
        let compiletest_test_file = std::fs::File::open(&path)?;
        let mut reader = std::io::BufReader::new(compiletest_test_file);

        let mut tmp_file = tempfile::NamedTempFile::new()?;

        let mut line_buf = String::new();
        'line: loop {
            line_buf.clear();
            let bytes_read = reader.read_line(&mut line_buf)?;
            if bytes_read == 0 {
                break;
            }

            if line_buf.trim_start().starts_with("//") {
                let (before, after) = line_buf.split_once("//").unwrap();

                for header in collected_headers.iter() {
                    if line_buf.replace("\r", "").replace("\n", "") == *header {
                        write!(tmp_file, "{}//@{}", before, after)?;
                        continue 'line;
                    }
                }

                // No matched directive, very unlikely a directive and instead just a comment
                write!(tmp_file, "{}", line_buf)?;
            } else {
                write!(tmp_file, "{}", line_buf)?;
            }
        }

        let tmp_path = tmp_file.into_temp_path();
        tmp_path.persist(path)?;
    }

    Ok(())
}

fn extract_directive_names(
    collected_directives: &BTreeSet<String>,
) -> anyhow::Result<BTreeSet<String>> {
    let mut ret = BTreeSet::new();

    for raw_directive in collected_directives {
        // Directives can take the forms:
        // 1. `// name` or with value or with comments:
        //     - `// name: <rest>`
        //     - `// name <rest>`
        // 2. `//[rev] name` or with value or with commments:
        //     - `//[rev] name: ...`
        //     - `//[rev] name ...`
        // There may be arbitrary whitespace between `//`, `[rev]` and `name`.

        // First, let's get rid of the `//`.
        let Some((leading, rest)) = raw_directive.split_once("//") else {
            bail!("failed to split `{}`", raw_directive);
        };
        assert!(
            leading.trim().is_empty(),
            "expected directive to be leading in the line, there's a bug in the collection script"
        );
        let rest = rest.trim_start();

        // Next, let's get rid of revisions.
        let mut rest = if let Some(lbracket_pos) = rest.find('[')
            && rest.starts_with('[')
        {
            let Some(rbracket_pos) = rest.find(']') else {
                error!(
                    ?raw_directive,
                    ?lbracket_pos,
                    "weird directive: `{:?}`",
                    rest
                );
                panic!("directive found with unpaired [] delimiters");
            };
            if lbracket_pos > rbracket_pos {
                error!(
                    ?raw_directive,
                    ?lbracket_pos,
                    ?rbracket_pos,
                    "weird directive: `{:?}`",
                    rest
                );
            }
            assert!(lbracket_pos <= rbracket_pos);
            let rest = &rest[(rbracket_pos + 1)..];
            rest.trim_start()
        } else {
            rest.trim_start()
        };

        // Special case: one of the test files has some weird syntax like
        // `// [rev]: directive-name`...
        if rest.starts_with(':') {
            // ... so skip that pesky colon.
            rest = &rest[1..];
            rest = rest.trim_start();
        }

        // Now, let's extract the directive name.
        let directive_name = if let Some((directive_name, _)) = rest.split_once([':', ' ']) {
            directive_name.trim()
        } else {
            let directive_name = rest;
            assert!(!directive_name.trim().contains([' ']));
            directive_name.trim()
        };

        ret.insert(directive_name.to_owned());
    }

    Ok(ret)
}
