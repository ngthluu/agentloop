# CD: Release-on-`production` + Quick Install Script — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When a commit lands on the `production` branch, GitHub Actions auto-tags the release (`v{Cargo.toml version}`), builds prebuilt `agentloop` binaries for three targets, and publishes them as a GitHub Release; users install with a single `curl … | bash` one-liner.

**Architecture:** One GitHub Actions workflow (`.github/workflows/release.yml`) with three jobs: `prepare` (reads the version from `Cargo.toml`, computes the tag, skips if it already exists, creates + pushes the tag), `build` (a 3-entry matrix that compiles `--release` per target and uploads each `agentloop-<target>.tar.gz` as a CI artifact), and `release` (downloads all artifacts and publishes a GitHub Release at the tag). A standalone `scripts/install.sh` detects the host OS/arch, downloads the matching asset from `releases/latest/download/…`, and installs the binary to `~/.local/bin`. The script factors platform detection into a sourceable `detect_target` function so it is unit-testable.

**Tech Stack:** Rust (cargo), GitHub Actions, `softprops/action-gh-release@v2`, `actions/upload-artifact@v4` / `download-artifact@v4`, POSIX-ish Bash, `shellcheck`, `actionlint`.

---

## File Structure

- **Create** `scripts/install.sh` — platform detection (`detect_target`), download, extract, install. Sourceable for tests; runs `main` only when executed/piped.
- **Create** `tests/install_test.sh` — sources `install.sh` and asserts `detect_target` mappings (supported + unsupported).
- **Create** `.github/workflows/release.yml` — the `prepare` → `build` → `release` pipeline triggered on push to `production`.
- **Modify** `README.md` — add an **Install** section (the one-liner) and a short **Releasing (CD)** section documenting the version-bump → push-`production` flow.

No existing CI exists, so there is nothing to follow/refactor. `Cargo.lock` is committed, so builds use `--locked`.

---

## Decisions (locked with the user)

- Build targets: `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`. (No Windows, no Linux arm64.)
- Tag scheme: read `version` from `Cargo.toml`, tag `v{version}`. If that tag already exists, the workflow no-ops cleanly (you bump the version to cut a release).
- Delivery: prebuilt binary from the **latest** GitHub Release; asset name carries the target only (`agentloop-<target>.tar.gz`) — the release/tag provides the version, so `install.sh` needs no version parsing and can use the `releases/latest/download/` redirect (no API/`jq` dependency).
- Only the `agentloop` binary is shipped; `fake_agent` is a test-only stub and is never released.

---

### Task 1: `install.sh` platform detection (test-first)

**Files:**
- Create: `tests/install_test.sh`
- Create: `scripts/install.sh`

- [ ] **Step 1: Write the failing test**

Create `tests/install_test.sh`:

```bash
#!/usr/bin/env bash
# Sources install.sh and verifies detect_target() maps OS/arch to rust targets.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=../scripts/install.sh
source "$SCRIPT_DIR/scripts/install.sh"

fail=0

assert_target() {
  local os="$1" arch="$2" want="$3" got
  got="$(detect_target "$os" "$arch" 2>/dev/null)" || got="<err>"
  if [ "$got" != "$want" ]; then
    printf 'FAIL: detect_target %s %s => %s (want %s)\n' "$os" "$arch" "$got" "$want"
    fail=1
  else
    printf 'ok:   detect_target %s %s => %s\n' "$os" "$arch" "$got"
  fi
}

assert_unsupported() {
  local os="$1" arch="$2"
  if detect_target "$os" "$arch" >/dev/null 2>&1; then
    printf 'FAIL: detect_target %s %s should be unsupported\n' "$os" "$arch"
    fail=1
  else
    printf 'ok:   detect_target %s %s rejected\n' "$os" "$arch"
  fi
}

assert_target Darwin arm64   aarch64-apple-darwin
assert_target Darwin aarch64 aarch64-apple-darwin
assert_target Darwin x86_64  x86_64-apple-darwin
assert_target Linux  x86_64  x86_64-unknown-linux-gnu
assert_target Linux  amd64   x86_64-unknown-linux-gnu
assert_unsupported Linux   aarch64
assert_unsupported Windows x86_64

if [ "$fail" -eq 0 ]; then echo "ALL PASS"; else echo "TESTS FAILED"; exit 1; fi
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `bash tests/install_test.sh`
Expected: FAIL — `scripts/install.sh` does not exist yet, so `source` errors with "No such file or directory" (non-zero exit).

- [ ] **Step 3: Write `scripts/install.sh`**

Create `scripts/install.sh`:

```bash
#!/usr/bin/env bash
# agentloop installer.
#   curl -fsSL https://raw.githubusercontent.com/ngthluu/agentloop/main/scripts/install.sh | bash
# Downloads the prebuilt binary for the host platform from the latest GitHub
# Release and installs it to ~/.local/bin (override with AGENTLOOP_INSTALL_DIR).
set -euo pipefail

REPO="ngthluu/agentloop"
BIN="agentloop"
DEFAULT_INSTALL_DIR="$HOME/.local/bin"

err() {
  printf 'error: %s\n' "$1" >&2
  exit 1
}

# detect_target <os> <arch> -> prints the rust target triple, or returns 1.
detect_target() {
  local os="$1" arch="$2"
  case "$os" in
    Darwin)
      case "$arch" in
        arm64|aarch64) echo "aarch64-apple-darwin" ;;
        x86_64)        echo "x86_64-apple-darwin" ;;
        *)             return 1 ;;
      esac
      ;;
    Linux)
      case "$arch" in
        x86_64|amd64) echo "x86_64-unknown-linux-gnu" ;;
        *)            return 1 ;;
      esac
      ;;
    *)
      return 1
      ;;
  esac
}

main() {
  local os arch target install_dir url tmp
  os="$(uname -s)"
  arch="$(uname -m)"
  target="$(detect_target "$os" "$arch")" || err "unsupported platform: $os $arch"

  install_dir="${AGENTLOOP_INSTALL_DIR:-$DEFAULT_INSTALL_DIR}"
  url="https://github.com/${REPO}/releases/latest/download/${BIN}-${target}.tar.gz"

  printf 'Installing %s (%s) to %s\n' "$BIN" "$target" "$install_dir"

  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT

  curl -fsSL "$url" -o "$tmp/${BIN}.tar.gz" || err "download failed: $url"
  tar -xzf "$tmp/${BIN}.tar.gz" -C "$tmp" || err "extract failed"
  [ -f "$tmp/$BIN" ] || err "archive did not contain '$BIN'"

  mkdir -p "$install_dir"
  install -m 0755 "$tmp/$BIN" "$install_dir/$BIN"

  printf '\n\xe2\x9c\x93 Installed %s to %s/%s\n' "$BIN" "$install_dir" "$BIN"
  case ":$PATH:" in
    *":$install_dir:"*) ;;
    *) printf '\n! %s is not on your PATH. Add to your shell profile:\n    export PATH="%s:$PATH"\n' "$install_dir" "$install_dir" ;;
  esac
}

# Run main only when executed/piped (curl | bash), not when sourced by tests.
if ! (return 0 2>/dev/null); then
  main "$@"
fi
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `bash tests/install_test.sh`
Expected: each `ok:` line prints, final line `ALL PASS`, exit 0.

- [ ] **Step 5: Lint the script**

Run: `shellcheck scripts/install.sh tests/install_test.sh`
(If `shellcheck` is missing: `brew install shellcheck`.)
Expected: no warnings/errors (exit 0).

- [ ] **Step 6: Make scripts executable and commit**

```bash
chmod +x scripts/install.sh tests/install_test.sh
git add scripts/install.sh tests/install_test.sh
git commit -m "feat(install): add platform-detecting install.sh + test"
```

---

### Task 2: Release workflow on push to `production`

**Files:**
- Create: `.github/workflows/release.yml`

- [ ] **Step 1: Write the workflow**

Create `.github/workflows/release.yml`:

```yaml
name: release

on:
  push:
    branches: [production]

permissions:
  contents: write

jobs:
  prepare:
    runs-on: ubuntu-latest
    outputs:
      version: ${{ steps.meta.outputs.version }}
      tag: ${{ steps.meta.outputs.tag }}
      should_release: ${{ steps.meta.outputs.should_release }}
    steps:
      - uses: actions/checkout@v4
        with:
          fetch-depth: 0
      - id: meta
        run: |
          version="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')"
          tag="v${version}"
          echo "version=${version}" >> "$GITHUB_OUTPUT"
          echo "tag=${tag}" >> "$GITHUB_OUTPUT"
          if git ls-remote --exit-code --tags origin "refs/tags/${tag}" >/dev/null 2>&1; then
            echo "tag ${tag} already exists; skipping release"
            echo "should_release=false" >> "$GITHUB_OUTPUT"
          else
            echo "should_release=true" >> "$GITHUB_OUTPUT"
          fi
      - name: Create and push tag
        if: steps.meta.outputs.should_release == 'true'
        run: |
          git config user.name "github-actions[bot]"
          git config user.email "41898282+github-actions[bot]@users.noreply.github.com"
          git tag -a "${{ steps.meta.outputs.tag }}" -m "Release ${{ steps.meta.outputs.tag }}"
          git push origin "${{ steps.meta.outputs.tag }}"

  build:
    needs: prepare
    if: needs.prepare.outputs.should_release == 'true'
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: macos-14
            target: aarch64-apple-darwin
          - os: macos-14
            target: x86_64-apple-darwin
          - os: ubuntu-latest
            target: x86_64-unknown-linux-gnu
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - name: Add rust target
        run: rustup target add ${{ matrix.target }}
      - name: Build release binary
        run: cargo build --release --locked --bin agentloop --target ${{ matrix.target }}
      - name: Package tarball
        run: |
          dir="target/${{ matrix.target }}/release"
          strip "$dir/agentloop" || true
          tar -C "$dir" -czf "agentloop-${{ matrix.target }}.tar.gz" agentloop
      - name: Upload artifact
        uses: actions/upload-artifact@v4
        with:
          name: agentloop-${{ matrix.target }}
          path: agentloop-${{ matrix.target }}.tar.gz
          if-no-files-found: error

  release:
    needs: [prepare, build]
    if: needs.prepare.outputs.should_release == 'true'
    runs-on: ubuntu-latest
    steps:
      - name: Download artifacts
        uses: actions/download-artifact@v4
        with:
          path: dist
          merge-multiple: true
      - name: Publish GitHub Release
        uses: softprops/action-gh-release@v2
        with:
          tag_name: ${{ needs.prepare.outputs.tag }}
          name: ${{ needs.prepare.outputs.tag }}
          generate_release_notes: true
          files: dist/*.tar.gz
          fail_on_unmatched_files: true
```

- [ ] **Step 2: Validate the workflow syntax**

Run: `actionlint .github/workflows/release.yml`
(If `actionlint` is missing: `brew install actionlint`.)
Expected: no output, exit 0.

Fallback if `actionlint` cannot be installed — validate YAML parses:
Run: `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml')); print('yaml ok')"`
Expected: `yaml ok`.

- [ ] **Step 3: Sanity-check the version-parse command used by `prepare`**

Run: `grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/'`
Expected: `0.1.0` (this is the value the workflow will turn into tag `v0.1.0`).

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci: release workflow — auto-tag + build + publish on push to production"
```

---

### Task 3: Document install + release process

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add an Install section after the intro**

In `README.md`, immediately after the opening description paragraph (before `## Requirements`), insert:

```markdown
## Install

Prebuilt binaries (macOS arm64/x86_64, Linux x86_64):

```bash
curl -fsSL https://raw.githubusercontent.com/ngthluu/agentloop/main/scripts/install.sh | bash
```

This installs the `agentloop` binary to `~/.local/bin` (override with
`AGENTLOOP_INSTALL_DIR=/usr/local/bin`). Ensure the install dir is on your `PATH`.
The `claude` and/or `codex` CLIs must also be on `PATH` at runtime.
```

- [ ] **Step 2: Add a Releasing (CD) section before `## Tests`**

In `README.md`, just before the `## Tests` section, insert:

```markdown
## Releasing (CD)

Releases are cut by pushing to the `production` branch:

1. Bump `version` in `Cargo.toml` (e.g. `0.1.0` -> `0.1.1`) and merge to `main`.
2. Fast-forward/merge `main` into `production` and push:

   ```bash
   git push origin main:production
   ```

3. The `release` workflow reads `version` from `Cargo.toml`, creates and pushes
   the tag `v{version}`, builds `agentloop` for each supported target, and
   publishes a GitHub Release with the `agentloop-<target>.tar.gz` assets.

If the tag `v{version}` already exists, the workflow no-ops — bump the version to
cut a new release. `install.sh` always fetches the **latest** release.
```

- [ ] **Step 3: Verify README renders the blocks correctly**

Run: `grep -n "install.sh | bash" README.md && grep -n "Releasing (CD)" README.md`
Expected: both lines found (non-empty output).

- [ ] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: install one-liner + CD release process"
```

---

### Task 4: Wire up `production` and verify end-to-end

This task ships the changes and proves the pipeline works. `install.sh` is fetched
from `main` via raw.githubusercontent, and `releases/latest/download` needs a
published release — so changes must reach `main` first, then `production` triggers
the release.

**Files:** none (git/CI operations only).

- [ ] **Step 1: Push the committed work to `main`**

```bash
git push origin main
```
Expected: push succeeds; the three commits from Tasks 1–3 are on `origin/main`.

- [ ] **Step 2: Create the `production` branch and trigger the release**

```bash
git push origin main:production
```
Expected: creates `origin/production` at the same commit as `main` and starts the `release` workflow.

- [ ] **Step 3: Watch the workflow run**

```bash
gh run list --workflow=release.yml --limit 1
gh run watch "$(gh run list --workflow=release.yml --limit 1 --json databaseId --jq '.[0].databaseId')"
```
Expected: `prepare`, all three `build` matrix legs, and `release` complete successfully (green).

- [ ] **Step 4: Confirm the release and assets exist**

```bash
gh release view v0.1.0 --json tagName,assets --jq '{tag: .tagName, assets: [.assets[].name]}'
```
Expected: `tag` is `v0.1.0` and `assets` contains exactly:
`agentloop-aarch64-apple-darwin.tar.gz`, `agentloop-x86_64-apple-darwin.tar.gz`, `agentloop-x86_64-unknown-linux-gnu.tar.gz`.

- [ ] **Step 5: Test the install one-liner into a throwaway dir**

```bash
AGENTLOOP_INSTALL_DIR="$(mktemp -d)" bash scripts/install.sh
```
(Run on this macOS arm64 machine; pulls `aarch64-apple-darwin`.)
Expected: prints `✓ Installed agentloop to <tmp>/agentloop`. Then confirm it runs:

```bash
"<the tmp dir printed above>/agentloop" --help
```
Expected: agentloop usage/help text prints (exit 0).

- [ ] **Step 6: Test the public curl one-liner**

```bash
curl -fsSL https://raw.githubusercontent.com/ngthluu/agentloop/main/scripts/install.sh | AGENTLOOP_INSTALL_DIR="$(mktemp -d)" bash
```
Expected: same `✓ Installed agentloop …` success output — confirms the hosted script + latest-release download path both work end-to-end.

---

## Self-Review

**Spec coverage:**
- "push to branch production → auto tag" → Task 2 `prepare` job (reads version, creates + pushes `v{version}`). ✓
- "auto build this app" → Task 2 `build` matrix (3 targets) + `release` job publishes assets. ✓
- "quick install like curl … | bash" → Task 1 `install.sh` + Task 3 documented one-liner + Task 4 Step 6 live test. ✓

**Placeholder scan:** No TBD/TODO/"handle edge cases"/"add validation" — every code/command step shows full content. ✓

**Type/name consistency:**
- Asset name `agentloop-<target>.tar.gz` is identical across the workflow (`build` package + upload), `install.sh` (`${BIN}-${target}.tar.gz`), and Task 4 verification. ✓
- `detect_target` outputs (`aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`) exactly match the workflow matrix `target` values and the test assertions. ✓
- Tag `v0.1.0` (= `v{version}`) is consistent between workflow output, `gh release view`, and docs. ✓
- `AGENTLOOP_INSTALL_DIR` env override name matches between `install.sh`, README, and verification steps. ✓

**Notes / caveats:**
- The sourced-vs-executed guard `if ! (return 0 2>/dev/null)` correctly runs `main` under `curl | bash` (not sourced) while letting `tests/install_test.sh` source `detect_target` without executing `main`.
- `x86_64-apple-darwin` is cross-compiled on the `macos-14` (arm64) runner via `rustup target add`; the Apple toolchain supports this. `strip` is guarded with `|| true`.
- Only `agentloop` is built/shipped (`--bin agentloop`); the `fake_agent` test stub is excluded.
