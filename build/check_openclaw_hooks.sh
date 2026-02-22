#!/usr/bin/env bash
# Check OpenClaw repository for before_tool_call hook implementation status.
#
# Run periodically or in CI as an informational step.
# Exits 0 always (informational only).
#
# Usage:
#   ./build/check_openclaw_hooks.sh
#
# Requires: gh (GitHub CLI), authenticated

set -uo pipefail

echo "=== OpenClaw Hook Status Check ==="
echo ""

# Check if gh is available
if ! command -v gh &>/dev/null; then
    echo "SKIP: GitHub CLI (gh) not installed."
    echo "Install with: brew install gh"
    exit 0
fi

# Check if authenticated
if ! gh auth status &>/dev/null 2>&1; then
    echo "SKIP: GitHub CLI not authenticated."
    echo "Run: gh auth login"
    exit 0
fi

REPO="openclaw/openclaw"
HOOK_NAME="before_tool_call"

echo "Searching for '$HOOK_NAME' in $REPO..."
echo ""

# Search for code references
echo "--- Code references ---"
RESULTS=$(gh api "search/code?q=${HOOK_NAME}+repo:${REPO}+language:typescript" \
    --jq '.items[] | "  \(.path) (score: \(.score))"' 2>/dev/null || echo "")

if [ -z "$RESULTS" ]; then
    echo "  No code references found (hook not yet implemented)."
else
    echo "$RESULTS"
fi
echo ""

# Search for related issues/PRs
echo "--- Related issues/PRs ---"
ISSUES=$(gh api "search/issues?q=${HOOK_NAME}+repo:${REPO}" \
    --jq '.items[] | "  #\(.number) [\(.state)] \(.title)"' 2>/dev/null || echo "")

if [ -z "$ISSUES" ]; then
    echo "  No related issues or PRs found."
else
    echo "$ISSUES"
fi
echo ""

# Check recent commits in hooks-related paths
echo "--- Recent commits in hooks/ or lifecycle/ ---"
COMMITS=$(gh api "repos/${REPO}/commits?path=hooks&per_page=3" \
    --jq '.[] | "  \(.sha[:7]) \(.commit.message | split("\n")[0]) (\(.commit.author.date[:10]))"' 2>/dev/null || echo "")

if [ -z "$COMMITS" ]; then
    COMMITS=$(gh api "repos/${REPO}/commits?path=lifecycle&per_page=3" \
        --jq '.[] | "  \(.sha[:7]) \(.commit.message | split("\n")[0]) (\(.commit.author.date[:10]))"' 2>/dev/null || echo "")
fi

if [ -z "$COMMITS" ]; then
    echo "  No recent commits in hooks/ or lifecycle/ paths."
else
    echo "$COMMITS"
fi
echo ""

echo "=== Check complete ==="
echo "When $HOOK_NAME becomes available, update openobscure-plugin to enable hard enforcement."
