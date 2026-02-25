# Git Compression Stats

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

## 🛠️ Installation

Ensure you have Rust and Cargo installed, then clone the repository and build:

```bash
git clone https://github.com/kassoulet/git-compression-stats.git
cd git-compression-stats
cargo build --release
```

The binary will be available at `./target/release/git-compression-stats`.

## 📖 Usage

```bash
./target/release/git-compression-stats [OPTIONS] [REPO_PATH]
```

### Options

- `-r, --recursive`: Scan the directory for multiple Git repositories.
- `-c, --current-only`: Rapid analysis of files currently in the working tree (ignores history).
- `-H, --human-readable`: Use human-readable sizes (KB, MB, GB).
- `-s, --sort-by <path|size|versions|uncompressed|compressed|ratio>`: Set the primary sort column (default: path).
- `-d, --descending`: Reverse the sort order.
- `--no-progress`: Disable the interactive progress bar.

## 📊 Example Output

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

## 🏗️ Technical Architecture

- **Rust**: For high-performance, memory-safe execution.
- **Rayon**: To parallelize data-intensive tasks.
- **Clap**: For robust command-line argument parsing.
- **Indicatif**: To provide progress feedback in the terminal.
- **Human-Size**: For readable byte formatting.
- **Git Binary**: Directly interfaces with local `git` for maximum speed and accuracy.

## ⚖️ License

[MIT](LICENSE) (or your preferred license)
