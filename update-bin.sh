#!/bin/bash
# update-bin.sh — build all native user programs and copy ELFs to bin/
#
# Usage: ./update-bin.sh [--clean]
#   --clean    also run "make clean" before building

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
USER_BIN="$SCRIPT_DIR/user/bin"
OUT_DIR="$SCRIPT_DIR/bin"

# Colour output
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'

clean_first=false
if [[ "${1:-}" == "--clean" ]]; then
    clean_first=true
fi

mkdir -p "$OUT_DIR"

echo "==> Scanning $USER_BIN ..."
built=0
failed=0

for dir in "$USER_BIN"/*/; do
    name=$(basename "$dir")

    # Skip non-program directories (e.g. shared crates)
    if [[ ! -f "$dir/Makefile" ]] && [[ ! -f "$dir/Cargo.toml" ]]; then
        continue
    fi

    echo ""
    echo "──► Building $name ..."

    pushd "$dir" > /dev/null

    if $clean_first; then
        make clean > /dev/null 2>&1 || true
    fi

    if make 2>&1 | tail -5; then
        if [[ -f "$name" ]]; then
            cp "$name" "$OUT_DIR/$name"
            echo -e "    ${GREEN}OK${NC}  →  $OUT_DIR/$name"
            built=$((built + 1))
        else
            echo -e "    ${RED}FAIL${NC}: binary '$name' not found after build"
            failed=$((failed + 1))
        fi
    else
        echo -e "    ${RED}FAIL${NC}: build error"
        failed=$((failed + 1))
    fi

    popd > /dev/null
done

echo ""
echo "==> Done: $built built, $failed failed"
if [[ $failed -gt 0 ]]; then
    exit 1
fi
