## 2025-05-14 - [Allocation & Redundancy Optimization]
**Learning:** Significant performance wins in Git-backed Rust CLI tools can be achieved by:
1. Using reusable `String` buffers with `read_line` instead of `reader.lines()` to avoid millions of allocations.
2. Avoiding `.split().collect::<Vec<_>>()` by using iterators directly.
3. Consolidating redundant Git process calls; the `current_only` path was previously spawning a second `git ls-tree` unnecessarily.
4. Optimizing `HashMap` merging in parallel `reduce` steps by always merging the smaller map into the larger one.
**Action:** Always profile the parsing of large process output and check for redundant command executions in fast-path logic.
