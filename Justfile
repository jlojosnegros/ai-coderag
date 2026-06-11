# build recipes
# Install: sudo dnf install just   (or see https://just.systems/man/en/packages.html)
# Usage:   just --list

# Show available recipes
default:
    @just --list

# Debug build (fast, for development)
dev:
    cargo build

# Release build (native, dynamically linked)
build:
    cargo build --release

# Static release build for Linux x86-64 (requires cross and Docker)
build-static:
    RUSTC_WRAPPER="" cross build --release --target x86_64-unknown-linux-musl

# Run unit tests only
test-unit:
    cargo nextest run

# Run integration tests (wiremock in-process, no cluster needed)
test-integration:
    cargo nextest run

# Run all tests — unit + HTTP integration
test: test-unit test-integration

# Apply compiler-suggested fixes automatically
fix:
    cargo fix --allow-dirty --allow-staged

# Check formatting and clippy (nightly needed for unstable rustfmt options)
check:
    cargo +nightly fmt --check
    cargo clippy

format:
    cargo +nightly fmt

# Security audit (requires: cargo install cargo-audit --locked)
audit:
    cargo audit

# License and dependency check (requires: cargo install cargo-deny --locked)
deny:
    cargo deny check

# Build and verify documentation (fails on broken intra-doc links, bad backticks, etc.)
docs:
    cargo doc --no-deps --quiet

# Full CI gate: fmt + clippy + doc + audit + deny + clean build + tests
ci: check docs audit deny clean-dev dev test

# Run before every commit — same checks as CI but without the clean build step
pre-commit: check docs audit deny test

# Run a new release (requires cargo-release and git-cliff)
release version:
    cargo release --config .config/release.toml {{version}} --execute

# Run a dry-run for a new release (requires cargo-release and git-cliff)
check-release version:
    cargo release --config .config/release.toml {{version}} 

# Modules excluded from coverage reports — require real infrastructure (cluster,
# subprocess binaries) or are pure boilerplate with no logic to test.
#
_cov_exclude := ""

# Code coverage — full report in terminal (requires cargo-llvm-cov)
coverage:
    cargo llvm-cov --ignore-filename-regex "{{_cov_exclude}}"

# Code coverage — summary only (requires cargo-llvm-cov)
coverage-sum:
    cargo llvm-cov --summary-only --ignore-filename-regex "{{_cov_exclude}}"

# Code coverage HTML report — opens in browser (requires cargo-llvm-cov)
coverage-html:
    cargo llvm-cov --html --open --ignore-filename-regex "{{_cov_exclude}}"

# Code coverage check for CI — fails if coverage drops below threshold
coverage-ci threshold="80":
    cargo llvm-cov --ignore-filename-regex "{{_cov_exclude}}" \
        --fail-under-lines {{threshold}}

# Render all Mermaid .mmd diagrams in docs/diagrams/ to SVG
# Requires: npm install -g @mermaid-js/mermaid-cli
diagrams:
    #!/usr/bin/env bash
    set -euo pipefail
    command -v mmdc &>/dev/null || {
        echo "mmdc not found. Install with: npm install -g @mermaid-js/mermaid-cli"
        exit 1
    }
    shopt -s nullglob
    count=0
    for f in docs/diagrams/*.mmd; do
        mmdc -i "$f" -o "${f%.mmd}.svg" --theme neutral --backgroundColor transparent
        echo "  rendered: $f → ${f%.mmd}.svg"
        count=$(( count + 1 ))
    done
    echo "Done ($count diagrams rendered)."

# Verify README.md diagram references are consistent with docs/diagrams/ (mirrors pre-commit hook)
check-diagram-refs:
    #!/usr/bin/env bash
    set -euo pipefail
    fail=0

    readme_svgs=$(grep -oE 'docs/diagrams/[^)]+\.svg' README.md || true)
    disk_mmds=$(find docs/diagrams -name '*.mmd' | sed 's|\.mmd$|.svg|' | sort)

    # SVGs referenced in README but .mmd missing on disk
    while IFS= read -r svg; do
        [ -z "$svg" ] && continue
        mmd="${svg%.svg}.mmd"
        if [ ! -f "$mmd" ]; then
            echo "MISSING SOURCE: $svg is referenced in README.md but $mmd does not exist"
            fail=1
        fi
        if [ ! -f "$svg" ]; then
            echo "MISSING SVG: $svg is referenced in README.md but the file does not exist"
            echo "             Run: just diagrams"
            fail=1
        fi
    done <<< "$readme_svgs"

    # .mmd files on disk whose SVG is not referenced in README
    while IFS= read -r svg; do
        [ -z "$svg" ] && continue
        if ! grep -qF "$svg" README.md; then
            mmd="${svg%.svg}.mmd"
            echo "ORPHANED: $mmd exists but $svg is not referenced in README.md"
            echo "          Either add the reference or: git rm $mmd $svg"
            fail=1
        fi
    done <<< "$disk_mmds"

    [ $fail -eq 0 ] && echo "All diagram references are consistent." || exit 1

# Verify all Mermaid SVGs are up to date with their .mmd sources
# Fails if any SVG is missing or older than its .mmd source (for CI)
check-diagrams:
    #!/usr/bin/env bash
    set -euo pipefail
    fail=0
    shopt -s nullglob
    for mmd in docs/diagrams/*.mmd; do
        svg="${mmd%.mmd}.svg"
        if [ ! -f "$svg" ]; then
            echo "MISSING: $svg — run: just diagrams"
            fail=1
        elif [ "$mmd" -nt "$svg" ]; then
            echo "STALE:   $svg is older than $mmd — run: just diagrams"
            fail=1
        fi
    done
    [ $fail -eq 0 ] && echo "All diagrams are up to date." || exit 1

# Remove all build artifacts
clean:
    cargo clean

# Remove only debug build artifacts
clean-dev:
    cargo clean --profile dev

# Remove only release build artifacts
clean-release:
    cargo clean --release

# Remove only static build artifacts
clean-static:
    cargo clean --release --target x86_64-unknown-linux-musl
