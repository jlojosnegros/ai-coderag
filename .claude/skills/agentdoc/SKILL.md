# agentdoc — Agentic Documentation Generator

Generate and maintain AI-agent-oriented documentation that combines a lightweight
root-level `CLAUDE.md` with a deep `docs/agent-overlay.md`, both equipped with
drift detection, freshness scoring, and protection for human-written sections.

Install: copy this file to `.claude/skills/agentdoc/SKILL.md` in the target repo.
Invoke: `/agentdoc [init [--mode single|multi]|migrate|draft|maintain|status] ["context hint"]`

`--mode single` (default): one overlay covers the whole repo.
`--mode multi`: one overlay per subsystem plus a hub overlay. The AI proposes subsystems;
the human provides the final list. Enables STATUS.md generation.

---

## Phase detection (auto-mode)

When invoked without a phase argument, detect phase from repo state:

1. Run `find docs -name "agent-overlay.md" 2>/dev/null`.
2. If not found → **init** (single mode).
3. If found, check whether an `agentdoc:` block exists inside the frontmatter:
   ```bash
   grep -q "^agentdoc:" docs/agent-overlay.md
   ```
   - Block **absent** → **migrate**. Do NOT proceed to draft or maintain.
4. Block present: read `agentdoc.mode`.
   - `mode: multi` → **multi-overlay mode**: iterate over `agentdoc.subsystems` list.
     For each subsystem, check its spoke overlay for `completeness` and drift
     independently (same logic as steps 5-6 below, applied per spoke).
     Report aggregate status across all spokes.
   - `mode: single` or absent → single-overlay mode, continue to step 5.
5. Read frontmatter field `completeness`.
   - `completeness < 50` → **draft**.
   - Otherwise read `scan` hash and check `watch_paths` for drift (see maintain logic).
     - Any path has commits newer than `scan` → **maintain**.
     - No drift → **status** (docs are current, print dashboard and exit).

Always tell the user which phase was detected and why before proceeding.

---

## CRITICAL RULES (read before every action)

1. **Never modify a section not listed in `inferred_sections`.**
   Human-written sections are identified by their absence from `inferred_sections`.
   If a human section is affected by a code change, add a `<!-- agentdoc:review-needed: <reason> -->`
   comment immediately above the section heading. Never edit the section body.

2. **Never edit `CLAUDE.md` in maintain or draft phases.**
   `CLAUDE.md` has a human lifecycle. Only the init phase creates it; humans own it after that.

3. **Always update `scan` to the current HEAD hash after any write.**
   Run `git rev-parse HEAD` and put the result in `agentdoc.scan` in the overlay frontmatter.

4. **Keep `CLAUDE.md` under 120 lines.**
   If it would exceed this, move content to `docs/agent-overlay.md` instead.

5. **Keep each overlay under 400 lines.**
   In single mode: if draft detects the overlay will exceed 350 lines, stop and offer
   a reactive split (see draft Step 9). In multi mode: each spoke overlay has its own
   400-line limit; the hub overlay is an index only and should stay under 80 lines.

9. **In multi-overlay mode, validate ALL inputs before creating ANY file.**
   Check every subsystem name and source path provided by the user before writing
   anything to disk. If any validation fails, abort the entire operation and report
   every error at once. Never leave partial state.

10. **In multi-overlay mode, the AI proposes; the human decides.**
    Always present a suggested subsystem list and wait for the human to provide the
    final list. Never proceed with AI-suggested subsystems without explicit human confirmation.

6. **Never set `human_input` yourself.**
   It is set only by humans. Leave it unchanged during draft and maintain.

7. **Claims must be atomic and verifiable.**
   Each claim must be a single assertable fact traceable to a specific source file.
   No compound claims ("A does X and also Y") — split them.

8. **Always write comments in English.**

---

## Phase: init

*Precondition: `docs/agent-overlay.md` does not exist.*

### Step 1 — Scan repo structure

Run the following to understand the project:

```bash
# Language and build system
ls -1 Cargo.toml go.mod package.json pyproject.toml pom.xml build.gradle 2>/dev/null
# Task runner
ls -1 Justfile Makefile Taskfile.yml 2>/dev/null
# Pre-commit hooks
ls -1 lefthook.yaml lefthook.yml .pre-commit-config.yaml .husky 2>/dev/null
# Source layout (top-level only, no descend)
find src lib cmd internal pkg -maxdepth 1 -type d 2>/dev/null | head -20
# Test layout
find tests test spec -maxdepth 1 -type d 2>/dev/null | head -10
# CI
find .github/workflows .gitlab-ci.yml .tekton -maxdepth 2 -name "*.yml" -o -name "*.yaml" 2>/dev/null | head -10
# Existing docs
ls docs/ 2>/dev/null
```

Read: `Cargo.toml` / `package.json` / `go.mod` (whichever is present), `Justfile` or `Makefile`,
any `README.md`, any existing `CLAUDE.md` or `AGENTS.md`.

### Step 2 — Generate CLAUDE.md

**If `CLAUDE.md` does not exist:** create it with the template below.

**If `CLAUDE.md` already exists:** print its full content, then offer three options:

```
CLAUDE.md already exists. Choose an action:
  [o] overwrite     Replace entirely with a freshly generated version.
                    WARNING: existing content will be lost.
  [a] add reference Keep all existing content. Only add the overlay reference
                    line if it is not already present. Nothing else is changed.
  [s] skip          Leave CLAUDE.md untouched. You are responsible for adding
                    the overlay reference manually if needed.
```

Wait for the user's choice before proceeding.

- **overwrite**: generate a new CLAUDE.md using the template below.
- **add reference**: check whether the file already references `docs/agent-overlay.md`:
  ```bash
  grep -q "agent-overlay.md" CLAUDE.md
  ```
  - If found: tell the user the reference is already present and skip.
  - If not found: insert the following block immediately after the first `#` heading
    (or prepend it if there is no heading), then stop — do not modify anything else:
    ```markdown
    > For architecture, patterns, and how to extend this codebase,
    > read [`docs/agent-overlay.md`](docs/agent-overlay.md) before touching code.
    ```
- **skip**: leave the file entirely unchanged. Print a reminder:
  `Note: if CLAUDE.md does not reference docs/agent-overlay.md, agents will not
  load the overlay automatically. Add the reference line manually when convenient.`

`CLAUDE.md` must contain **only** information an agent cannot infer from reading the code:

```markdown
# CLAUDE.md

> For architecture, patterns, and how to extend this codebase,
> read [`docs/agent-overlay.md`](docs/agent-overlay.md) before touching code.

## Task runner

[List the task runner (just/make/cargo/npm) and its most-used recipes]

## Commands

| Goal | Command |
| ---- | ------- |
[Fill from scan: build, test, lint, format, coverage, release]

## Pre-commit hooks

[List hooks found, what they enforce, consequences of --no-verify]

## Source layout

| Path | Contents |
| ---- | -------- |
[Top-level directories and their role]

## Tests

[How to run tests. Which test suites require external resources (cluster, DB, etc.) and must
not be run locally. Where fixtures live.]

## Error handling

[The error type / library used and the propagation pattern]

## Release

[Release command and any constraints (never release manually, always use X)]
```

Omit any section where there is nothing non-obvious to say.

### Step 3 — Detect watch_paths

Identify the 8-15 most architecturally significant source files. Prefer:
- Entry points (main.rs, main.go, cmd/root.go, index.ts, server.rs)
- Interface / trait / abstract layer files (ports/, interfaces/, contracts/)
- Top-level module files (lib.rs, mod.rs, index files)
- Configuration/wiring files (di.go, app.module.ts, prodfactory.rs)
- Build descriptor (Cargo.toml, go.mod, package.json)

Do NOT include test files, fixture files, or generated code in `watch_paths`.

### Step 4 — Generate docs/agent-overlay.md stub

Create `docs/` if needed. Create `docs/agent-overlay.md`:

```markdown
---
agentdoc:
  scan: "<OUTPUT OF: git rev-parse HEAD>"
  freshness: 0
  human_input: 0
  completeness: 0
  inferred_sections:
    - id: architecture-summary
      heading: "## Architecture in One Paragraph"
    - id: module-map
      heading: "## Module Map"
    - id: key-patterns
      heading: "## Key Patterns & Conventions"
    - id: how-to-extend
      heading: "## How to Extend"
    - id: type-conventions
      heading: "## Type Conventions"
    - id: error-handling
      heading: "## Error Handling Pattern"
    - id: testing-pattern
      heading: "## Testing Pattern"
  watch_paths:
    [LIST watch_paths identified in Step 3]
  stale_sections: []
---

# Agent Overlay — [repo name]

## Architecture in One Paragraph

_[To be filled in draft phase]_

## Module Map

_[To be filled in draft phase]_

## Key Patterns & Conventions

_[To be filled in draft phase]_

## How to Extend

_[To be filled in draft phase]_

## Type Conventions

_[To be filled in draft phase]_

## Error Handling Pattern

_[To be filled in draft phase]_

## Testing Pattern

_[To be filled in draft phase]_

## What NOT to Do

_[Human-authored section — fill this after using the codebase. Not in inferred_sections.]_
```

### Step 5 — Initialize claims file

Create `docs/.agentdoc/claims.yml`:

```yaml
# agentdoc claims registry
# Human-readable: yes. Committed to git: recommended.
# To regenerate: /agentdoc draft

_meta:
  scan: "<git rev-parse HEAD>"
  generated_by: "agentdoc"
  version: "1"

_retired_ids: []

claims: []
```

### Step 6 — Generate status file

Create `docs/.agentdoc/status.json`:

```json
{
  "scan": "<git rev-parse HEAD>",
  "phase": "init",
  "overlay": {
    "freshness": 0,
    "human_input": 0,
    "completeness": 0,
    "stale_sections": []
  },
  "claims": {
    "total": 0,
    "inferred": 0,
    "stated": 0,
    "stale": 0
  },
  "ci_gates": {
    "freshness_pass": false,
    "human_input_pass": false,
    "completeness_pass": false
  }
}
```

### Step 7 — Report

Print:
- Files created
- List of `watch_paths` selected and why
- Sections that will be filled in draft phase
- Command to run next: `/agentdoc draft`

---

### Mode: multi — Multi-overlay init (`/agentdoc init --mode multi`)

*Use when the repo has clearly separated subsystems that need independent drift tracking.*
*The AI proposes; the human decides. No file is created until the human confirms the final list.*

#### Multi-init Step 1 — Scan and propose subsystems

Run the same repo scan as single-mode Step 1. Then analyze the result and identify
candidate subsystems based on:
- Top-level source directories with distinct concerns (`src/proxy/`, `src/tls/`, etc.)
- Workspace members in `Cargo.toml` / `package.json` workspaces
- Existing `docs/[subsystem]/` directories
- README sections that describe distinct components

Present the proposal clearly:

```
I detected the following candidate subsystems. This is a suggestion only —
you will provide the final list.

Suggested subsystems:
  1. proxy-data-plane   → src/proxy/, src/copy.rs, src/socket.rs
  2. tls-and-crypto     → src/tls/
  3. xds-and-state      → src/state/, src/xds/
  4. inpod-mode         → src/inpod/
  5. observability      → src/metrics/, src/admin/
  6. build-and-testing  → Cargo.toml, tests/, Justfile

Please provide your final subsystem list. For each subsystem, specify:
  - A name (letters, digits, hyphens only; starts with a letter)
  - The source paths it covers

Format (one per line):
  proxy: src/proxy/, src/copy.rs
  tls: src/tls/
  [...]

You may use the suggestions above, modify them, add or remove subsystems.
Minimum 2 subsystems (for a single overlay use /agentdoc init without --mode multi).
```

Wait for the human's response. Do not proceed until the list is received.

#### Multi-init Step 2 — Validate ALL inputs (abort on any error)

Run every check before creating any file. Collect ALL errors and report them together.

For each subsystem in the human-provided list:

```bash
# Name validation
[[ "$name" =~ ^[a-z][a-z0-9-]*$ ]] || error "Invalid name '$name': use lowercase letters, digits, hyphens; must start with a letter."

# Conflict check: would overwrite an existing spoke overlay?
[[ ! -f "docs/agent-overlay-${name}.md" ]] || error "File already exists: docs/agent-overlay-${name}.md"

# Source path existence
for path in $paths; do
  [[ -e "$path" ]] || error "Source path does not exist: '$path' (subsystem: $name)"
done
```

Additional checks:
- Duplicate names: `error "Duplicate subsystem name: '$name'"`
- Fewer than 2 subsystems: `error "Multi-overlay requires at least 2 subsystems. Use /agentdoc init for a single overlay."`
- Hub conflict: `[[ ! -f "docs/agent-overlay.md" ]] || error "docs/agent-overlay.md already exists. Run /agentdoc migrate instead."`

**If any error is found: print ALL errors, then abort. Do not create or modify any file.**

#### Multi-init Step 3 — Show files to be created and confirm

Print a full summary of what will be created:

```
Files to create:
  CLAUDE.md                                      (or modify — see options below)
  docs/agent-overlay.md                          (hub — index only)
  docs/STATUS.md                                 (aggregate health dashboard)
  docs/agent-overlay-proxy.md                    (spoke)
  docs/agent-overlay-tls.md                      (spoke)
  [... one per subsystem ...]
  docs/.agentdoc/claims-proxy.yml
  docs/.agentdoc/claims-tls.yml
  [... one per subsystem ...]
  docs/.agentdoc/status.json

Proceed? [y/N]
```

Wait for confirmation. If "n" or anything other than "y"/"yes": abort gracefully, create nothing.

Then handle CLAUDE.md as per single-mode Step 2 (overwrite / add reference / skip).

#### Multi-init Step 4 — Create files

Only after validation passes and the human confirms. Create in this order:

**Hub overlay** (`docs/agent-overlay.md`):

```yaml
---
agentdoc:
  mode: multi
  scan: "<git rev-parse HEAD>"
  subsystems:
    - name: proxy
      file: agent-overlay-proxy.md
      claims: .agentdoc/claims-proxy.yml
      watch_paths: ["src/proxy/", "src/copy.rs"]
    - name: tls
      file: agent-overlay-tls.md
      claims: .agentdoc/claims-tls.yml
      watch_paths: ["src/tls/"]
---

# Agent Overlay Hub — [repo name]

> For subsystem details see the spoke overlays linked below.
> This file covers cross-cutting concerns only.

## Subsystems

[For each subsystem: - [name](agent-overlay-[name].md) — one-line description]

## Cross-cutting Conventions

_[Human-authored: error handling pattern, commit conventions, test strategy.
Not in inferred_sections — fill this manually.]_
```

**Spoke overlays** (`docs/agent-overlay-[name].md`): one per subsystem, same stub
structure as single-mode Step 4 but scoped to the subsystem's paths.

**Claims files** (`docs/.agentdoc/claims-[name].yml`): empty, same structure as
single-mode Step 5 but with subsystem scope.

**STATUS.md** (`docs/STATUS.md`):

```markdown
# agentdoc Status

| Subsystem | Freshness | Human Input | Completeness | Stale Sections |
|---|---|---|---|---|
| [proxy](agent-overlay-proxy.md) | 0/100 | 0/100 | 0/100 | — |
| [tls](agent-overlay-tls.md) | 0/100 | 0/100 | 0/100 | — |

_Generated by agentdoc. Update with `/agentdoc status`._
```

#### Multi-init Step 5 — Report

```
MULTI-OVERLAY INIT COMPLETE
────────────────────────────────────────────────────
Hub:      docs/agent-overlay.md
Spokes:   [list]
Claims:   [list]
STATUS:   docs/STATUS.md

Next steps:
  1. Fill each spoke overlay:
     /agentdoc draft               ← fills all spokes with completeness < 50
  2. Add cross-cutting knowledge to the hub overlay manually
  3. Check aggregate health:
     /agentdoc status
────────────────────────────────────────────────────
```

---

## Phase: migrate

*Precondition: `docs/agent-overlay.md` exists but has no `agentdoc:` frontmatter block.*

The overlay was created outside agentdoc (manually, by another tool, or by a previous
approach like codebase-scribe or a custom prompt). Migration adds the `agentdoc:` block
without touching any existing content.

**This phase never modifies section content. It only adds frontmatter metadata.**

### Step 1 — Read and parse the existing overlay

Read `docs/agent-overlay.md` in full. Extract:

- All `##`-level headings, in order (these become candidate sections).
- The content body of each section (used to assess inferred vs human-written).
- All source file paths referenced in backtick spans (e.g. `` `src/foo.rs` ``, `` `src/ports/` ``).
  These are candidates for `watch_paths`.

Check for an existing frontmatter block (`---` delimiters). Note whether the file has one
(with unrelated keys) or none at all.

### Step 2 — Classify sections (interactive)

For each `##` heading, assess the content and classify it:

- **Inferred** (AI-generated): structured tables, systematic module maps, code patterns,
  step-by-step guides. Safe for the maintain phase to auto-update.
- **Human-written** (protected): narrative observations, gotchas, "don't do X" advice,
  architectural rationale written in first person, tribal knowledge. Must never be auto-modified.

Print a numbered list with your suggested classification and ask the user to confirm or
correct before proceeding:

```
Found 8 sections. Suggested classification (press Enter to accept, or list section
numbers to toggle, e.g. "3 5"):

  1. [inferred ] ## Architecture in One Paragraph
  2. [inferred ] ## Module Map
  3. [inferred ] ## Key Patterns & Conventions
  4. [inferred ] ## How to Extend
  5. [inferred ] ## Type Conventions
  6. [inferred ] ## Error Handling Pattern
  7. [inferred ] ## Testing Pattern
  8. [protected] ## What NOT to Do

Sections marked [protected] will NEVER be auto-modified by agentdoc.
```

Wait for user input. Apply any toggles. Confirm the final list before writing.

### Step 3 — Propose watch_paths (interactive)

From the source file paths extracted in Step 1, propose a watch_paths list:
- Deduplicate and sort
- Prefer entry points, traits/interfaces, and build descriptors
- Limit to 15 paths maximum

Print the proposed list and ask the user to confirm, add, or remove entries before proceeding.

### Step 4 — Assess completeness

Self-assess `completeness` (0-100) based on the sections present and their content depth.
Use the same rubric as the draft phase. Print the assessed value and reasoning.

### Step 5 — Write frontmatter

Generate the `agentdoc:` block from the confirmed data:

- `scan`: `git rev-parse HEAD` — conservative; assumes overlay reflects current code.
- `freshness`: 100 — optimistic but intentional. The user knows the state of their overlay.
  Running `/agentdoc maintain` afterwards will detect any real drift.
- `human_input`: 0 — conservative default. The user should update this manually to reflect
  how much human knowledge the overlay already contains.
- `completeness`: assessed value from Step 4.
- `inferred_sections`: confirmed list, with `id` derived from heading text (lowercase,
  spaces → hyphens, special chars removed). Example: `## Module Map` → `id: module-map`.
- `watch_paths`: confirmed list from Step 3.
- `stale_sections`: []

**If the file has no existing frontmatter block:** prepend a new `---` block at the top.

**If the file already has a frontmatter block** (with unrelated keys): add the `agentdoc:`
key inside the existing block, preserving all other keys.

After writing, verify the file opens correctly and content is intact below the frontmatter.

### Step 6 — Initialize support files

Create `docs/.agentdoc/claims.yml` with an empty claims list (do not attempt to extract
claims from the existing overlay — that is left for a future `/agentdoc draft` pass if desired):

```yaml
# agentdoc claims registry — initialized by migrate phase
# Run /agentdoc draft to populate claims from the existing overlay.
_meta:
  scan: "<git rev-parse HEAD>"
  generated_by: "agentdoc/migrate"
  version: "1"

_retired_ids: []
claims: []
```

Create `docs/.agentdoc/status.json` with current scores.

### Step 7 — Report

Print a summary:

```
MIGRATION COMPLETE
────────────────────────────────────────────────────
Overlay:         docs/agent-overlay.md
Frontmatter:     added (agentdoc: block)
Content:         unchanged

Inferred sections (auto-updatable):  <n>
Protected sections (human-owned):    <n>

watch_paths:     <count> paths configured
completeness:    <score>/100
human_input:     0  ← update this manually in the frontmatter to reflect
                       existing human knowledge in the overlay

Next steps:
  1. Review the frontmatter: docs/agent-overlay.md
     - Adjust human_input if the overlay already contains tribal knowledge
     - Verify inferred_sections and watch_paths look correct
  2. Check documentation health:
     ./scripts/agentdoc-check.sh status
  3. Detect drift since the overlay was written:
     /agentdoc maintain
  4. (Optional) Extract claims from existing content:
     /agentdoc draft
     Note: draft only fills sections with content '_[To be filled...]_'.
     Existing content is preserved; claims will be extracted from it.
────────────────────────────────────────────────────
```

---

## Phase: draft

*Precondition: `docs/agent-overlay.md` exists with `completeness < 50`.*

### Step 1 — Read state

Read `docs/agent-overlay.md` frontmatter:
- `inferred_sections`: list of sections to fill
- `watch_paths`: source files to read
- `human_input`: DO NOT change this value

Read all files listed in `watch_paths`. Also read:
- Any `docs/decisions/` or `adr/` files
- The README architecture section if present
- Any existing `CLAUDE.md`

### Step 2 — Fill each inferred section

For each section in `inferred_sections`, fill the content using this guidance:

**`## Architecture in One Paragraph`**
One dense paragraph (5-8 sentences) describing the full request/response or data flow,
naming the key types and modules. Must be specific enough that a reader can locate
any component in the codebase. No marketing language.

**`## Module Map`**
A Markdown table with columns: Path | Role | Notes.
One row per top-level source directory or key file.
Notes column: only non-obvious facts (visibility, build-time dependency, why it exists separately).

**`## Key Patterns & Conventions`**
Subsections (###) for each distinct pattern. Each subsection:
- Names the pattern
- Shows the canonical example (file path + what to look for)
- Lists variants or exceptions
Focus on patterns that repeat across the codebase and that an agent would need to follow
to write consistent code. Do NOT document single-use patterns.

**`## How to Extend`**
Numbered steps for the most common extension task (add a new endpoint, add a new domain,
add a new command, etc.). Be precise: which files to edit, in which order, what compile
errors to expect if a step is missed (useful for type-safe languages).

**`## Type Conventions`**
Covers: serialization strategy, naming of internal vs external types, nullability conventions,
enum naming, any project-specific type aliases. One paragraph per convention is enough.

**`## Error Handling Pattern`**
The error type, how errors propagate, how they are logged vs returned, any project-specific
error codes or categories. Include the import path of the error type.

**`## Testing Pattern`**
Unit test location and how to run them. Integration test location, what infrastructure they
need, what they test that unit tests cannot. How mocks/fakes are constructed. Any test
utilities or fixtures and where to find them.

### Step 3 — Extract claims

After filling sections, extract verifiable claims. Rules:
- One claim = one atomic, checkable fact about the codebase
- `type`: one of `pattern | constraint | technology | data_flow | boundary | decision`
- `source`: the file where the claim can be verified
- `provenance.origin`: `inferred` (AI-generated) or `stated` (human-written, e.g. from ADR)
- `provenance.confidence`: `high` (directly visible in code) | `medium` (inferred from structure) | `low` (inferred from naming/comments)

Aim for 5-15 claims per inferred section. Avoid claims that are trivially obvious from
reading any line of code; focus on non-obvious facts, invariants, and design decisions.

Write all claims to `docs/.agentdoc/claims.yml`.

### Step 4 — Assess completeness

After filling all sections, self-assess `completeness` (0-100):
- 100: all major subsystems documented, key patterns covered, how-to-extend is actionable
- 70-99: most subsystems covered, one or two gaps
- 50-69: skeleton filled but shallow; key patterns missing
- < 50: stub level

### Step 5 — Update frontmatter

Update `docs/agent-overlay.md` frontmatter:
- `scan`: `git rev-parse HEAD`
- `freshness`: 100
- `completeness`: assessed value
- `stale_sections`: []
- Leave `human_input` unchanged

### Step 6 — Mechanical review

Check that every file path, function name, type name, and command referenced in the
overlay actually exists in the repo:

```bash
# Example for Rust: check that referenced paths exist
grep -oP '`src/[^`]+`' docs/agent-overlay.md | tr -d '`' | while read f; do
  [ -e "$f" ] || echo "BROKEN REF: $f"
done
```

Fix any broken references before proceeding.

### Step 7 — Update status.json

Write updated `docs/.agentdoc/status.json` with current scores and claim counts.

### Step 8 — Report

Print:
- Completeness score and what was left incomplete
- Number of claims extracted
- Any broken references found and fixed
- Sections that need human input ("What NOT to Do", any section left as `_[To be filled...]_`)
- Command to check freshness: `./scripts/agentdoc-check.sh status`

### Step 9 — Reactive split detection (single mode only)

*Run this step after all sections are filled, before writing the overlay to disk.*

Count the total lines the filled overlay would have. If the count exceeds **350**:

```
The overlay has [N] lines, which approaches the 400-line limit.
This may indicate the repo has multiple distinct subsystems that would benefit
from separate overlays.

Based on the content I just analyzed, here are potential subsystems I can identify:

  1. [subsystem-name]  — covers: [list of source areas]
  2. [subsystem-name]  — covers: [list of source areas]
  [...]

This is a suggestion only. You decide.

Options:
  [s] split  — Convert to multi-overlay mode. You provide the final subsystem list.
  [k] keep   — Write the overlay as-is (will exceed the recommended limit).

Your choice: [wait for input]
```

**If the human chooses `keep`:** write the overlay as-is. Proceed normally.

**If the human chooses `split`:**

1. Ask the human for the final subsystem list in the same format as multi-init Step 1.
2. Run ALL validations from multi-init Step 2 before touching any file.
   If any validation fails: report all errors and abort. The existing partial overlay
   (still in memory, not yet written) is discarded. No files are created or modified.
3. Show the files that will be created (same as multi-init Step 3) and ask confirmation.
4. If confirmed: create the hub + spoke structure, distribute the already-filled content
   into the appropriate spoke overlays, create claims files per spoke.
   The in-memory overlay content is distributed — no re-reading of source files needed.
5. If not confirmed: abort, create nothing.

**Multi-overlay mode draft (when hub exists with `mode: multi`):**

When the hub overlay has `agentdoc.mode: multi`, draft processes each spoke
with `completeness < 50` independently. For each spoke:
- Read its `watch_paths` and `inferred_sections` from its own frontmatter
- Fill its sections
- Extract claims into its own claims file (`docs/.agentdoc/claims-[name].yml`)
- Update its frontmatter scores
- Update STATUS.md after all spokes are processed

---

## Phase: maintain

*Precondition: `docs/agent-overlay.md` exists with `completeness >= 50`.*

### Step 1 — Compute drift

Read `scan` hash and `watch_paths` from overlay frontmatter.

```bash
SCAN=$(grep -A1 'scan:' docs/agent-overlay.md | tail -1 | tr -d ' "')
git log --name-only --pretty=format: ${SCAN}..HEAD -- <each watch_path> 2>/dev/null
```

Collect the set of watch_paths files that have commits newer than `scan`.
If the set is empty: no drift detected. Print "Docs are current." and exit.

### Step 2 — Classify drift per changed file

For each changed file, read its current content and compare against all claims that
reference it (from `docs/.agentdoc/claims.yml`).

Classify each affected claim as:
- **Mechanical drift**: the claim references a path, name, or command that no longer exists.
  Auto-fix: update the claim and the corresponding text in the overlay inferred section.
- **Semantic drift**: the behavior described in the claim has changed, but the referenced
  path still exists. Flag for human review; do not auto-fix.

### Step 3 — Fix mechanical drift

For each mechanically drifted claim:
1. Identify which `inferred_section` contains text derived from this claim.
2. Re-read the changed source file.
3. Update only the affected sentences/lines in the inferred section.
4. Update the claim in `claims.yml` with the new content and reset `stale: false`.

Do NOT regenerate entire sections. Make surgical edits.

### Step 4 — Flag semantic drift

For each semantically drifted claim:
1. Add `stale: true` to the claim in `claims.yml`.
2. Identify the heading of the inferred section that contains text from this claim.
3. Add `<!-- agentdoc:review-needed: <brief reason> -->` on the line immediately
   above the section heading. Do NOT edit the section body.
4. Add the section heading to `stale_sections` in the overlay frontmatter.

For claims referencing a **non-inferred (human) section** that appears affected by the change:
- Add `<!-- agentdoc:review-needed: <reason> -->` above the section heading.
- Add the section to `stale_sections`.
- Never edit the body.

### Step 5 — Update frontmatter

Compute new `freshness`:

```
freshness = round(100 * unchanged_watch_paths / total_watch_paths)
```

Where `unchanged_watch_paths` = watch_paths with no commits newer than `scan`.

Update:
- `scan`: `git rev-parse HEAD`
- `freshness`: computed value
- `stale_sections`: list of section headings with `review-needed` comments

Leave `human_input` and `completeness` unchanged.

### Step 6 — Update status.json

Write updated `docs/.agentdoc/status.json` including:
- New freshness score
- Count of stale claims (mechanical fixed + semantic flagged)
- List of sections needing human review

### Step 7 — Report

Print a summary table:

```
MAINTAIN SUMMARY
────────────────────────────────────────────────────
Auto-fixed (mechanical drift):
  • [claim-id]: <what changed> in <file>

Flagged for human review (semantic drift):
  • [claim-id]: <reason> — affects section "## X"

Human sections flagged (adjacent code changed):
  • "## What NOT to Do" — review-needed comment added

Freshness: 87 → 94
Stale sections: ["## Key Patterns & Conventions"]
────────────────────────────────────────────────────
Next: review flagged sections, then remove review-needed comments.
When done: run /agentdoc maintain again to clear stale flags.
```

### Multi-overlay maintain

When the hub overlay has `agentdoc.mode: multi`, iterate over each spoke listed in
`agentdoc.subsystems`. For each spoke:

1. Read its own frontmatter (`scan`, `watch_paths`).
2. Compute drift for that spoke's watch_paths against its own `scan` hash.
3. Run Steps 2-6 of the single-overlay maintain independently for that spoke,
   using its own claims file (`docs/.agentdoc/claims-[name].yml`).
4. Update the spoke's frontmatter scores.

After all spokes are processed, regenerate `docs/STATUS.md` with the updated
aggregate scores from all spokes.

Print a consolidated summary table showing per-spoke drift results.

---

## Phase: status

*Print current documentation health dashboard. Never modifies any file.*

Read `docs/agent-overlay.md` frontmatter. Detect mode.

**Single-overlay mode:**

Read `docs/.agentdoc/claims.yml`. Compute current freshness (same formula as
maintain Step 5). Print:

```
AGENTDOC STATUS — [repo name]
────────────────────────────────────────────────────
Freshness:    [score]/100   [last scan: <hash short> on <date>]
Human input:  [score]/100
Completeness: [score]/100

Watch paths:
  ✓ src/server.rs          (no changes since scan)
  ✗ src/ports/jira.rs      (2 commits since scan)
  ✓ Cargo.toml             (no changes since scan)

Claims: [total] total, [inferred] inferred, [stated] stated, [stale] stale

Stale sections:
  [list or "none"]

CI gate (default thresholds — freshness ≥ 80, completeness ≥ 70):
  [PASS / FAIL]
────────────────────────────────────────────────────
```

**Multi-overlay mode:**

For each spoke in `agentdoc.subsystems`, compute freshness independently using
the spoke's own `scan` hash and `watch_paths`. Print a consolidated table:

```
AGENTDOC STATUS — [repo name] (multi-overlay)
────────────────────────────────────────────────────
Subsystem           Freshness  Human  Completeness  Stale
proxy-data-plane    94/100     20/100  90/100       none
tls-and-crypto      100/100    0/100   80/100       none
xds-and-state       75/100     30/100  95/100       ## Key Patterns

Aggregate:
  Lowest freshness:    75/100 (xds-and-state)
  Avg human input:     17/100
  Avg completeness:    88/100

CI gate (freshness ≥ 80 on ALL spokes):  FAIL (xds-and-state: 75)
────────────────────────────────────────────────────
```

In multi-overlay mode, the CI gate applies the freshness threshold to **each spoke
individually** — one stale spoke fails the whole gate. Update `docs/STATUS.md` with
the current scores after printing.

---

## Output file templates (reference)

### docs/.agentdoc/claims.yml claim entry

```yaml
- id: <topic>-<n>            # e.g. arch-1, testing-3
  type: pattern              # pattern|constraint|technology|data_flow|boundary|decision
  scope: overlay             # which doc file owns this claim
  claim: "<atomic statement>"
  source: "<path/to/file.rs>"
  provenance:
    origin: inferred         # inferred|stated
    confidence: high         # high|medium|low
  stale: false
```

### docs/.agentdoc/status.json

```json
{
  "scan": "<full git hash>",
  "generated_at": "<ISO-8601 timestamp>",
  "overlay": {
    "freshness": 94,
    "human_input": 20,
    "completeness": 90,
    "stale_sections": []
  },
  "claims": {
    "total": 42,
    "inferred": 35,
    "stated": 7,
    "stale": 2
  },
  "ci_gates": {
    "freshness_threshold": 80,
    "completeness_threshold": 70,
    "human_input_threshold": 0,
    "freshness_pass": true,
    "completeness_pass": true,
    "human_input_pass": true,
    "overall_pass": true
  }
}
```

### docs/agent-overlay.md frontmatter (complete)

```yaml
---
agentdoc:
  scan: "<full git hash>"
  freshness: 94
  human_input: 20
  completeness: 90
  inferred_sections:
    - id: architecture-summary
      heading: "## Architecture in One Paragraph"
    - id: module-map
      heading: "## Module Map"
    - id: key-patterns
      heading: "## Key Patterns & Conventions"
    - id: how-to-extend
      heading: "## How to Extend"
    - id: type-conventions
      heading: "## Type Conventions"
    - id: error-handling
      heading: "## Error Handling Pattern"
    - id: testing-pattern
      heading: "## Testing Pattern"
  watch_paths:
    - "src/server.rs"
    - "src/ports/"
    - "Cargo.toml"
  stale_sections: []
---
```
