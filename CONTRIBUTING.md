# Contributing to git-compression-stats

Thank you for considering contributing to `git-compression-stats`! We welcome contributions from everyone.

## How Can I Contribute?

### Reporting Bugs

Before creating bug reports, please check existing issues as you might find out that you don't need to create one. When you are creating a bug report, please include as many details as possible:

* **Use a clear and descriptive title**
* **Describe the exact steps to reproduce the problem**
* **Provide specific examples to demonstrate the steps**
* **Describe the behavior you observed and what behavior you expected**
* **Include any relevant error messages or logs**
* **Include your environment details** (OS, Rust version, git version)

### Suggesting Enhancements

Enhancement suggestions are tracked as GitHub issues. When creating an enhancement suggestion, please include:

* **Use a clear and descriptive title**
* **Provide a detailed description of the suggested enhancement**
* **Explain why this enhancement would be useful**
* **List some examples of how this enhancement would be used**

### Pull Requests

* Fill in the required template
* Follow the Rust style guide
* Include appropriate tests if applicable
* Update documentation as needed
* Ensure all tests pass and there are no clippy warnings

## Development Setup

### Prerequisites

* Rust 1.85 or later
* Git

### Building from Source

```bash
git clone https://github.com/kassoulet/git-compression-stats.git
cd git-compression-stats
cargo build --release
```

### Pre-commit Hook

We provide a pre-commit hook that runs the same checks as CI (formatting, build, tests, and clippy):

```bash
# Install the pre-commit hook
ln -sf ../../scripts/pre-commit.sh .git/hooks/pre-commit
```

Once installed, the hook will automatically run before each commit and prevent commits that fail CI checks.

To run the checks manually:

```bash
./scripts/pre-commit.sh
```

### Running Tests

```bash
cargo test
```

### Code Style

We use `rustfmt` and `clippy` to maintain code quality:

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
```

## Code of Conduct

### Our Pledge

We pledge to make participation in our project a harassment-free experience for everyone.

### Our Standards

Examples of behavior that contributes to creating a positive environment include:

* Using welcoming and inclusive language
* Being respectful of differing viewpoints and experiences
* Gracefully accepting constructive criticism
* Focusing on what is best for the community
* Showing empathy towards other community members

### Enforcement

Instances of abusive, harassing, or otherwise unacceptable behavior may be reported by contacting the project maintainer.

## Questions?

Feel free to open an issue for any questions or concerns.
