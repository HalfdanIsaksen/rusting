#!/usr/bin/env bash
# Prints release-notes.md without the template's HTML comments and without
# empty sections, ready to publish as the GitHub release text.
set -euo pipefail
perl -0pe 's/<!--.*?-->//gs; s/^###[^\n]*\n\s*(?=###|\z)//gm; s/\A\s+//; s/\s*\z/\n/' "${1:-release-notes.md}"
