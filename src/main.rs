//! Git Compression Stats - Analyze compression efficiency in Git repositories.
//!
//! This tool compares the total uncompressed size of all versions of a file
//! against their actual on-disk size in Git's object database (pack files).
//!
//! # Features
//!
//! - Deep history analysis tracking every unique version of files
//! - Accurate storage metrics using `git cat-file` with `%(objectsize:disk)`
//! - Parallel execution with Rayon for concurrent phases
//! - Real-time progress feedback with indicatif
//! - Color-coded compression ratio indicators

use clap::{Parser, ValueEnum};
use human_size::{Byte, SpecificSize};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// CLI arguments for git-compression-stats
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the Git repository to analyze
    #[arg(default_value = ".")]
    repo_path: PathBuf,

    /// Scan the directory for multiple Git repositories
    #[arg(short, long)]
    recursive: bool,

    /// Set the primary sort column
    #[arg(short, long, value_enum, default_value = "path")]
    sort_by: SortBy,

    /// Reverse the sort order
    #[arg(short, long, default_value = "false")]
    descending: bool,

    /// Disable the interactive progress bar
    #[arg(long, default_value = "false")]
    no_progress: bool,

    /// Only analyze files currently in the working tree (much faster)
    #[arg(short, long)]
    current_only: bool,

    /// Use human-readable sizes (KB, MB, GB)
    #[arg(short = 'H', long)]
    human_readable: bool,
}

/// Sort options for the output
#[derive(Debug, Clone, ValueEnum)]
enum SortBy {
    Path,
    Size,
    Versions,
    Uncompressed,
    Compressed,
    Ratio,
}

/// Statistics for a single file
#[derive(Debug, Default, Clone)]
struct FileStats {
    /// Number of unique versions (blobs) of this file
    versions: u64,
    /// Sum of uncompressed sizes of all unique blobs
    total_uncompressed: u64,
    /// Sum of compressed (on-disk) sizes of all unique blobs
    total_compressed: u64,
    /// Size of the file in the most recent version (HEAD)
    latest_size: u64,
}

const COLOR_THRESHOLD: u64 = 1024;

/// ANSI color codes for terminal output
#[derive(Clone, Copy)]
struct Colors {
    reset: &'static str,
    green: &'static str,
    orange: &'static str,
    red: &'static str,
    bold: &'static str,
}

impl Colors {
    fn new() -> Self {
        let use_colors = std::io::stdout().is_terminal();
        if use_colors {
            Self {
                reset: "\x1b[0m",
                green: "\x1b[32m",
                orange: "\x1b[33m",
                red: "\x1b[31m",
                bold: "\x1b[1m",
            }
        } else {
            Self {
                reset: "",
                green: "",
                orange: "",
                red: "",
                bold: "",
            }
        }
    }
}

/// Format bytes into a human-readable string
///
/// # Panics
///
/// Panics if `bytes` is too large to convert to f64 without precision loss.
#[allow(clippy::cast_precision_loss)]
fn format_size(bytes: u64, human: bool) -> String {
    if !human {
        return format!("{bytes}");
    }

    let size =
        SpecificSize::new(bytes as f64, Byte).unwrap_or(SpecificSize::new(0.0, Byte).unwrap());

    if bytes < 1024 {
        format!("{size}")
    } else if bytes < 1024 * 1024 {
        use human_size::Kilobyte;
        format!("{:.1}", size.into::<Kilobyte>())
    } else if bytes < 1024 * 1024 * 1024 {
        use human_size::Megabyte;
        format!("{:.1}", size.into::<Megabyte>())
    } else {
        use human_size::Gigabyte;
        format!("{:.1}", size.into::<Gigabyte>())
    }
}

/// Get color based on compression ratio and file size
fn get_color(colors: &Colors, ratio: f64, size: u64) -> &'static str {
    if size < COLOR_THRESHOLD {
        return "";
    }
    if ratio < 50.0 {
        colors.green
    } else if ratio < 80.0 {
        colors.orange
    } else {
        colors.red
    }
}

/// Find all Git repositories in a directory (recursively)
fn find_git_repos(path: &std::path::Path) -> Vec<PathBuf> {
    let mut repos = Vec::new();
    if path.is_dir() {
        if path.join(".git").exists() {
            repos.push(path.to_path_buf());
            return repos;
        }
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir()
                    && !p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with('.') && n != ".git")
                {
                    repos.extend(find_git_repos(&p));
                }
            }
        }
    }
    repos
}

/// Get sizes for all objects in the repository
///
/// Returns a HashMap mapping object IDs to (uncompressed_size, compressed_size)
fn get_all_sizes(repo_path: &PathBuf) -> HashMap<String, (u64, u64)> {
    let mut all_sizes = HashMap::new();
    let child = Command::new("git")
        .args([
            "cat-file",
            "--batch-check=%(objectname) %(objectsize) %(objectsize:disk)",
            "--batch-all-objects",
        ])
        .current_dir(repo_path)
        .stdout(Stdio::piped())
        .spawn()
        .ok();

    if let Some(mut child) = child {
        let stdout = child.stdout.take().unwrap();
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 {
                let oid = parts[0].to_string();
                let uncompressed = parts[1].parse::<u64>().unwrap_or(0);
                let compressed = parts[2].parse::<u64>().unwrap_or(0);
                all_sizes.insert(oid, (uncompressed, compressed));
            }
        }
        let _ = child.wait();
    }
    all_sizes
}

/// Analyze a single Git repository
///
/// # Errors
///
/// Returns an error if git commands fail or if there are IO errors.
#[allow(clippy::too_many_lines)]
fn analyze_repo(
    repo_path: &PathBuf,
    pb: Option<ProgressBar>,
    current_only: bool,
) -> Result<HashMap<String, FileStats>, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(pb) = &pb {
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .unwrap(),
        );
        pb.set_message(format!("Reading {:?}...", repo_path.display()));
    }

    // Phase 1 and 2: Run object sizing and history in parallel
    let ((all_sizes, current_latest), history_blobs) = rayon::join(
        || {
            // Task A: Object sizes and Task C: Latest sizes
            let sizes = get_all_sizes(repo_path);
            let mut latest = HashMap::new();

            let tree_output = Command::new("git")
                .args(["ls-tree", "-r", "-z", "HEAD"])
                .current_dir(repo_path)
                .output()
                .ok();

            if let Some(output) = tree_output {
                if output.status.success() {
                    let tree_data = String::from_utf8_lossy(&output.stdout);
                    for entry in tree_data.split('\0').filter(|s| !s.is_empty()) {
                        let parts: Vec<&str> = entry.split_whitespace().collect();
                        if parts.len() >= 3 {
                            let oid = parts[2];
                            let filename = entry.split('\t').nth(1).unwrap_or("");
                            if !filename.is_empty() {
                                let (uncompressed, _) = sizes.get(oid).copied().unwrap_or((0, 0));
                                latest.insert(filename.to_string(), uncompressed);
                            }
                        }
                    }
                }
            }
            (sizes, latest)
        },
        || {
            // Task B: History
            if current_only {
                return HashMap::new();
            }

            // Get all commit SHAs
            let shas_output = Command::new("git")
                .args(["rev-list", "--all"])
                .current_dir(repo_path)
                .output()
                .ok();

            let shas: Vec<String> = if let Some(output) = shas_output {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .map(std::string::ToString::to_string)
                    .collect()
            } else {
                Vec::new()
            };

            if let Some(pb) = &pb {
                pb.set_length(shas.len() as u64);
                pb.set_style(
                    ProgressStyle::default_bar()
                        .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}")
                        .unwrap(),
                );
                pb.set_message("Scanning history...");
            }

            // Process chunks in parallel
            let chunk_size = (shas.len() / (rayon::current_num_threads() * 4)).max(100);
            shas.par_chunks(chunk_size)
                .map(|chunk| {
                    let mut chunk_blobs: HashMap<String, HashSet<String>> = HashMap::new();
                    let child = Command::new("git")
                        .args([
                            "diff-tree",
                            "-r",
                            "--raw",
                            "--no-commit-id",
                            "--no-renames",
                            "--root",
                            "--stdin",
                        ])
                        .current_dir(repo_path)
                        .stdin(Stdio::piped())
                        .stdout(Stdio::piped())
                        .spawn()
                        .ok();

                    if let Some(mut child) = child {
                        let mut stdin = child.stdin.take().unwrap();
                        let chunk_owned = chunk.to_vec();
                        std::thread::spawn(move || {
                            for sha in chunk_owned {
                                let _ = writeln!(stdin, "{sha}");
                            }
                        });

                        let stdout = child.stdout.take().unwrap();
                        let reader = BufReader::new(stdout);
                        for line in reader.lines().map_while(Result::ok) {
                            if let Some((metadata, path)) = line.split_once('\t') {
                                let parts: Vec<&str> = metadata.split_whitespace().collect();
                                if parts.len() >= 5 {
                                    let new_oid = parts[3];
                                    if !new_oid.chars().all(|c| c == '0') {
                                        chunk_blobs
                                            .entry(path.to_string())
                                            .or_default()
                                            .insert(new_oid.to_string());
                                    }
                                }
                            }
                        }
                        let _ = child.wait();
                    }

                    if let Some(pb) = &pb {
                        pb.inc(chunk.len() as u64);
                    }

                    chunk_blobs
                })
                .reduce(HashMap::new, |mut a, b| {
                    for (path, set2) in b {
                        a.entry(path).or_default().extend(set2);
                    }
                    a
                })
        },
    );

    if let Some(pb) = &pb {
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .unwrap(),
        );
        pb.set_message(format!(
            "Aggregating results for {:?}...",
            repo_path.display()
        ));
    }

    // Phase 4: Merge results
    let mut final_stats = HashMap::new();

    if current_only {
        // Just the current files
        for (path, latest_size) in current_latest {
            let stats = final_stats.entry(path).or_insert_with(FileStats::default);
            stats.latest_size = latest_size;
            // Since we skipped Task B, we need to get total stats for these current files
            // Wait, if current_only, we only count 1 version
            stats.versions = 1;
            // We need to look up uncompressed/compressed for the current blob
            // This requires we have the OID from the ls-tree. Let's fix Task A above.
            // (Self-correction: I'll re-run Task B logic just for HEAD in the current_only branch if needed)
        }
        // Actually, current_only is best handled by just Phase 1 + a modified Phase 2.
    } else {
        for (path, oids) in history_blobs {
            let mut stats = FileStats {
                versions: oids.len() as u64,
                total_uncompressed: 0,
                total_compressed: 0,
                latest_size: *current_latest.get(&path).unwrap_or(&0),
            };
            for oid in oids {
                let (u, c) = all_sizes.get(&oid).copied().unwrap_or((0, 0));
                stats.total_uncompressed += u;
                stats.total_compressed += c;
            }
            final_stats.insert(path, stats);
        }
    }

    // Special case for current_only if the above was skipped
    if current_only {
        // Rerun a quick pass for HEAD
        let tree_output = Command::new("git")
            .args(["ls-tree", "-r", "-z", "HEAD"])
            .current_dir(repo_path)
            .output()?;
        if tree_output.status.success() {
            let tree_data = String::from_utf8_lossy(&tree_output.stdout);
            for entry in tree_data.split('\0').filter(|s| !s.is_empty()) {
                let parts: Vec<&str> = entry.split_whitespace().collect();
                if parts.len() >= 3 {
                    let oid = parts[2];
                    let filename = entry.split('\t').nth(1).unwrap_or("");
                    if !filename.is_empty() {
                        let (u, c) = all_sizes.get(oid).copied().unwrap_or((0, 0));
                        let stats = final_stats.entry(filename.to_string()).or_default();
                        stats.versions = 1;
                        stats.total_uncompressed = u;
                        stats.total_compressed = c;
                        stats.latest_size = u;
                    }
                }
            }
        }
    }

    if let Some(pb) = pb {
        pb.finish_and_clear();
    }

    Ok(final_stats)
}

/// Main entry point
fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

/// Run the main analysis logic
///
/// # Errors
///
/// Returns an error if repository analysis fails.
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let repos = if args.recursive {
        find_git_repos(&args.repo_path)
    } else {
        vec![args.repo_path.clone()]
    };

    if repos.is_empty() {
        eprintln!(
            "No git repositories found in {:?}",
            args.repo_path.display()
        );
        return Ok(());
    }

    let mp = MultiProgress::new();

    let overall_pb = if args.no_progress {
        None
    } else {
        let pb = mp.add(ProgressBar::new_spinner());
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.green} {msg}")
                .unwrap(),
        );
        pb.set_message("Starting analysis...");
        Some(pb)
    };

    let mut all_stats = HashMap::new();

    // Process repositories sequentially (overkill to parallelize at this level)
    for (i, repo_path) in repos.iter().enumerate() {
        if let Some(pb) = &overall_pb {
            pb.set_message(format!("Analyzing repo {}/{}...", i + 1, repos.len()));
        }

        let pb = if args.no_progress {
            None
        } else {
            Some(mp.add(ProgressBar::new(100)))
        };

        if let Ok(stats) = analyze_repo(repo_path, pb, args.current_only) {
            for (path, file_stat) in stats {
                let full_path = if repos.len() > 1 {
                    format!("{}/{}", repo_path.display(), path)
                } else {
                    path
                };
                let entry = all_stats
                    .entry(full_path)
                    .or_insert_with(FileStats::default);
                entry.versions += file_stat.versions;
                entry.total_uncompressed += file_stat.total_uncompressed;
                entry.total_compressed += file_stat.total_compressed;
                entry.latest_size = file_stat.latest_size;
            }
        }
    }

    if let Some(pb) = &overall_pb {
        pb.set_message(format!("Sorting {} unique files...", all_stats.len()));
    }

    let mut sorted_files: Vec<_> = all_stats.iter().collect();

    // Parallel sort
    #[allow(clippy::cast_precision_loss)]
    sorted_files.par_sort_by(|a, b| {
        let (_, sa) = a;
        let (_, sb) = b;
        let cmp = match args.sort_by {
            SortBy::Path => a.0.cmp(b.0),
            SortBy::Size => sa.latest_size.cmp(&sb.latest_size),
            SortBy::Versions => sa.versions.cmp(&sb.versions),
            SortBy::Uncompressed => sa.total_uncompressed.cmp(&sb.total_uncompressed),
            SortBy::Compressed => sa.total_compressed.cmp(&sb.total_compressed),
            SortBy::Ratio => {
                let ra = if sa.total_uncompressed > 0 {
                    sa.total_compressed as f64 / sa.total_uncompressed as f64
                } else {
                    0.0
                };
                let rb = if sb.total_uncompressed > 0 {
                    sb.total_compressed as f64 / sb.total_uncompressed as f64
                } else {
                    0.0
                };
                ra.partial_cmp(&rb).unwrap_or(std::cmp::Ordering::Equal)
            }
        };
        if args.descending { cmp.reverse() } else { cmp }
    });

    if let Some(pb) = &overall_pb {
        pb.finish_and_clear();
    }
    // Clean up MultiProgress display
    let _ = mp.clear();

    let colors = Colors::new();
    let max_path_len = sorted_files
        .iter()
        .map(|(p, _)| p.len())
        .max()
        .unwrap_or(40)
        .min(60);
    let path_width = max_path_len.max(20);

    println!(
        "{bold}{:<path_width$} {:>14} {:>10} {:>16} {:>16} {:>12}{reset}",
        "File",
        "Size",
        "Versions",
        "Total Uncomp.",
        "Total Comp.",
        "Ratio",
        bold = colors.bold,
        reset = colors.reset,
        path_width = path_width
    );
    println!("{}", "-".repeat(path_width + 84));

    for (path, stats) in &sorted_files {
        let size = format_size(stats.latest_size, args.human_readable);
        let tu = format_size(stats.total_uncompressed, args.human_readable);
        let tc = format_size(stats.total_compressed, args.human_readable);
        #[allow(clippy::cast_precision_loss)]
        let ratio = if stats.total_uncompressed > 0 {
            (stats.total_compressed as f64 / stats.total_uncompressed as f64) * 100.0
        } else {
            0.0
        };
        let color = get_color(&colors, ratio, stats.latest_size);
        let dp = if path.len() > path_width {
            format!("...{}", &path[path.len() - (path_width - 3)..])
        } else {
            path.to_string()
        };
        let rs = format!(
            "{color}{:>10.1}%{reset}",
            ratio,
            color = color,
            reset = if color.is_empty() { "" } else { colors.reset }
        );
        println!(
            "{:<path_width$} {:>14} {:>10} {:>16} {:>16} {}",
            dp,
            size,
            stats.versions,
            tu,
            tc,
            rs,
            path_width = path_width
        );
    }

    println!("{}", "-".repeat(path_width + 84));
    let tf: u64 = sorted_files.iter().map(|(_, s)| s.latest_size).sum();
    let tv: u64 = sorted_files.iter().map(|(_, s)| s.versions).sum();
    let tu: u64 = sorted_files.iter().map(|(_, s)| s.total_uncompressed).sum();
    let tc: u64 = sorted_files.iter().map(|(_, s)| s.total_compressed).sum();
    #[allow(clippy::cast_precision_loss)]
    let g = if tu > 0 {
        (tc as f64 / tu as f64) * 100.0
    } else {
        0.0
    };

    println!(
        "{bold}{:<path_width$} {:>14} {:>10} {:>16} {:>16} {:>11.1}%{reset}",
        "TOTAL",
        format_size(tf, args.human_readable),
        tv,
        format_size(tu, args.human_readable),
        format_size(tc, args.human_readable),
        g,
        bold = colors.bold,
        reset = colors.reset,
        path_width = path_width
    );

    Ok(())
}
