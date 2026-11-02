#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 The SplatStudy contributors
# SPDX-License-Identifier: GPL-3.0-or-later WITH LicenseRef-SplatStudy-app-store-exception
# PreToolUse hook: keeps Claude Code inside this project during the hackathon.
# Built with Claude Japan 2026 requires every line to be written during the
# event, so earlier prototypes in sibling folders must never be read.
# Blocks any tool call that names a folder under dev/ other than "splatstudy".

input=$(cat)

# Normalise separators (Windows, JSON-escaped, Git Bash), then pick the
# folder that follows dev/.
segments=$(printf '%s' "$input" \
  | tr '\\' '/' \
  | tr -s '/' \
  | grep -oiE '([a-z]:|/[a-z])/dev/[^/"'"'"' ]+' \
  | sed -E 's#.*/##')

for segment in $segments; do
  if [ "$(printf '%s' "$segment" | tr '[:upper:]' '[:lower:]')" != "splatstudy" ]; then
    echo "Blocked: '$segment' is outside this project. Hackathon rule: only code written during the event; never open other folders under dev/." >&2
    exit 2
  fi
done
exit 0
