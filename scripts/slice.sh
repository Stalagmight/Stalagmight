#!/bin/bash
#
# slice.sh - Smart file slicing with AST parsing
#
# Extracts specific functions, classes, or sections from large source files
# using proper AST parsing to prevent token limit errors.
#
# Supported languages:
#   - TypeScript/JavaScript (.ts, .tsx, .js, .jsx, .mjs, .cjs)
#   - Rust (.rs)
#
# Usage:
#   ./scripts/slice.sh <file> --toc                    # Table of contents
#   ./scripts/slice.sh <file> --fn <name>              # Extract function/method
#   ./scripts/slice.sh <file> --class <name>           # Extract class (TS/JS)
#   ./scripts/slice.sh <file> --interface <name>       # Extract interface/type (TS/JS)
#   ./scripts/slice.sh <file> --component <name>       # Extract React component (TS/JS)
#   ./scripts/slice.sh <file> --impl <name>            # Extract impl block (Rust)
#   ./scripts/slice.sh <file> --struct <name>          # Extract struct (Rust)
#   ./scripts/slice.sh <file> --enum <name>            # Extract enum (Rust)
#   ./scripts/slice.sh <file> --trait <name>           # Extract trait (Rust)
#   ./scripts/slice.sh <file> --lines <start>-<end>    # Extract line range
#
# Examples:
#   ./scripts/slice.sh src-tauri/src/core/state.rs --toc
#   ./scripts/slice.sh src-tauri/src/core/state.rs --fn execute_command
#   ./scripts/slice.sh src-tauri/src/core/state.rs --impl AppState
#   ./scripts/slice.sh src/components/Map/SurveyLayer.ts --toc
#   ./scripts/slice.sh src/components/Map/SurveyLayer.ts --component SurveyLayer
#   ./scripts/slice.sh src/store/surveyStore.ts --fn useStore
#   ./scripts/slice.sh src/types/survey.ts --interface SurveyData

set -e

# ===== Color output =====
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

info() { echo -e "${BLUE}[INFO]${NC} $1" >&2; }
warn() { echo -e "${YELLOW}[WARN]${NC} $1" >&2; }
error() { echo -e "${RED}[ERROR]${NC} $1" >&2; }
success() { echo -e "${GREEN}[OK]${NC} $1" >&2; }

# ===== Configuration =====
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
LIB_DIR="$SCRIPT_DIR/lib"
SLICE_TS="$LIB_DIR/slice-ts.mjs"
SLICE_RS_DIR="$LIB_DIR/slice-rs"
SLICE_RS_BIN="$SLICE_RS_DIR/target/release/slice-rs"

# ===== Help =====
show_help() {
    cat << 'EOF'
slice.sh - Smart file slicing with AST parsing

USAGE:
    ./scripts/slice.sh <file> <mode> [target]

MODES:
    --toc                    Show table of contents (all declarations)
    --fn <name>              Extract function or method
    --lines <start>-<end>    Extract line range with context

    TypeScript/JavaScript only:
    --class <name>           Extract class declaration
    --interface <name>       Extract interface or type alias
    --component <name>       Extract React component
    --method <name>          Extract method from class

    Rust only:
    --impl <name>            Extract impl block for type
    --struct <name>          Extract struct definition
    --enum <name>            Extract enum definition
    --trait <name>           Extract trait definition
    --mod <name>             Extract module

EXAMPLES:
    # Show what's in a large Rust file
    ./scripts/slice.sh src-tauri/src/core/state.rs --toc

    # Extract a specific function from Rust
    ./scripts/slice.sh src-tauri/src/core/state.rs --fn execute_command

    # Extract an impl block
    ./scripts/slice.sh src-tauri/src/core/state.rs --impl AppState

    # Show contents of a TypeScript file
    ./scripts/slice.sh src/components/Map/SurveyLayer.ts --toc

    # Extract a React component
    ./scripts/slice.sh src/components/Map/SurveyLayer.ts --component SurveyLayer

    # Extract a specific interface
    ./scripts/slice.sh src/types/survey.ts --interface Station

    # Extract lines 100-200
    ./scripts/slice.sh src/store/surveyStore.ts --lines 100-200

SUPPORTED FILE TYPES:
    TypeScript:  .ts, .tsx
    JavaScript:  .js, .jsx, .mjs, .cjs
    Rust:        .rs
EOF
}

# ===== Build Rust helper if needed =====
ensure_rust_parser() {
    if [[ ! -f "$SLICE_RS_BIN" ]]; then
        info "Building Rust parser (first-time setup)..."

        if ! command -v cargo &> /dev/null; then
            error "Rust/Cargo not found. Install from https://rustup.rs/"
            exit 1
        fi

        (cd "$SLICE_RS_DIR" && cargo build --release --quiet 2>&1) || {
            error "Failed to build Rust parser"
            exit 1
        }

        success "Rust parser built: $SLICE_RS_BIN"
    fi
}

# ===== Detect language from extension =====
detect_language() {
    local file="$1"
    local ext="${file##*.}"

    case "$ext" in
        ts|tsx|js|jsx|mjs|cjs)
            echo "typescript"
            ;;
        rs)
            echo "rust"
            ;;
        *)
            echo "unknown"
            ;;
    esac
}

# ===== Main =====
main() {
    if [[ $# -lt 2 ]] || [[ "$1" == "--help" ]] || [[ "$1" == "-h" ]]; then
        show_help
        exit 0
    fi

    local file="$1"
    shift

    # Handle relative paths
    if [[ ! "$file" = /* ]]; then
        file="$PROJECT_ROOT/$file"
    fi

    if [[ ! -f "$file" ]]; then
        error "File not found: $file"
        exit 1
    fi

    local lang=$(detect_language "$file")

    case "$lang" in
        typescript)
            # Use Node.js TypeScript parser
            if ! command -v node &> /dev/null; then
                error "Node.js not found"
                exit 1
            fi
            node "$SLICE_TS" "$file" "$@"
            ;;
        rust)
            ensure_rust_parser
            "$SLICE_RS_BIN" "$file" "$@"
            ;;
        *)
            error "Unsupported file type: $file"
            echo "Supported: .ts, .tsx, .js, .jsx, .mjs, .cjs, .rs" >&2
            exit 1
            ;;
    esac
}

main "$@"
