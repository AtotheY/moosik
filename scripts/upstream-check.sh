#!/bin/bash
# Compare upstream without integrating it or touching working files.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
case "${1:-}" in
    --fetch) git fetch upstream refs/heads/main:refs/remotes/upstream/main ;;
    "") ;;
    *) echo 'usage: scripts/upstream-check.sh [--fetch]' >&2; exit 2 ;;
esac
if ! git rev-parse --verify upstream/main >/dev/null 2>&1; then
    echo 'No upstream/main yet. Run scripts/upstream-check.sh --fetch.' >&2
    exit 1
fi
printf 'Moosik:  '; git log -1 --format='%h %s' HEAD
printf 'Refryt:  '; git log -1 --format='%h %s' upstream/main
printf 'Moosik-only / upstream-only commits: '; git rev-list --left-right --count HEAD...upstream/main
printf '\nUpstream commits not in this branch:\n'
git log --oneline HEAD..upstream/main
if [ -n "$(git status --porcelain)" ]; then
    printf '\nLocal changes exist. Review and checkpoint them before integration.\n'
fi
printf '\nNo source files changed and no merge performed. See docs/upstream-updates.md.\n'
