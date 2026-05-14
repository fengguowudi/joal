#!/bin/bash

# Transmission Client Analyzer
# Extracts version, user-agent, and peer-id information from Transmission source code
# Generates .client files for use with JOAL

set -euo pipefail

readonly SCRIPT_NAME="transmission_analyzer"
readonly TEMP_DIR="${TMPDIR:-/tmp}/${SCRIPT_NAME}_$$"
readonly CACHE_DIR="${HOME}/.cache/${SCRIPT_NAME}"

readonly RED='\033[0;31m'
readonly GREEN='\033[0;32m'
readonly NC='\033[0m'

USE_CACHE=true
FORCE_DOWNLOAD=false

cleanup() {
    if [[ -d "$TEMP_DIR" ]]; then
        rm -rf "$TEMP_DIR"
    fi
}
trap cleanup EXIT

info() { echo -e "[INFO] $*" >&2; }
warn() { echo -e "[WARN] $*" >&2; }
error_exit() { echo -e "[ERROR] $*" >&2; exit 1; }

check_dependencies() {
    local deps=("curl" "jq" "tar" "grep")
    local missing=()
    for dep in "${deps[@]}"; do
        if ! command -v "$dep" >/dev/null 2>&1; then
            missing+=("$dep")
        fi
    done
    if [[ ${#missing[@]} -gt 0 ]]; then
        error_exit "Missing dependencies: ${missing[*]}"
    fi
}

setup_cache() {
    [[ -d "$CACHE_DIR" ]] || mkdir -p "$CACHE_DIR"
}

get_releases() {
    curl -s "https://api.github.com/repos/transmission/transmission/releases" \
        || error_exit "Failed to fetch releases from GitHub API"
}

is_stable_release() {
    local tag="$1"
    [[ ! "$tag" =~ (alpha|beta|rc|dev|pre) ]] && [[ "$tag" =~ ^[0-9]+\.[0-9]+ ]]
}

# Parse version string into major.minor.patch
# Transmission uses: "2.94" (=2.9.4), "3.00" (=3.0.0), "4.0.6" (=4.0.6)
parse_version() {
    local tag="$1"
    local major minor patch

    if [[ "$tag" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
        # Format: X.Y.Z (4.0.6)
        major="${BASH_REMATCH[1]}"
        minor="${BASH_REMATCH[2]}"
        patch="${BASH_REMATCH[3]}"
    elif [[ "$tag" =~ ^([0-9]+)\.([0-9])([0-9])$ ]]; then
        # Format: X.YZ (2.94 = 2.9.4, 3.00 = 3.0.0)
        major="${BASH_REMATCH[1]}"
        minor="${BASH_REMATCH[2]}"
        patch="${BASH_REMATCH[3]}"
    else
        echo ""
        return 1
    fi

    echo "$major $minor $patch"
}

# Compute peer-id prefix using BASE62 encoding (matches Transmission's update-version-h.sh)
compute_peer_id_prefix() {
    local major="$1" minor="$2" patch="$3"
    local BASE62=($(echo {0..9} {A..Z} {a..z}))
    echo "-TR${BASE62[$major]}${BASE62[$minor]}${BASE62[$patch]}0-"
}

# Determine Accept-Encoding header based on version
get_accept_encoding() {
    local major="$1"
    if [[ $major -ge 3 ]]; then
        echo "deflate, gzip"
    else
        echo "gzip;q=1.0, deflate, identity"
    fi
}

# Generate .client JSON for a given version
generate_client_config() {
    local tag="$1"
    local version_parts
    version_parts=$(parse_version "$tag") || error_exit "Cannot parse version: $tag"
    read -r major minor patch <<< "$version_parts"

    local peer_id_prefix
    peer_id_prefix=$(compute_peer_id_prefix "$major" "$minor" "$patch")

    local accept_encoding
    accept_encoding=$(get_accept_encoding "$major")

    local user_agent="Transmission/${tag}"

    jq -n \
        --arg peer_id_prefix "$peer_id_prefix" \
        --arg user_agent "$user_agent" \
        --arg accept_encoding "$accept_encoding" \
        '{
            keyGenerator: {
                algorithm: {
                    type: "DIGIT_RANGE_TRANSFORMED_TO_HEX_WITHOUT_LEADING_ZEROES",
                    inclusiveLowerBound: 1,
                    inclusiveUpperBound: 2147483647
                },
                refreshOn: "NEVER",
                keyCase: "lower"
            },
            peerIdGenerator: {
                algorithm: {
                    type: "RANDOM_POOL_WITH_CHECKSUM",
                    prefix: $peer_id_prefix,
                    charactersPool: "0123456789abcdefghijklmnopqrstuvwxyz",
                    base: 36
                },
                refreshOn: "TORRENT_VOLATILE",
                shouldUrlEncode: false
            },
            urlEncoder: {
                encodingExclusionPattern: "[A-Za-z0-9-]",
                encodedHexCase: "lower"
            },
            query: "info_hash={infohash}&peer_id={peerid}&port={port}&uploaded={uploaded}&downloaded={downloaded}&left={left}&numwant={numwant}&key={key}&compact=1&supportcrypto=1&event={event}&ipv6={ipv6}",
            numwant: 80,
            numwantOnStop: 0,
            requestHeaders: [
                { name: "User-Agent", value: $user_agent },
                { name: "Accept", value: "*/*" },
                { name: "Accept-Encoding", value: $accept_encoding }
            ]
        }'
}

# Extract version tag from existing client filename
extract_version_from_filename() {
    local filename="$1"
    basename "$filename" | sed -E 's/^transmission-//; s/(_[0-9]+)?\.client$//'
}

# Batch update: generate missing .client files
run_batch_update() {
    local clients_dir="$1"

    if [[ ! -d "$clients_dir" ]]; then
        error_exit "Directory not found: $clients_dir"
    fi

    info "Starting Transmission batch update for: $clients_dir"

    # Collect existing versions (normalize to tag format)
    local existing_versions=()
    if ls "$clients_dir"/transmission-*.client >/dev/null 2>&1; then
        while IFS= read -r file; do
            local ver
            ver=$(extract_version_from_filename "$file")
            existing_versions+=("$ver")
        done < <(ls "$clients_dir"/transmission-*.client)
        info "Found ${#existing_versions[@]} existing Transmission client files"
    fi

    # Fetch stable releases from GitHub
    local releases
    releases=$(get_releases)

    local all_stable=()
    while IFS= read -r tag; do
        if is_stable_release "$tag"; then
            all_stable+=("$tag")
        fi
    done < <(echo "$releases" | jq -r '.[].tag_name')

    info "Found ${#all_stable[@]} stable releases on GitHub"

    # Find missing versions
    local missing=()
    for tag in "${all_stable[@]}"; do
        local found=false
        for existing in "${existing_versions[@]}"; do
            if [[ "$tag" == "$existing" ]]; then
                found=true
                break
            fi
        done
        if [[ "$found" == false ]]; then
            # Verify we can parse this version
            if parse_version "$tag" >/dev/null 2>&1; then
                missing+=("$tag")
            else
                warn "Skipping unparseable version: $tag"
            fi
        fi
    done

    if [[ ${#missing[@]} -eq 0 ]]; then
        info "All Transmission stable releases are up to date!"
        return 0
    fi

    info "Generating ${#missing[@]} missing client files: ${missing[*]}"

    local processed=0
    for tag in "${missing[@]}"; do
        local output_file="${clients_dir}/transmission-${tag}.client"
        if generate_client_config "$tag" > "$output_file"; then
            info "Generated: $(basename "$output_file")"
            processed=$((processed + 1))
        else
            warn "Failed to generate for version $tag"
            rm -f "$output_file"
        fi
    done

    info "Batch update complete! Generated $processed new Transmission client files."
}

usage() {
    cat << EOF
Usage: $0 [OPTIONS]

Generate Transmission .client files for JOAL.

Options:
    -h, --help              Show this help message
    --batch-update DIR      Scan directory and generate missing .client files
    --version VER           Generate a single .client file for the given version
    --list-releases         List available stable releases

Examples:
    $0 --batch-update resources/clients
    $0 --version 4.0.6
    $0 --list-releases
EOF
}

main() {
    local batch_update_dir=""
    local single_version=""
    local list_flag=false

    while [[ $# -gt 0 ]]; do
        case $1 in
            -h|--help) usage; exit 0 ;;
            --batch-update) batch_update_dir="$2"; shift 2 ;;
            --version) single_version="$2"; shift 2 ;;
            --list-releases) list_flag=true; shift ;;
            *) error_exit "Unknown option: $1" ;;
        esac
    done

    check_dependencies

    if [[ "$list_flag" == true ]]; then
        local releases
        releases=$(get_releases)
        echo "Available stable Transmission releases:"
        echo "$releases" | jq -r '.[].tag_name' | while read -r tag; do
            if is_stable_release "$tag"; then
                echo "  $tag"
            fi
        done
        exit 0
    fi

    if [[ -n "$batch_update_dir" ]]; then
        run_batch_update "$batch_update_dir"
        exit 0
    fi

    if [[ -n "$single_version" ]]; then
        generate_client_config "$single_version"
        exit 0
    fi

    usage
    exit 1
}

main "$@"
