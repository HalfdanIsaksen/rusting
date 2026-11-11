#!/usr/bin/env bash
# Fails when release-notes.md is still the empty template or has no summary
# sentence, so a release never goes out without notes or with the previous
# release's notes. Used by .github/workflows/release.yml.
set -euo pipefail

notes="${1:-release-notes.md}"
template="${2:-scripts/release-notes-template.md}"

if [[ ! -s "$notes" ]]; then
  echo "$notes is missing or empty. Write the release notes first (see docs/releasing.md)." >&2
  exit 1
fi
if cmp -s "$notes" "$template"; then
  echo "$notes is still the empty template. Write the release notes first (see docs/releasing.md)." >&2
  exit 1
fi

# Drop HTML comments, then the first non-blank line must be the summary sentence.
summary="$(perl -0pe 's/<!--.*?-->//gs' "$notes" | sed -n '/[^[:space:]]/{p;q;}')"
if [[ -z "$summary" || "$summary" =~ ^[[:space:]]*(#|-|\*) ]]; then
  echo "$notes must start with a one-sentence summary before the first section." >&2
  exit 1
fi
echo "Release notes OK: $summary"
