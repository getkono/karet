#!/bin/sh
# Enforce the maintainability ceiling from issue #81 using tokei's Rust code count.
set -eu

limit=800

if ! command -v tokei >/dev/null 2>&1; then
    echo "error: tokei is required (run 'mise install')" >&2
    exit 2
fi
if ! command -v jq >/dev/null 2>&1; then
    echo "error: jq is required (run 'mise install')" >&2
    exit 2
fi

offenders=$(
    tokei --output json --type Rust . |
        jq -r --argjson limit "$limit" '
            [(.Rust.reports // [])[]
                | select(.stats.code > $limit)
                | { name, code: .stats.code }]
            | sort_by(-.code, .name)
            | .[]
            | "\(.name): \(.code) code lines"
        '
)

if [ -n "$offenders" ]; then
    echo "Rust files exceeding the ${limit}-code-line limit:" >&2
    echo "$offenders" >&2
    exit 1
fi

echo "All Rust files are within the ${limit}-code-line limit."
