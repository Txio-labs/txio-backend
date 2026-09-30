# Contributing to txio-backend

First off, thank you for considering contributing! It's people like you that make txio's backend reliable across five chains.

## How Can I Contribute?

### Reporting Bugs

Before creating a bug report, please check the [existing issues](https://github.com/Txio-labs/txio-backend/issues). When you do open one, please include:

* A clear and descriptive title
* The exact steps that reproduce the problem, including the RPC method/params involved if relevant
* What you expected to happen vs. what actually happened
* The JSON-RPC error envelope returned, if any (see the error code registry in `walkthrough.md`)

### Suggesting Enhancements

Enhancement suggestions are tracked as GitHub issues. Please include:

* A clear and descriptive title
* A step-by-step description of the suggested enhancement
* Why it would be useful to most txio users

### Pull Requests

* Do not include issue numbers in the PR title.
* Before merging, automated checks must pass: build, tests, and lint.
* End all files with a newline.
* New inputs must go through `validator::Validate` before reaching the service layer — this project is fail-fast by convention.

## Development Setup

The Axum API lives in `api/`, not the repo root:

```bash
cd api
cp .env.example .env   # fill in MONGO_URI, JWT_SECRET, BREVO_API_KEY
cargo run
```

Run the standalone RPC test tool:

```bash
cargo run --bin sui_cli -- -m sui_getChainIdentifier --pretty
```

Run tests:

```bash
cargo test
```

## Styleguides

### Git Commit Messages

* Use the present tense ("Add feature" not "Added feature")
* Use the imperative mood ("Move cursor to..." not "Moves cursor to...")
* Limit the first line to 72 characters or less
* Reference issues and pull requests liberally after the first line

### Code Style

* Follow the repository pattern already in place: queries in `src/repositories/`, business logic in `src/services/` — don't reach into the database from a handler
* New RPC error conditions should be wrapped in the existing JSON-RPC 2.0 error envelope convention, not a raw HTTP error
* Run `cargo fmt` and `cargo clippy` before opening a PR
