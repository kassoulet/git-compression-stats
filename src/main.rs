use clap::{Parser, ValueEnum};
use human_size::{Byte, SpecificSize};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(default_value = ".")]
    repo_path: PathBuf,
    #[arg(short, long)]
    recursive: bool,
    #[arg(short, long, value_enum, default_value = "path")]
    sort_by: SortBy,
    #[arg(short, long, default_value = "false")]
    descending: bool,
    #[arg(long, default_value = "false")]
    no_progress: bool,
    /// Only analyze files currently in the working tree (much faster)
    #[arg(short, long)]
    current_only: bool,
    /// Use human-readable sizes (KB, MB, GB)
    #[arg(short = 'H', long)]
    human_readable: bool,
}

#[derive(Debug, Clone, ValueEnum)]
enum SortBy {
    Path, Size, Versions, Uncompressed, Compressed, Ratio,
}

#[derive(Debug, Default, Clone)]
struct FileStats {
    versions: u64,
    total_uncompressed: u64,
    total_compressed: u64,
    latest_size: u64,
}

const RESET: &str = "\x1b[0m";
const GREEN: &str = "\x1b[32m";
const ORANGE: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const BOLD: &str = "\x1b[1m";
const COLOR_THRESHOLD: u64 = 1024;

fn format_size(bytes: u64, human: bool) -> String {
    if !human {
        return format!("{}", bytes);
    }

    use human_size::Kilobyte;
    let size = SpecificSize::new(bytes as f64, Byte).unwrap();
    
    if bytes < 1024 {
        format!("{}", size)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}", size.into::<Kilobyte>())
    } else if bytes < 1024 * 1024 * 1024 {
        use human_size::Megabyte;
        format!("{:.1}", size.into::<Megabyte>())
    } else {
        use human_size::Gigabyte;
        format!("{:.1}", size.into::<Gigabyte>())
    }
}

fn get_color(ratio: f64, size: u64) -> &'static str {
    if size < COLOR_THRESHOLD { return ""; }
    if ratio < 50.0 { GREEN } else if ratio < 80.0 { ORANGE } else { RED }
}

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
                if p.is_dir() && !p.file_name().and_then(|n| n.to_str()).map(|n| n.starts_with('.') && n != ".git").unwrap_or(false) {
                    repos.extend(find_git_repos(&p));
                }
            }
        }
    }
    repos
}

fn get_all_sizes(repo_path: &PathBuf) -> HashMap<String, (u64, u64)> {
    let mut all_sizes = HashMap::new();
    let child = Command::new("git")
        .args(["cat-file", "--batch-check=%(objectname) %(objectsize) %(objectsize:disk)", "--batch-all-objects"])
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

fn analyze_repo(
    repo_path: &PathBuf,
    pb: Option<ProgressBar>,
    current_only: bool,
) -> Result<HashMap<String, FileStats>, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(pb) = &pb {
        pb.set_style(ProgressStyle::default_spinner().template("{spinner:.green} {msg}").unwrap());
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
                String::from_utf8_lossy(&output.stdout).lines().map(|s| s.to_string()).collect()
            } else {
                Vec::new()
            };

            if let Some(pb) = &pb {
                pb.set_length(shas.len() as u64);
                pb.set_style(ProgressStyle::default_bar().template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}").unwrap());
                pb.set_message("Scanning history...");
            }

            // Process chunks in parallel
            let chunk_size = (shas.len() / (rayon::current_num_threads() * 4)).max(100);
            shas.par_chunks(chunk_size).map(|chunk| {
                let mut chunk_blobs: HashMap<String, HashSet<String>> = HashMap::new();
                let child = Command::new("git")
                    .args(["diff-tree", "-r", "--raw", "--no-commit-id", "--no-renames", "--root", "--stdin"])
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
                            let _ = writeln!(stdin, "{}", sha);
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
                                    chunk_blobs.entry(path.to_string()).or_default().insert(new_oid.to_string());
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
            }).reduce(HashMap::new, |mut a, b| {
                for (path, set2) in b {
                    a.entry(path).or_default().extend(set2);
                }
                a
            })
        }
    );

    // Phase 4: Merge results
    let mut final_stats = HashMap::new();

    if current_only {
        // Just the current files
        for (path, latest_size) in current_latest {
            let stats = final_stats.entry(path).or_insert(FileStats::default());
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
            let mut stats = FileStats::default();
            stats.versions = oids.len() as u64;
            for oid in oids {
                let (u, c) = all_sizes.get(&oid).copied().unwrap_or((0, 0));
                stats.total_uncompressed += u;
                stats.total_compressed += c;
            }
            stats.latest_size = *current_latest.get(&path).unwrap_or(&0);
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

    if let Some(pb) = pb { pb.finish_and_clear(); }

    Ok(final_stats)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let repos = if args.recursive { find_git_repos(&args.repo_path) } else { vec![args.repo_path.clone()] };
    
    if repos.is_empty() {
        eprintln!("No git repositories found in {:?}", args.repo_path);
        return Ok(());
    }

    let mp = MultiProgress::new();
    
    let mut all_stats = HashMap::new();
    
    // Process repositories sequentially (overkill to parallelize at this level)
    for repo_path in &repos {
        let pb = if !args.no_progress {
            Some(mp.add(ProgressBar::new(100)))
        } else {
            None
        };
        
        if let Ok(stats) = analyze_repo(repo_path, pb, args.current_only) {
            for (path, file_stat) in stats {
                let full_path = if repos.len() > 1 {
                    format!("{}/{}", repo_path.display(), path)
                } else {
                    path
                };
                let entry = all_stats.entry(full_path).or_insert(FileStats::default());
                entry.versions += file_stat.versions;
                entry.total_uncompressed += file_stat.total_uncompressed;
                entry.total_compressed += file_stat.total_compressed;
                entry.latest_size = file_stat.latest_size;
            }
        }
    }

    let mut sorted_files: Vec<_> = all_stats.iter().collect();
    
    // Parallel sort
    sorted_files.par_sort_by(|a, b| {
        let (_, sa) = a; let (_, sb) = b;
        let cmp = match args.sort_by {
            SortBy::Path => a.0.cmp(b.0),
            SortBy::Size => sa.latest_size.cmp(&sb.latest_size),
            SortBy::Versions => sa.versions.cmp(&sb.versions),
            SortBy::Uncompressed => sa.total_uncompressed.cmp(&sb.total_uncompressed),
            SortBy::Compressed => sa.total_compressed.cmp(&sb.total_compressed),
            SortBy::Ratio => {
                let ra = if sa.total_uncompressed > 0 { sa.total_compressed as f64 / sa.total_uncompressed as f64 } else { 0.0 };
                let rb = if sb.total_uncompressed > 0 { sb.total_compressed as f64 / sb.total_uncompressed as f64 } else { 0.0 };
                ra.partial_cmp(&rb).unwrap_or(std::cmp::Ordering::Equal)
            }
        };
        if args.descending { cmp.reverse() } else { cmp }
    });

    let max_path_len = sorted_files.iter().map(|(p, _)| p.len()).max().unwrap_or(40).min(60);
    let path_width = max_path_len.max(20);

    println!("{bold}{:<path_width$} {:>14} {:>10} {:>16} {:>16} {:>12}{reset}",
        "File", "Size", "Versions", "Total Uncomp.", "Total Comp.", "Ratio",
        bold = BOLD, reset = RESET, path_width = path_width);
    println!("{}", "-".repeat(path_width + 84));

    for (path, stats) in &sorted_files {
        let size = format_size(stats.latest_size, args.human_readable);
        let tu = format_size(stats.total_uncompressed, args.human_readable);
        let tc = format_size(stats.total_compressed, args.human_readable);
        let ratio = if stats.total_uncompressed > 0 { (stats.total_compressed as f64 / stats.total_uncompressed as f64) * 100.0 } else { 0.0 };
        let color = get_color(ratio, stats.latest_size);
        let dp = if path.len() > path_width { format!("...{}", &path[path.len() - (path_width - 3)..]) } else { path.to_string() };
        let rs = format!("{color}{:>10.1}%{reset}", ratio, color = color, reset = if color.is_empty() { "" } else { RESET });
        println!("{:<path_width$} {:>14} {:>10} {:>16} {:>16} {}", dp, size, stats.versions, tu, tc, rs, path_width = path_width);
    }

    println!("{}", "-".repeat(path_width + 84));
    let tf: u64 = sorted_files.iter().map(|(_, s)| s.latest_size).sum();
    let tv: u64 = sorted_files.iter().map(|(_, s)| s.versions).sum();
    let tu: u64 = sorted_files.iter().map(|(_, s)| s.total_uncompressed).sum();
    let tc: u64 = sorted_files.iter().map(|(_, s)| s.total_compressed).sum();
    let g = if tu > 0 { (tc as f64 / tu as f64) * 100.0 } else { 0.0 };

    println!("{bold}{:<path_width$} {:>14} {:>10} {:>16} {:>16} {:>11.1}%{reset}",
        "TOTAL", format_size(tf, args.human_readable), tv, format_size(tu, args.human_readable), format_size(tc, args.human_readable), g,
        bold = BOLD, reset = RESET, path_width = path_width);

    Ok(())
}
