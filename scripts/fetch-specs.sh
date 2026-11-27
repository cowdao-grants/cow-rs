#!/usr/bin/env bash
#
# Refresh the upstream conformance specs in ./specs/ from the shas
# pinned in parity/source-lock.toml. Bump the shas in source-lock.toml
# first, run this, then `just test` (the schema_validation tests will
# fail loudly if the shape of any spec has drifted in a way our types
# do not handle).

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
lock="${repo_root}/parity/source-lock.toml"
out="${repo_root}/specs"

# Extract the `sha = "..."` value following a given `name = "<name>"`
# block in the lock. POSIX-shell-only; avoids a TOML parser dependency.
sha_for() {
    local name="$1"
    awk -v n="$name" '
        $0 ~ "^name = \"" n "\"$" { found=1; next }
        found && /^sha = / { match($0, /"[^"]+"/); print substr($0, RSTART+1, RLENGTH-2); exit }
    ' "$lock"
}

services_sha="$(sha_for services)"
appdata_sha="$(sha_for app-data)"
subgraph_sha="$(sha_for subgraph)"

if [[ -z "$services_sha" || -z "$appdata_sha" || -z "$subgraph_sha" ]]; then
    echo "error: could not extract one or more shas from $lock" >&2
    exit 1
fi

mkdir -p "$out"

fetch() {
    local url="$1"
    local dest="$2"
    echo "fetch $dest"
    curl -fsSL "$url" -o "$dest"
}

fetch \
    "https://raw.githubusercontent.com/cowprotocol/services/${services_sha}/crates/orderbook/openapi.yml" \
    "${out}/orderbook-api.yml"

fetch \
    "https://raw.githubusercontent.com/cowprotocol/app-data/${appdata_sha}/src/schemas/v1.6.0.json" \
    "${out}/app-data-v1.6.0.json"

fetch \
    "https://raw.githubusercontent.com/cowprotocol/app-data/${appdata_sha}/src/schemas/definitions.json" \
    "${out}/app-data-definitions.json"

fetch \
    "https://raw.githubusercontent.com/cowprotocol/app-data/${appdata_sha}/src/schemas/partnerFee/v1.0.0.json" \
    "${out}/app-data-partner-fee-v1.0.0.json"

fetch \
    "https://raw.githubusercontent.com/cowprotocol/app-data/${appdata_sha}/src/schemas/quote/v1.1.0.json" \
    "${out}/app-data-quote-v1.1.0.json"

fetch \
    "https://raw.githubusercontent.com/cowprotocol/app-data/${appdata_sha}/src/schemas/referrer/v0.2.0.json" \
    "${out}/app-data-referrer-v0.2.0.json"

fetch \
    "https://raw.githubusercontent.com/cowprotocol/subgraph/${subgraph_sha}/schema.graphql" \
    "${out}/subgraph.graphql"

echo
echo "done. Now run: just test"
