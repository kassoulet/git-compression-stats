use clap::{Parser, ValueEnum};
use human_size::{Byte, SpecificSize};
use indicatif::{ProgressBar, ProgressStyle};
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

/// Analyze git repository file compression statistics
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the git repository (defaults to current directory)
    #[arg(default_value = ".")]
    repo_path: PathBuf,

    /// Recursively search for git repositories in subdirectories
    #[arg(short, long)]
    recursive: bool,

    /// Sort output by column
    #[arg(short, long, value_enum, default_value = "path")]
    sort_by: SortBy,

    /// Sort in descending order (largest first)
    #[arg(short, long, default_value = "false")]
    descending: bool,

    /// Disable progress bar
    #[arg(long, default_value = "false")]
    no_progress: bool,
}

#[derive(Debug, Clone, ValueEnum)]
enum SortBy {
    Path,
    Size,
    Versions,
    Uncompressed,
    Compressed,
    Ratio,
}

#[derive(Debug, Default)]
struct FileStats {
    versions: u64,
    total_uncompressed: u64,
    total_compressed: u64,
    latest_size: u64,
    /// Track unique blob hashes to count actual versions (content changes)
    unique_blobs: HashSet<String>,
    /// Track unique blob sizes to avoid double-counting
    blob_sizes: HashMap<String, (u64, u64)>, // hash -> (uncompressed, compressed)
}

/// ANSI color codes
const RESET: &str = "\x1b[0m";
const GREEN: &str = "\x1b[32m";
const ORANGE: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const BOLD: &str = "\x1b[1m";

/// Minimum file size to apply color coding (1 KB)
const COLOR_THRESHOLD: u64 = 1024;

/// Format a size value as a human-readable string with fixed width
fn format_size(bytes: u64) -> String {
    match SpecificSize::<Byte>::new(bytes as f64, Byte::default()) {
        Ok(size) => format!("{}", size),
        Err(_) => format!("{} B", bytes),
    }
}

/// Get color based on compression ratio
fn get_color(ratio: f64, size: u64) -> &'static str {
    // Only color files over 1KB
    if size < COLOR_THRESHOLD {
        return "";
    }
    
    if ratio < 50.0 {
        GREEN
    } else if ratio < 80.0 {
        ORANGE
    } else {
        RED
    }
}

/// Find all git repositories in a directory (recursive)
fn find_git_repos(path: &std::path::Path) -> Vec<PathBuf> {
    let mut repos = Vec::new();

    if path.is_dir() {
        let git_path = path.join(".git");
        if git_path.exists() {
            repos.push(path.to_path_buf());
            // Don't recurse into a git repo's internals
            return repos;
        }

        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let entry_path = entry.path();
                if entry_path.is_dir() {
                    // Skip hidden directories except .git
                    if entry_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with('.') && n != ".git")
                        .unwrap_or(false)
                    {
                        continue;
                    }
                    repos.extend(find_git_repos(&entry_path));
                }
            }
        }
    }

    repos
}

/// Get the actual size of an object on disk
fn get_object_size_on_disk(
    repo: &gix::Repository,
    oid: gix::ObjectId,
    cache: &Arc<Mutex<HashMap<String, u64>>>,
) -> Option<u64> {
    // Check cache first
    let oid_str = oid.to_string();
    {
        let cache_guard = cache.lock().ok()?;
        if let Some(&size) = cache_guard.get(&oid_str) {
            return Some(size);
        }
    }

    // First try loose objects
    let hex = oid.to_string();
    if hex.len() >= 2 {
        let dir = &hex[..2];
        let file = &hex[2..];
        let objects_dir = repo.path().join("objects");
        let loose_path = objects_dir.join(dir).join(file);

        if loose_path.exists() {
            let size = std::fs::metadata(&loose_path).ok().map(|m| m.len())?;
            cache.lock().ok()?.insert(oid_str, size);
            return Some(size);
        }
    }

    // For packed objects, get the blob data and compress it to estimate size
    if let Ok(blob) = repo.find_blob(oid) {
        let data = &blob.data;
        
        // Empty files have minimal compressed size
        if data.is_empty() {
            cache.lock().ok()?.insert(oid_str, 0);
            return Some(0);
        }
        
        // Compress the data with zlib to get actual compressed size
        let mut encoder = flate2::write::ZlibEncoder::new(
            Vec::new(),
            flate2::Compression::default()
        );
        if encoder.write_all(data).is_ok() {
            if let Ok(compressed) = encoder.finish() {
                // Add git object header overhead (typically ~20-40 bytes)
                let size = compressed.len() as u64 + 30;
                cache.lock().ok()?.insert(oid_str, size);
                return Some(size);
            }
        }
    }

    None
}

/// Recursively walk a tree and collect file entries with full paths
fn walk_tree(
    tree: &gix::Tree,
    repo: &gix::Repository,
    prefix: &str,
    file_stats: &mut HashMap<String, FileStats>,
    cache: &Arc<Mutex<HashMap<String, u64>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in tree.iter() {
        let entry = entry?;
        let name = entry.filename().to_string();
        let full_path = if prefix.is_empty() {
            name
        } else {
            format!("{}/{}", prefix, name)
        };

        // Skip non-blob entries (directories, submodules, etc.)
        if entry.mode().kind() != gix::object::tree::EntryKind::Blob.into() {
            // Recurse into subdirectories
            if entry.mode().kind() == gix::object::tree::EntryKind::Tree.into() {
                let subtree = repo.find_tree(entry.oid())?;
                walk_tree(&subtree, repo, &full_path, file_stats, cache)?;
            }
            continue;
        }

        let oid = entry.oid();
        let blob = repo.find_blob(oid)?;
        let blob_hash = oid.to_string();

        // Get uncompressed size
        let uncompressed_size = blob.data.len() as u64;

        // Get compressed size - try to get from ODB
        let compressed_size = get_object_size_on_disk(repo, oid.to_owned(), cache).unwrap_or(uncompressed_size);

        let stats = file_stats.entry(full_path).or_default();

        // Track unique blobs to count actual versions (content changes)
        if stats.unique_blobs.insert(blob_hash.clone()) {
            // New unique blob - count it
            stats.versions += 1;
            stats.total_uncompressed += uncompressed_size;
            stats.total_compressed += compressed_size;
            stats.blob_sizes.insert(blob_hash, (uncompressed_size, compressed_size));
            
            // First occurrence is the latest (revwalk goes newest-first)
            if stats.latest_size == 0 {
                stats.latest_size = uncompressed_size;
            }
        }
    }
    Ok(())
}

/// Analyze a single git repository
fn analyze_repo(
    repo_path: &PathBuf,
    pb: Option<&ProgressBar>,
) -> Result<HashMap<String, FileStats>, Box<dyn std::error::Error>> {
    let repo = gix::open(repo_path)?;
    let mut file_stats: HashMap<String, FileStats> = HashMap::new();
    
    // Cache for compressed sizes (shared across all commits)
    let cache: Arc<Mutex<HashMap<String, u64>>> = Arc::new(Mutex::new(HashMap::new()));

    // Get the head commit and walk through all commits
    let mut head = repo.head()?;
    let head_commit = head.peel_to_commit_in_place()?;

    // Create a revwalk to iterate through all commits
    let revwalk = repo.rev_walk([head_commit.id]).all()?;

    // Collect all commit IDs first for progress tracking
    let commit_ids: Vec<_> = revwalk.filter_map(|r| r.ok()).collect();
    let total_commits = commit_ids.len() as u64;

    // Set up progress bar with correct length
    if let Some(pb) = pb {
        pb.set_length(total_commits);
        pb.set_position(0);
    }

    // Track unique commits to avoid processing duplicates
    let mut processed_commits = std::collections::HashSet::new();

    for (i, commit_info) in commit_ids.into_iter().enumerate() {
        let commit_id = commit_info.id;

        // Update progress
        if let Some(pb) = pb {
            pb.set_position(i as u64 + 1);
        }

        // Skip if already processed
        if !processed_commits.insert(commit_id) {
            continue;
        }

        let commit = repo.find_commit(commit_id)?;
        let tree = commit.tree()?;

        // Walk the tree recursively to get all entries with full paths
        walk_tree(&tree, &repo, "", &mut file_stats, &cache)?;
    }

    // Clean up tracking fields before returning
    for stats in file_stats.values_mut() {
        stats.unique_blobs.clear();
        stats.blob_sizes.clear();
    }

    Ok(file_stats)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Find all repositories to analyze
    let repos = if args.recursive {
        find_git_repos(&args.repo_path)
    } else {
        vec![args.repo_path.clone()]
    };

    if repos.is_empty() {
        eprintln!("No git repositories found in {:?}", args.repo_path);
        return Ok(());
    }

    // Set up progress bar
    let pb = if !args.no_progress {
        let pb = ProgressBar::new(100);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}")?
                .progress_chars("=>-"),
        );
        pb.enable_steady_tick(std::time::Duration::from_millis(100));
        Some(pb)
    } else {
        None
    };

    // Collect all file statistics across all repositories
    let mut all_stats: HashMap<String, FileStats> = HashMap::new();

    for repo_path in &repos {
        if let Some(pb) = &pb {
            pb.set_message(format!(
                "{}",
                repo_path.display()
            ));
        }

        match analyze_repo(repo_path, pb.as_ref()) {
            Ok(stats) => {
                // Merge stats, prefixing paths with repo name if multiple repos
                for (path, file_stat) in stats {
                    let full_path = if repos.len() > 1 {
                        format!("{}/{}", repo_path.display(), path)
                    } else {
                        path
                    };

                    let entry = all_stats.entry(full_path).or_default();
                    entry.versions += file_stat.versions;
                    entry.total_uncompressed += file_stat.total_uncompressed;
                    entry.total_compressed += file_stat.total_compressed;
                    entry.latest_size = file_stat.latest_size;
                }
            }
            Err(e) => {
                eprintln!("Warning: Could not analyze {:?}: {}", repo_path, e);
            }
        }
    }

    if let Some(pb) = &pb {
        pb.finish_with_message("Analysis complete!");
    }

    // Sort files based on user preference
    let mut sorted_files: Vec<_> = all_stats.iter().collect();

    sorted_files.sort_by(|a, b| {
        let (_, stats_a) = a;
        let (_, stats_b) = b;

        let cmp = match args.sort_by {
            SortBy::Path => a.0.cmp(b.0),
            SortBy::Size => stats_a.latest_size.cmp(&stats_b.latest_size),
            SortBy::Versions => stats_a.versions.cmp(&stats_b.versions),
            SortBy::Uncompressed => stats_a.total_uncompressed.cmp(&stats_b.total_uncompressed),
            SortBy::Compressed => stats_a.total_compressed.cmp(&stats_b.total_compressed),
            SortBy::Ratio => {
                let ratio_a = if stats_a.total_uncompressed > 0 {
                    stats_a.total_compressed as f64 / stats_a.total_uncompressed as f64
                } else {
                    0.0
                };
                let ratio_b = if stats_b.total_uncompressed > 0 {
                    stats_b.total_compressed as f64 / stats_b.total_uncompressed as f64
                } else {
                    0.0
                };
                ratio_a.partial_cmp(&ratio_b).unwrap_or(std::cmp::Ordering::Equal)
            }
        };

        if args.descending {
            cmp.reverse()
        } else {
            cmp
        }
    });

    // Calculate column widths based on actual data
    let max_path_len = sorted_files
        .iter()
        .map(|(p, _)| p.len())
        .max()
        .unwrap_or(40)
        .min(60);
    let path_width = max_path_len.max(20);

    // Print header
    println!(
        "{bold}{:<path_width$} {:>14} {:>10} {:>16} {:>16} {:>12}{reset}",
        "File", "Size", "Versions", "Total Uncomp.", "Total Comp.", "Ratio",
        bold = BOLD,
        reset = RESET,
        path_width = path_width
    );
    println!("{}", "-".repeat(path_width + 84));

    // Print statistics for each file
    for (path, stats) in &sorted_files {
        let size = format_size(stats.latest_size);
        let total_uncomp = format_size(stats.total_uncompressed);
        let total_comp = format_size(stats.total_compressed);

        let compression_ratio = if stats.total_uncompressed > 0 {
            (stats.total_compressed as f64 / stats.total_uncompressed as f64) * 100.0
        } else {
            0.0
        };

        // Get color based on ratio and file size
        let color = get_color(compression_ratio, stats.latest_size);

        // Truncate long paths
        let display_path = if path.len() > path_width {
            format!("...{}", &path[path.len() - (path_width - 3)..])
        } else {
            path.to_string()
        };

        // Apply color to the ratio value
        let ratio_str = format!(
            "{color}{:>10.1}%{reset}",
            compression_ratio,
            color = color,
            reset = if color.is_empty() { "" } else { RESET }
        );

        println!(
            "{:<path_width$} {:>14} {:>10} {:>16} {:>16} {}",
            display_path, size, stats.versions, total_uncomp, total_comp, ratio_str,
            path_width = path_width
        );
    }

    // Print summary
    println!("{}", "-".repeat(path_width + 84));

    let total_files: u64 = sorted_files.iter().map(|(_, s)| s.latest_size).sum();
    let total_versions: u64 = sorted_files.iter().map(|(_, s)| s.versions).sum();
    let total_uncompressed: u64 = sorted_files.iter().map(|(_, s)| s.total_uncompressed).sum();
    let total_compressed: u64 = sorted_files.iter().map(|(_, s)| s.total_compressed).sum();

    let global_compression = if total_uncompressed > 0 {
        (total_compressed as f64 / total_uncompressed as f64) * 100.0
    } else {
        0.0
    };

    println!(
        "{bold}{:<path_width$} {:>14} {:>10} {:>16} {:>16} {:>11.1}%{reset}",
        "TOTAL",
        format_size(total_files),
        total_versions,
        format_size(total_uncompressed),
        format_size(total_compressed),
        global_compression,
        bold = BOLD,
        reset = RESET,
        path_width = path_width
    );

    Ok(())
}
