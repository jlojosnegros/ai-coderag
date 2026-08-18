# coderag build recipes
# Install: sudo dnf install just   (or see https://just.systems/man/en/packages.html)
# Usage:   just --list

# Use sccache as rustc wrapper if available; empty string disables it gracefully
export RUSTC_WRAPPER := `which sccache 2>/dev/null || echo ""`

# Modules excluded from coverage reports:
#   - main.rs: CLI entry point, pure clap dispatch + tracing init. All logic
#     lives in library functions. 0% coverage is expected.
#   - mcp/mod.rs: MCP server glue (tool handlers). Requires a running MCP
#     server to exercise. Tool logic is thin (calls store/embedder/ranking).
#     Ranking logic is tested in mcp/ranking.rs (98% coverage).
_cov_exclude := "main\\.rs|mcp/mod"

# Show available recipes
default:
    @just --list

# ─── Formatting ────────────────────────────────────────────────────────────────

# Check formatting without applying changes (nightly for unstable rustfmt options)
fmt-check:
    cargo +nightly fmt --check

# Apply code formatting
format:
    cargo +nightly fmt

# ─── Linting ───────────────────────────────────────────────────────────────────

# Run clippy linter
clippy:
    cargo clippy --all-targets

# Check format + run clippy
check: fmt-check clippy

# ─── Auto-fix ──────────────────────────────────────────────────────────────────

# Apply compiler-suggested fixes automatically
fix:
    cargo fix --allow-dirty --allow-staged

# Apply clippy-suggested fixes automatically
fix-clippy:
    cargo clippy --all-targets --fix --allow-dirty

# ─── Documentation ─────────────────────────────────────────────────────────────

# Build docs and fail on broken intra-doc links, bad backticks, etc.
docs:
    cargo doc --no-deps --quiet

# ─── Security & supply-chain ───────────────────────────────────────────────────

# Security audit of dependencies (requires: cargo install cargo-audit --locked)
audit:
    cargo audit

# License and dependency policy check (requires: cargo install cargo-deny --locked)
deny:
    cargo deny check

# ─── Build ─────────────────────────────────────────────────────────────────────

# Debug build (fast, for development)
dev:
    cargo build

# Release build (native, dynamically linked)
build:
    cargo build --release

# Static release build for Linux x86-64.
# Uses cargo directly if x86_64-linux-musl-gcc is available (CI: apt-get install musl-tools).
# Falls back to cross (Docker required) for local development on non-musl hosts.
build-static:
    #!/usr/bin/env bash
    set -euo pipefail
    if command -v x86_64-linux-musl-gcc &>/dev/null; then
        cargo build --release --target x86_64-unknown-linux-musl
    elif command -v cross &>/dev/null; then
        RUSTC_WRAPPER="" cross build --release --target x86_64-unknown-linux-musl
    else
        echo "ERROR: neither x86_64-linux-musl-gcc nor cross found."
        echo "  CI:    apt-get install musl-tools"
        echo "  Local: cargo install cross  (requires Docker)"
        exit 1
    fi

# ─── Tests ─────────────────────────────────────────────────────────────────────

# Run unit tests only (lib crate, no integration tests)
test-unit:
    cargo nextest run --lib

# Run integration tests (real LanceDB + embeddings, optionally rust-analyzer)
test-integration:
    cargo nextest run --test integration_test

# Run all tests
test: test-unit test-integration

# ─── Commit validation ────────────────────────────────────────────────────────

# Validate a single commit message file against Conventional Commits.
_check-commit-msg file:
    #!/usr/bin/env bash
    MSG=$(cat "{{file}}")
    PATTERN="^(test|feat|fix|docs|refactor|ci|style|chore)(\(.+\))?: .+"
    if ! echo "$MSG" | grep -qE "$PATTERN"; then
        echo ""
        echo "ERROR: commit message does not follow Conventional Commits."
        echo "  Got:      $MSG"
        echo "  Expected: type(scope): description"
        echo "  Types:    test feat fix docs refactor ci style chore"
        echo ""
        exit 1
    fi

# Validate all non-merge commits in the current branch vs base branch.
validate-commits base="main":
    #!/usr/bin/env bash
    set -euo pipefail
    git fetch origin "{{base}}" --quiet
    FAIL=0
    TMPFILE=$(mktemp)
    trap 'rm -f "$TMPFILE"' EXIT
    while IFS= read -r msg; do
        echo "$msg" > "$TMPFILE"
        just _check-commit-msg "$TMPFILE" || FAIL=1
    done < <(git log "origin/{{base}}..HEAD" --no-merges --pretty='%s')
    [ $FAIL -eq 0 ] && echo "All commit messages follow Conventional Commits." || exit 1

# ─── Version check ─────────────────────────────────────────────────────────────

# Verify that the git tag matches the version in Cargo.toml.
_check-version tag:
    #!/usr/bin/env bash
    set -euo pipefail
    TAG="{{tag}}"
    if [[ ! "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        echo "ERROR: tag '$TAG' does not match expected format vX.Y.Z"
        exit 1
    fi
    TAG_VERSION="${TAG#v}"
    CARGO_VERSION=$(grep '^version' Cargo.toml | head -1 | cut -d'"' -f2)
    if [ "$TAG_VERSION" != "$CARGO_VERSION" ]; then
        echo "ERROR: tag '$TAG' does not match Cargo.toml version '$CARGO_VERSION'"
        echo "Update Cargo.toml version to $TAG_VERSION before tagging."
        exit 1
    fi
    echo "OK: tag and Cargo.toml both at $CARGO_VERSION"

# ─── Coverage ──────────────────────────────────────────────────────────────────

# Code coverage — full report in terminal (requires cargo-llvm-cov)
coverage:
    cargo llvm-cov --ignore-filename-regex "{{_cov_exclude}}"

# Code coverage — summary only (requires cargo-llvm-cov)
coverage-sum:
    cargo llvm-cov --summary-only --ignore-filename-regex "{{_cov_exclude}}"

# Code coverage HTML report — opens in browser (requires cargo-llvm-cov)
coverage-html:
    cargo llvm-cov --html --open --ignore-filename-regex "{{_cov_exclude}}"

# Show uncovered lines in new/modified code only (diff vs base branch).
# Crosses git diff line ranges with LCOV coverage data — only reports lines that
# are both (a) added/modified in this branch and (b) executed 0 times by tests.
# Pre-existing uncovered lines are filtered out.
# Requires: cargo-llvm-cov
coverage-uncovered base="main":
    #!/usr/bin/env bash
    set -euo pipefail

    if ! cargo llvm-cov --version &>/dev/null; then
        echo "SKIP: cargo-llvm-cov not installed"
        exit 0
    fi

    LCOV_FILE=$(mktemp /tmp/coderag-lcov.XXXXXX)
    trap 'rm -f "$LCOV_FILE"' EXIT

    git fetch origin "{{base}}" --quiet 2>/dev/null || true
    MERGE_BASE=$(git merge-base HEAD "origin/{{base}}" 2>/dev/null || echo "origin/{{base}}")

    echo "==> Running tests and collecting coverage..."
    cargo llvm-cov --lcov --output-path "$LCOV_FILE" --ignore-filename-regex "{{_cov_exclude}}" 2>/dev/null

    echo "==> Comparing against ${MERGE_BASE:0:12}..."
    echo ""

    REPO_ROOT=$(git rev-parse --show-toplevel)

    DIFF_RANGES=$(git diff --unified=0 "$MERGE_BASE" -- '*.rs' | awk -v root="$REPO_ROOT" '
        /^diff --git/ {
            split($0, parts, " b/")
            file = root "/" parts[2]
        }
        /^@@/ {
            match($0, /\+([0-9]+)(,([0-9]+))?/, m)
            start = m[1]
            count = (m[3] == "" ? 1 : m[3])
            if (count > 0) print file ":" start ":" count
        }
    ')

    if [ -z "$DIFF_RANGES" ]; then
        echo "No .rs files changed in this branch."
        exit 0
    fi

    UNCOVERED=$(awk '
        /^SF:/ { file = substr($0, 4) }
        /^DA:/ {
            split(substr($0, 4), parts, ",")
            if (parts[2] + 0 == 0) print file ":" parts[1]
        }
    ' "$LCOV_FILE")

    FOUND=0
    TOTAL_UNCOVERED=0
    CURRENT_FILE=""

    while IFS=: read -r file start count; do
        end=$((start + count - 1))
        for line in $(seq "$start" "$end"); do
            if echo "$UNCOVERED" | grep -qxF "${file}:${line}"; then
                if [ "$file" != "$CURRENT_FILE" ]; then
                    [ "$CURRENT_FILE" != "" ] && echo ""
                    CURRENT_FILE="$file"
                    REL_PATH="${file#$REPO_ROOT/}"
                    echo "── ${REL_PATH} ──"
                fi
                SRC_LINE=$(sed -n "${line}p" "$file" 2>/dev/null | sed 's/^[[:space:]]*/  /')
                printf "  L%-5d %s\n" "$line" "$SRC_LINE"
                TOTAL_UNCOVERED=$((TOTAL_UNCOVERED + 1))
                FOUND=1
            fi
        done
    done <<< "$DIFF_RANGES"

    echo ""
    if [ $FOUND -eq 0 ]; then
        echo "All new/modified lines are covered."
    else
        echo "── Summary: ${TOTAL_UNCOVERED} new lines without coverage ──"
    fi

# JSON output for machine consumption — used internally by coverage-diff
_coverage-json:
    cargo llvm-cov --json --ignore-filename-regex "{{_cov_exclude}}"

# LCOV format (for CI diff annotations)
_coverage-lcov output="lcov.info":
    cargo llvm-cov --lcov --output-path {{output}} --ignore-filename-regex "{{_cov_exclude}}"

# Cobertura XML (for CI diff annotations)
_coverage-cobertura output="cobertura.xml":
    cargo llvm-cov --cobertura --output-path {{output}} --ignore-filename-regex "{{_cov_exclude}}"

# JSON + Cobertura in one test run — used by CI coverage job.
# Tests run once; --no-run reuses the profraw data for the second format.
_coverage-report json="coverage.json" cobertura="cobertura.xml":
    cargo llvm-cov --json --output-path {{json}} --ignore-filename-regex "{{_cov_exclude}}"
    cargo llvm-cov --no-run --cobertura --output-path {{cobertura}} --ignore-filename-regex "{{_cov_exclude}}"

# CI coverage gate — generates JSON + Cobertura AND fails if line coverage is below threshold.
_coverage-check threshold="80" json="coverage.json" cobertura="cobertura.xml":
    #!/usr/bin/env bash
    set -euo pipefail
    just _coverage-report "{{json}}" "{{cobertura}}"
    PCT=$(jq '.data[0].totals.lines.percent' "{{json}}" 2>/dev/null | awk '{printf "%.2f", $1}')
    if [ -z "$PCT" ] || [ "$PCT" = "00" ]; then
        echo "ERROR: could not parse coverage percentage from '{{json}}'" >&2
        echo "       Ensure cargo-llvm-cov and jq are installed and tests pass." >&2
        exit 1
    fi
    THRESHOLD_FMT=$(printf "%.2f" "{{threshold}}")
    echo "Coverage: ${PCT}%"
    if awk -v p="$PCT" -v t="{{threshold}}" 'BEGIN { exit !(p + 0 < t + 0) }'; then
        echo ""
        echo "================================================================"
        echo "=                                                              ="
        echo "=  WARNING: CODE COVERAGE IS BELOW THE ${THRESHOLD_FMT}% THRESHOLD  ="
        echo "=                                                              ="
        echo "=  Current  : ${PCT}%"
        echo "=  Required : ${THRESHOLD_FMT}%"
        echo "=                                                              ="
        echo "=  New code is not adequately covered by tests.               ="
        echo "=  Please add tests before this is merged.                    ="
        echo "=                                                              ="
        echo "================================================================"
        echo ""
        exit 1
    fi
    echo "Coverage OK: ${PCT}% (above ${THRESHOLD_FMT}% threshold)"

# Compare line coverage of the current branch against a base branch — fails if coverage drops.
# Requires: cargo-llvm-cov, jq
coverage-diff base="main" tolerance="0.5":
    #!/usr/bin/env bash
    set -euo pipefail

    WORKTREE_DIR=$(mktemp -d /tmp/coderag-cov-base.XXXXXX)
    trap 'git worktree remove --force "$WORKTREE_DIR" 2>/dev/null || true' EXIT

    echo "==> Fetching latest {{base}} from origin..."
    git fetch origin "{{base}}" --quiet

    MERGE_BASE=$(git merge-base HEAD "origin/{{base}}")
    echo "==> Merge-base: ${MERGE_BASE:0:12}"
    echo "==> Setting up worktree for merge-base..."
    git worktree add --quiet "$WORKTREE_DIR" "$MERGE_BASE"

    echo "==> Measuring coverage on {{base}} (merge-base)..."
    BASE_PCT=$(bash -c "cd '$WORKTREE_DIR' && just _coverage-json 2>/dev/null" \
        | jq -r '.data[0].totals.lines.percent // empty')
    if [ -z "$BASE_PCT" ] || [ "$BASE_PCT" = "null" ]; then
        echo "ERROR: failed to measure coverage at merge-base ${MERGE_BASE:0:12}" >&2
        echo "       Ensure the base commit builds successfully and all tests pass." >&2
        exit 1
    fi

    echo "==> Measuring coverage on current branch..."
    CURR_PCT=$(just _coverage-json 2>/dev/null \
        | jq -r '.data[0].totals.lines.percent // empty')
    if [ -z "$CURR_PCT" ] || [ "$CURR_PCT" = "null" ]; then
        echo "ERROR: failed to measure coverage on current branch" >&2
        exit 1
    fi

    echo ""
    echo "Coverage at merge-base (${MERGE_BASE:0:12}): ${BASE_PCT}%"
    echo "Coverage on current branch:                ${CURR_PCT}%"
    echo "Tolerance:                                 {{tolerance}}%"
    echo ""

    awk -v base="$BASE_PCT" -v curr="$CURR_PCT" -v tol="{{tolerance}}" 'BEGIN {
        diff = curr - base
        sign = (diff >= 0) ? "+" : ""
        printf "Delta: %s%.2f%%\n", sign, diff
        if (curr < base - tol) {
            printf "FAIL: coverage dropped by %.2f%% beyond %.2f%% tolerance (%.2f%% < %.2f%%)\n", -diff, tol, curr, base
            exit 1
        }
        if (curr < base) {
            printf "OK: coverage decreased by %.2f%% but within %.2f%% tolerance\n", -diff, tol
        } else {
            printf "OK: coverage did not decrease (%.2f%% >= %.2f%%)\n", curr, base
        }
    }'

# ─── Optional-tool skip wrappers (private) ─────────────────────────────────────

# Skip gracefully if cargo-llvm-cov or jq is not installed.
_coverage-diff-if-available:
    #!/usr/bin/env bash
    if ! cargo llvm-cov --version &>/dev/null; then
        echo "SKIP: cargo-llvm-cov not installed — skipping coverage-diff"
        echo "      Install: cargo install cargo-llvm-cov --locked"
        exit 0
    fi
    if ! command -v jq &>/dev/null; then
        echo "SKIP: jq not in PATH — skipping coverage-diff"
        echo "      Install: sudo dnf install jq | brew install jq | apt-get install jq"
        exit 0
    fi
    just coverage-diff

# ─── CI gates ──────────────────────────────────────────────────────────────────

# Mirrors the CI job: lint + docs + deny + tests.
# Does NOT include: clean rebuild (CI starts fresh), coverage (separate job).
ci: check docs deny test

# Pre-push gate: run before opening a PR for >90% confidence CI will pass.
# Coverage regression check included (skipped gracefully if tools missing).
pre-commit: check docs deny test _coverage-diff-if-available

# ─── Release ───────────────────────────────────────────────────────────────────

# Run a new release (requires cargo-release and git-cliff)
_release version:
    cargo release --config .config/release.toml {{version}} --execute

# Dry-run a release (requires cargo-release and git-cliff).
# The real release always generates the changelog via its pre-release-hook.
# In dry-run mode the hook does NOT run, so pass --create-changelog to generate it:
#   just _check-release 0.4.0 --create-changelog
_check-release version *flags:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo release --config .config/release.toml {{version}}
    if [[ " {{flags}} " == *" --create-changelog "* ]]; then
        git-cliff --config .config/cliff.toml --tag "v{{version}}" --output CHANGELOG.md
        echo "CHANGELOG.md regenerated for v{{version}}"
    fi

# ─── Diagrams ──────────────────────────────────────────────────────────────────

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

# Verify README.md diagram references are consistent with docs/diagrams/
check-diagram-refs:
    #!/usr/bin/env bash
    set -euo pipefail
    fail=0

    readme_svgs=$(grep -oE 'docs/diagrams/[^)]+\.svg' README.md || true)
    disk_mmds=$(find docs/diagrams -name '*.mmd' | sed 's|\.mmd$|.svg|' | sort)

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

# Verify all Mermaid SVGs are up to date with their .mmd sources — used by CI
_check-diagrams:
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

# ─── Clean ─────────────────────────────────────────────────────────────────────

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
