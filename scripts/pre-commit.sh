#!/bin/bash
# Pre-commit hook that runs the same checks as CI
# Source: .github/workflows/ci.yml

set -e

echo "Running pre-commit checks (same as CI)..."
echo "=========================================="

# Check formatting
echo "→ Checking formatting..."
if ! cargo fmt --all -- --check; then
    echo "❌ Formatting check failed. Run 'cargo fmt' to fix."
    exit 1
fi
echo "✓ Formatting OK"

# Build
echo "→ Building..."
if ! cargo build --verbose; then
    echo "❌ Build failed."
    exit 1
fi
echo "✓ Build OK"

# Tests
echo "→ Running tests..."
if ! cargo test --verbose; then
    echo "❌ Tests failed."
    exit 1
fi
echo "✓ Tests OK"

# Clippy
echo "→ Running Clippy..."
if ! cargo clippy --all-targets --all-features -- -D warnings; then
    echo "❌ Clippy found issues. Run 'cargo clippy --all-targets --all-features' to see details."
    exit 1
fi
echo "✓ Clippy OK"

echo "=========================================="
echo "✅ All pre-commit checks passed!"
