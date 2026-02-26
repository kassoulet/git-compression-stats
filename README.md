# Git Compression Stats

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Crates.io](https://img.shields.io/crates/v/git-compression-stats.svg)](https://crates.io/crates/git-compression-stats)
[![docs.rs](https://docs.rs/git-compression-stats/badge.svg)](https://docs.rs/git-compression-stats)
[![Rust](https://img.shields.io/badge/rust-1.85%2B-blue.svg)](https://github.com/rust-lang/rust)
[![Build Status](https://img.shields.io/github/actions/workflow/status/kassoulet/git-compression-stats/ci.yml?branch=main)](https://github.com/kassoulet/git-compression-stats/actions)

A high-performance Rust CLI tool to analyze how efficiently files are compressed within a Git repository's history. It compares the total uncompressed size of all versions of a file against their actual "on-disk" size in Git's object database (pack files).

## 🚀 Key Features

- **Deep History Analysis**: Tracks every unique version (blob) of every file across the entire commit history.
- **Accurate Storage Metrics**: Retrieves actual disk footprint using `git cat-file` with `%(objectsize:disk)`, accounting for Git's internal delta compression and zlib storage.
- **Parallel Execution**:
    - **Concurrent Phases**: Object sizing and history traversal run simultaneously using `rayon::join`.
    - **Intra-Repo Parallelism**: History is chunked and processed in parallel using Rayon and multiple `git diff-tree` workers.
- **Optimized for Large Repos**: Uses a diff-based approach to history traversal, processing only changed files. Capable of analyzing repositories with millions of commits (like the Linux kernel) in under a minute.
- **Visual Insights**:
    - **Progress Feedback**: Real-time progress bars for history scanning.
    - **Color-Coded Ratios**: Visual indicators for compression efficiency (🟢 < 50%, 🟠 50-80%, 🔴 > 80%).
- **Flexible Reporting**: Sort by path, size, versions, uncompressed size, compressed size, or ratio.
- **Multiple Output Formats**: Table (default), JSON, and CSV for programmatic use.
- **Smart Filtering**: Filter results by minimum file size and compression ratio thresholds.

## 📦 Installation

### From Source

```bash
git clone https://github.com/kassoulet/git-compression-stats.git
cd git-compression-stats
cargo install --path .
```

### From crates.io (when published)

```bash
cargo install git-compression-stats
```

### Build Manually

```bash
git clone https://github.com/kassoulet/git-compression-stats.git
cd git-compression-stats
cargo build --release
```

The binary will be available at `./target/release/git-compression-stats`.

## 📖 Usage

```bash
git-compression-stats [OPTIONS] [REPO_PATH]
```

### Options

| Option | Description |
|--------|-------------|
| `-r, --recursive` | Scan the directory for multiple Git repositories |
| `-c, --current-only` | Rapid analysis of files currently in the working tree (ignores history) |
| `-H, --human-readable` | Use human-readable sizes (KB, MB, GB) |
| `-s, --sort-by <SORT>` | Set the primary sort column: `path`, `size`, `versions`, `uncompressed`, `compressed`, `ratio` (default: `path`) |
| `-d, --descending` | Reverse the sort order |
| `-f, --format <FORMAT>` | Output format: `table` (default), `json`, `csv` |
| `--min-size <SIZE>` | Minimum file size to display (e.g., `1024`, `1K`, `1M`, `1G`) |
| `--min-ratio <RATIO>` | Minimum compression ratio to display (0-100%) |
| `--no-progress` | Disable the interactive progress bar |
| `-h, --help` | Print help information |
| `-V, --version` | Print version information |

### Examples

**Analyze current repository:**
```bash
git-compression-stats
```

**Analyze a specific repository:**
```bash
git-compression-stats /path/to/repo
```

**Scan recursively for multiple repositories:**
```bash
git-compression-stats -r ~/projects
```

**Quick analysis of current files only:**
```bash
git-compression-stats -c
```

**Sort by compression ratio in descending order:**
```bash
git-compression-stats -s ratio -d
```

**Human-readable output:**
```bash
git-compression-stats -H
```

**JSON output for programmatic use:**
```bash
git-compression-stats --format json
```

**CSV output for spreadsheet import:**
```bash
git-compression-stats --format csv
```

**Filter by minimum file size (1KB):**
```bash
git-compression-stats --min-size 1K
```

**Filter by minimum compression ratio (50%):**
```bash
git-compression-stats --min-ratio 50
```

**Combined filters with JSON output:**
```bash
git-compression-stats --format json --min-size 1K --min-ratio 30
```

**Pipe JSON to jq for further processing:**
```bash
git-compression-stats --format json --min-size 5K | jq '.files[] | select(.ratio > 40)'
```

## 📊 Example Output

### Table Format (Default)

```text
File                           Size   Versions    Total Uncomp.      Total Comp.        Ratio
--------------------------------------------------------------------------------------------------------
.gitignore                      8 B          1              8 B             17 B      212.5%
Cargo.lock                  56324 B          2         109643 B          14053 B       12.8%
Cargo.toml                    288 B          2            542 B            210 B       38.7%
src/main.rs                 15379 B          4          32023 B           5620 B       17.5%
--------------------------------------------------------------------------------------------------------
TOTAL                       71999 B          9         142216 B          19900 B        14.0%
```

### JSON Format

```json
{
  "files": [
    {
      "path": "src/main.rs",
      "size": 15379,
      "versions": 4,
      "total_uncompressed": 32023,
      "total_compressed": 5620,
      "ratio": 17.55
    }
  ],
  "summary": {
    "total_files": 4,
    "total_size": 71999,
    "total_versions": 9,
    "total_uncompressed": 142216,
    "total_compressed": 19900,
    "global_ratio": 14.0
  }
}
```

### CSV Format

```csv
path,size,versions,total_uncompressed,total_compressed,ratio
.gitignore,8,1,8,17,212.50
Cargo.lock,56324,2,109643,14053,12.82
Cargo.toml,288,2,542,210,38.75
src/main.rs,15379,4,32023,5620,17.55
```

### Color Coding

The ratio column is color-coded based on compression efficiency:

- 🟢 **Green** (< 50%): Excellent compression
- 🟠 **Orange** (50-80%): Moderate compression
- 🔴 **Red** (> 80%): Poor compression

## 🏗️ Technical Architecture

### Dependencies

| Crate | Purpose |
|-------|---------|
| `rayon` | Parallelism engine for concurrent phases and parallel history processing |
| `clap` | Command-line argument parsing and validation |
| `human-size` | Formats raw byte counts into human-readable units |
| `indicatif` | Progress bars and terminal styling |
| `serde` + `serde_json` | JSON serialization for programmatic output |

### Data Flow

1. **Object Inventory**: Runs `git cat-file --batch-check --batch-all-objects` to map every Object ID to its compressed and uncompressed size.
2. **History Scanning**: Fetches all commit SHAs via `git rev-list --all` and distributes them across parallel workers using `git diff-tree --stdin`.
3. **Latest State Verification**: Performs `git ls-tree -r HEAD` to determine current file sizes.
4. **Aggregation and Sorting**: Merges results and performs parallel sort based on user-specified criteria.

### Performance Targets

- **Large Scale**: Capable of analyzing repositories with millions of commits (e.g., Linux Kernel) in under a minute on modern hardware.
- **Efficiency**: CPU utilization scales linearly with history depth and number of cores.

## 🤝 Contributing

Contributions are welcome! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## ⚖️ License

This project is licensed under the [MIT License](LICENSE).

## 🙏 Acknowledgments

- The Git project for the amazing version control system
- The Rust community for excellent libraries and tools
