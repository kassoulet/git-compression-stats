use clap::Parser;
use human_size::{Byte, SpecificSize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Analyze git repository file compression statistics
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the git repository (defaults to current directory)
    #[arg(default_value = ".")]
    repo_path: PathBuf,
}

#[derive(Debug, Default)]
struct FileStats {
    versions: u64,
    total_uncompressed: u64,
    total_compressed: u64,
    latest_size: u64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Open the repository
    let repo = gix::open(&args.repo_path)?;

    // Collect all file statistics across all commits
    let mut file_stats: HashMap<String, FileStats> = HashMap::new();

    // Get the head commit and walk through all commits
    let mut head = repo.head()?;
    let head_commit = head.peel_to_commit_in_place()?;

    // Create a revwalk to iterate through all commits
    let revwalk = repo.rev_walk([head_commit.id]).all()?;

    // Track unique commits to avoid processing duplicates
    let mut processed_commits = std::collections::HashSet::new();

    for commit_info in revwalk {
        let commit_id = commit_info?.id;

        // Skip if already processed
        if !processed_commits.insert(commit_id) {
            continue;
        }

        let commit = repo.find_commit(commit_id)?;
        let tree = commit.tree()?;

        // Iterate through all entries in the tree
        for entry in tree.iter() {
            let entry = entry?;

            // Get the file path from the entry
            let path = entry.filename().to_string();

            // Skip non-blob entries (directories, submodules, etc.)
            if entry.mode().kind() != gix::object::tree::EntryKind::Blob.into() {
                continue;
            }

            let oid = entry.oid();
            let blob = repo.find_blob(oid)?;

            // Get uncompressed size
            let uncompressed_size = blob.data.len() as u64;

            // Get compressed size - try to get from ODB
            let compressed_size = get_object_size_on_disk(&repo, oid.to_owned()).unwrap_or(uncompressed_size);

            let stats = file_stats.entry(path).or_default();
            stats.versions += 1;
            stats.total_uncompressed += uncompressed_size;
            stats.total_compressed += compressed_size;
            stats.latest_size = uncompressed_size;
        }
    }

    // Print header
    println!(
        "{:<60} {:>12} {:>10} {:>15} {:>15} {:>12}",
        "File", "Size", "Versions", "Total Uncomp.", "Total Comp.", "Compression"
    );
    println!("{}", "-".repeat(130));

    // Sort files by path for consistent output
    let mut sorted_files: Vec<_> = file_stats.iter().collect();
    sorted_files.sort_by(|a, b| a.0.cmp(b.0));

    // Print statistics for each file
    for (path, stats) in sorted_files {
        let size = SpecificSize::<Byte>::new(stats.latest_size as f64, Byte::default())?;
        let total_uncomp =
            SpecificSize::<Byte>::new(stats.total_uncompressed as f64, Byte::default())?;
        let total_comp = SpecificSize::<Byte>::new(stats.total_compressed as f64, Byte::default())?;

        let compression_ratio = if stats.total_uncompressed > 0 {
            (stats.total_compressed as f64 / stats.total_uncompressed as f64) * 100.0
        } else {
            0.0
        };

        // Truncate long paths
        let display_path = if path.len() > 58 {
            format!("...{}", &path[path.len() - 55..])
        } else {
            path.clone()
        };

        println!(
            "{:<60} {:>12} {:>10} {:>15} {:>15} {:>11.1}%",
            display_path, size, stats.versions, total_uncomp, total_comp, compression_ratio
        );
    }

    // Print summary
    println!("{}", "-".repeat(130));

    let total_files: u64 = file_stats.values().map(|s| s.latest_size).sum();
    let total_versions: u64 = file_stats.values().map(|s| s.versions).sum();
    let total_uncompressed: u64 = file_stats.values().map(|s| s.total_uncompressed).sum();
    let total_compressed: u64 = file_stats.values().map(|s| s.total_compressed).sum();

    let global_compression = if total_uncompressed > 0 {
        (total_compressed as f64 / total_uncompressed as f64) * 100.0
    } else {
        0.0
    };

    println!(
        "{:<60} {:>12} {:>10} {:>15} {:>15} {:>11.1}%",
        "TOTAL",
        SpecificSize::<Byte>::new(total_files as f64, Byte::default())?,
        total_versions,
        SpecificSize::<Byte>::new(total_uncompressed as f64, Byte::default())?,
        SpecificSize::<Byte>::new(total_compressed as f64, Byte::default())?,
        global_compression
    );

    Ok(())
}

/// Get the actual size of an object on disk (for loose objects)
fn get_object_size_on_disk(repo: &gix::Repository, oid: gix::ObjectId) -> Option<u64> {
    // Construct the loose object path manually
    // Git stores loose objects as .git/objects/XX/YYYY... where XX is first 2 chars of hash
    let hex = oid.to_string();
    if hex.len() >= 2 {
        let dir = &hex[..2];
        let file = &hex[2..];
        let objects_dir = repo.path().join("objects");
        let loose_path = objects_dir.join(dir).join(file);
        
        if loose_path.exists() {
            return std::fs::metadata(&loose_path).ok().map(|m| m.len());
        }
    }
    
    // For packed objects, we use the uncompressed size as approximation
    None
}
// Version 2
// Version 3
