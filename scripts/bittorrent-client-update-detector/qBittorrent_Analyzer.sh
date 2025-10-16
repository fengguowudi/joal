#!/bin/bash

# qBittorrent Client Analyzer - Fixed Version
# Extracts version, user-agent, and peer-id information from qBittorrent source code
# Supports both GitHub API responses and direct tarball URLs
# Compatible with qBittorrent v4.x and v5.x+ (handles session.cpp -> sessionimpl.cpp change)

set -euo pipefail

# Configuration
readonly SCRIPT_NAME="qbt_analyzer"
readonly TEMP_DIR="${TMPDIR:-/tmp}/${SCRIPT_NAME}_$$"
readonly CACHE_DIR="${HOME}/.cache/${SCRIPT_NAME}"
readonly LOG_LEVEL="${LOG_LEVEL:-INFO}"

# Colors for output
readonly RED='\033[0;31m'
readonly GREEN='\033[0;32m'
readonly YELLOW='\033[1;33m'
readonly BLUE='\033[0;34m'
readonly NC='\033[0m' # No Color

# Global variables
VERBOSE=false
FORCE_DOWNLOAD=false
OUTPUT_FORMAT="text"
MAJOR_VERSION=""
USE_CACHE=true

# Cleanup function
cleanup() {
    if [[ -d "$TEMP_DIR" ]]; then
        rm -rf "$TEMP_DIR"
    fi
}

# Set up cleanup trap
trap cleanup EXIT

# Logging functions
log() {
    local level="$1"
    shift
    local timestamp=$(date '+%Y-%m-%d %H:%M:%S')
    echo -e "[${timestamp}] [${level}] $*" >&2
}

info() { log "INFO" "$@"; }
warn() { log "WARN" "$@"; }
error() { log "ERROR" "$@"; }

error_exit() {
    error "$@"
    exit 1
}

# Dependency checking
check_dependencies() {
    local deps=("curl" "jq" "tar" "grep" "cut" "head" "tr")
    local missing=()
    
    for dep in "${deps[@]}"; do
        if ! command -v "$dep" >/dev/null 2>&1; then
            missing+=("$dep")
        fi
    done
    
    if [[ ${#missing[@]} -gt 0 ]]; then
        error_exit "Missing required dependencies: ${missing[*]}"
    fi
}

# Create cache directory
setup_cache() {
    if [[ ! -d "$CACHE_DIR" ]]; then
        mkdir -p "$CACHE_DIR"
    fi
}

# Clear cache function
clear_cache() {
    if [[ -d "$CACHE_DIR" ]]; then
        local cache_size
        cache_size=$(du -sh "$CACHE_DIR" 2>/dev/null | cut -f1)
        local file_count
        file_count=$(find "$CACHE_DIR" -name "*.tar.gz" 2>/dev/null | wc -l)
        
        info "Found cache directory: $CACHE_DIR"
        info "Cache size: ${cache_size:-0B}"
        info "Cached files: ${file_count:-0}"
        
        if [[ ${file_count:-0} -gt 0 ]]; then
            info "Clearing cache..."
            rm -rf "$CACHE_DIR"/*
            info "Cache cleared successfully!"
        else
            info "Cache is already empty."
        fi
    else
        info "No cache directory found at: $CACHE_DIR"
    fi
}

# GitHub API functions
get_github_api_url() {
    echo "https://api.github.com/repos/qbittorrent/qBittorrent/tags"
}

# Check if input looks like a version number (e.g., "5.0.4", "release-5.0.4", "v5.0.4", or partial like "5.0")
is_version_number() {
    local input="$1"
    # Match patterns like: 5.0.4, v5.0.4, release-5.0.4, 5.0, etc.
    [[ "$input" =~ ^(v|release-)?[0-9]+(\.[0-9]+)?(\.[0-9]+)?([a-z]+[0-9]*)?$ ]]
}

# Normalize version number (remove prefixes)
normalize_version() {
    local input="$1"
    # Remove common prefixes and return clean version
    echo "$input" | sed -E 's/^(v|release-)//'
}

# Find release URL by version number
find_release_by_version() {
    local target_version="$1"
    local releases
    releases=$(get_releases)
    
    # Normalize the target version
    target_version=$(normalize_version "$target_version")
    
    info "Looking for version: $target_version"
    
    # Look for exact match first
    local found_url=""
    while IFS= read -r line; do
        local tag_name url version
        tag_name=$(echo "$line" | jq -r '.name')
        url=$(echo "$line" | jq -r '.tarball_url')
        
        # Extract version from tag name (remove release- prefix)
        version=$(echo "$tag_name" | sed 's/^release-//')
        
        if [[ "$version" == "$target_version" ]]; then
            found_url="$url"
            break
        fi
    done < <(echo "$releases" | jq -c '.[]')
    
    if [[ -n "$found_url" ]]; then
        info "Found exact match: release-$target_version"
        echo "$found_url"
        return 0
    fi
    
    # If no exact match, look for partial matches and suggest alternatives
    local suggestions=()
    while IFS= read -r line; do
        local tag_name
        tag_name=$(echo "$line" | jq -r '.name')
        version=$(echo "$tag_name" | sed 's/^release-//')
        
        # Check if version starts with target (e.g., 5.0 matches 5.0.4, 5.0.5)
        if [[ "$version" =~ ^${target_version} ]]; then
            suggestions+=("$version")
        fi
    done < <(echo "$releases" | jq -c '.[]')
    
    # Show suggestions if any found
    if [[ ${#suggestions[@]} -gt 0 ]]; then
        error "Version $target_version not found. Did you mean one of these?"
        for suggestion in "${suggestions[@]:0:5}"; do  # Show max 5 suggestions
            error "  - $suggestion"
        done
    else
        error "Version $target_version not found. Use --list-releases to see available versions."
    fi
    
    return 1
}

# Check if input looks like a direct tarball URL
is_tarball_url() {
    local url="$1"
    [[ "$url" =~ ^https://.*/(tarball|archive)/ ]]
}

# Extract version from tarball URL
extract_version_from_url() {
    local url="$1"
    # Extract version from URLs like:
    # https://api.github.com/repos/qbittorrent/qBittorrent/tarball/refs/tags/release-5.0.5
    # https://github.com/qbittorrent/qBittorrent/archive/refs/tags/release-5.0.5.tar.gz
    
    local version=""
    if [[ "$url" =~ /tarball/refs/tags/release-([0-9]+\.[0-9]+\.[0-9]+) ]]; then
        version="${BASH_REMATCH[1]}"
    elif [[ "$url" =~ /archive/refs/tags/release-([0-9]+\.[0-9]+\.[0-9]+) ]]; then
        version="${BASH_REMATCH[1]}"
    elif [[ "$url" =~ release-([0-9]+\.[0-9]+\.[0-9]+) ]]; then
        version="${BASH_REMATCH[1]}"
    fi
    
    echo "$version"
}

# Get releases from GitHub API
get_releases() {
    local api_url
    api_url=$(get_github_api_url)
    
    info "Fetching releases from GitHub API..."
    curl -s "$api_url" || error_exit "Failed to fetch releases from GitHub API"
}

# Check if a release tag is a stable release (not pre-release)
is_stable_release() {
    local tag_name="$1"
    
    # Check for pre-release keywords
    if [[ "$tag_name" =~ (alpha|beta|rc|dev|pre|test) ]]; then
        return 1
    fi
    
    # Check if it follows the release-X.Y.Z pattern
    if [[ ! "$tag_name" =~ ^release-[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        return 1
    fi
    
    return 0
}

# Compare version strings (e.g., "4.6.2" vs "4.5.1")
version_compare() {
    local ver1="$1"
    local ver2="$2"
    
    # Split versions into arrays
    IFS='.' read -ra v1_parts <<< "$ver1"
    IFS='.' read -ra v2_parts <<< "$ver2"
    
    # Compare each part
    local max_parts=${#v1_parts[@]}
    if [[ ${#v2_parts[@]} -gt $max_parts ]]; then
        max_parts=${#v2_parts[@]}
    fi
    
    for ((i=0; i<max_parts; i++)); do
        local part1="${v1_parts[i]:-0}"
        local part2="${v2_parts[i]:-0}"
        
        if [[ $part1 -gt $part2 ]]; then
            return 0  # ver1 > ver2
        elif [[ $part1 -lt $part2 ]]; then
            return 1  # ver1 < ver2
        fi
    done
    
    return 1  # ver1 == ver2 (return false for "greater than")
}

# Get the latest stable release URL
get_latest_stable_release_url() {
    local releases
    releases=$(get_releases)
    
    local latest_version=""
    local latest_url=""
    
    info "Looking for latest stable release..."
    
    # Parse releases and find the latest stable one
    while IFS= read -r line; do
        local tag_name url version
        tag_name=$(echo "$line" | jq -r '.name')
        url=$(echo "$line" | jq -r '.tarball_url')
        
        if is_stable_release "$tag_name"; then
            version=$(echo "$tag_name" | sed 's/^release-//')
            
            if [[ -z "$latest_version" ]] || version_compare "$version" "$latest_version"; then
                latest_version="$version"
                latest_url="$url"
            fi
        fi
    done < <(echo "$releases" | jq -c '.[]')
    
    if [[ -z "$latest_url" ]]; then
        error_exit "No stable releases found"
    fi
    
    info "Latest stable release: v$latest_version"
    echo "$latest_url"
}

# Get the latest release URL (including pre-releases)
get_latest_release_url() {
    local releases
    releases=$(get_releases)
    
    echo "$releases" | jq -r '.[0].tarball_url'
}

# List available releases
list_releases() {
    local releases
    releases=$(get_releases)
    
    echo "Available qBittorrent releases:"
    echo "================================"
    
    echo "$releases" | jq -r '.[] | "\(.name) - \(.tarball_url)"' | head -20
}

# Find release by major version
find_release_by_major_version() {
    local major_version="$1"
    local releases
    releases=$(get_releases)
    
    info "Looking for latest release in v$major_version.x series..."
    
    local found_version=""
    local found_url=""
    
    while IFS= read -r line; do
        local tag_name url version
        tag_name=$(echo "$line" | jq -r '.name')
        url=$(echo "$line" | jq -r '.tarball_url')
        
        if [[ "$tag_name" =~ ^release-${major_version}\.[0-9]+\.[0-9]+$ ]]; then
            if is_stable_release "$tag_name"; then
                version=$(echo "$tag_name" | sed 's/^release-//')
                
                if [[ -z "$found_version" ]] || version_compare "$version" "$found_version"; then
                    found_version="$version"
                    found_url="$url"
                fi
            fi
        fi
    done < <(echo "$releases" | jq -c '.[]')
    
    if [[ -z "$found_url" ]]; then
        error_exit "No stable releases found for v$major_version.x"
    fi
    
    info "Found: v$found_version"
    echo "$found_url"
}

# Download and extract source
download_and_extract() {
    local url="$1"
    local cache_key
    cache_key=$(echo "$url" | sha256sum | cut -d' ' -f1)
    local cache_file="$CACHE_DIR/$cache_key.tar.gz"
    
    # Create temp directory
    mkdir -p "$TEMP_DIR"
    
    # Check cache first
    if [[ "$USE_CACHE" == true ]] && [[ -f "$cache_file" ]] && [[ "$FORCE_DOWNLOAD" == false ]]; then
        info "Using cached download..."
        cp "$cache_file" "$TEMP_DIR/source.tar.gz"
    else
        info "Downloading source code from: $url"
        curl -L -o "$TEMP_DIR/source.tar.gz" "$url" || error_exit "Failed to download source"
        
        # Cache the download
        if [[ "$USE_CACHE" == true ]]; then
            cp "$TEMP_DIR/source.tar.gz" "$cache_file"
        fi
    fi
    
    info "Extracting source code..."
    cd "$TEMP_DIR"
    tar -xzf source.tar.gz || error_exit "Failed to extract source"
    
    # Find the extracted directory
    local source_dir
    source_dir=$(find . -maxdepth 1 -type d -name "qbittorrent-*" | head -1)
    
    if [[ -z "$source_dir" ]]; then
        error_exit "Could not find extracted source directory"
    fi
    
    echo "$TEMP_DIR/$source_dir"
}

# Helper functions from libtorrent_funcs.sh (integrated)
version_to_char() {
    local version="$1"
    printf "\\$(printf '%03o' "$version")"
}

libtorrent__compute_peer_id_prefix() {
    local major="$1"
    local minor="$2"
    local patch="$3"
    
    # qBittorrent uses decimal format: -qB + major + minor + patch + 0-
    # e.g., version 5.0.4 becomes -qB5040-
    echo "-qB${major}${minor}${patch}0-"
}

libtorrent_get_key_format() {
    local major="$1"
    local minor="$2"
    
    if [[ $major -gt 4 ]] || [[ $major -eq 4 && $minor -ge 4 ]]; then
        echo "hex"
    else
        echo "dec"
    fi
}

# Extract version information from source
extract_version_info() {
    local source_dir="$1"
    local version_file="$source_dir/src/base/version.h.in"
    
    if [[ ! -f "$version_file" ]]; then
        error_exit "Version file not found: $version_file"
    fi
    
    info "Extracting version information..."
    
    local major minor patch
    major=$(grep -E "^#define QBT_VERSION_MAJOR" "$version_file" | cut -d' ' -f3)
    minor=$(grep -E "^#define QBT_VERSION_MINOR" "$version_file" | cut -d' ' -f3)
    patch=$(grep -E "^#define QBT_VERSION_BUGFIX" "$version_file" | cut -d' ' -f3)
    
    if [[ -z "$major" ]] || [[ -z "$minor" ]] || [[ -z "$patch" ]]; then
        error_exit "Failed to extract version numbers from $version_file"
    fi
    
    echo "$major.$minor.$patch"
}

# Extract protocol information (user-agent and peer-id)
extract_protocol_info() {
    local source_dir="$1"
    local version="$2"
    
    # Try to find the session file - check for both possible locations
    local session_file=""
    if [[ -f "$source_dir/src/base/bittorrent/sessionimpl.cpp" ]]; then
        session_file="$source_dir/src/base/bittorrent/sessionimpl.cpp"
        info "Using session file: sessionimpl.cpp (v5.x+ format)"
    elif [[ -f "$source_dir/src/base/bittorrent/session.cpp" ]]; then
        session_file="$source_dir/src/base/bittorrent/session.cpp"
        info "Using session file: session.cpp (v4.x format)"
    else
        error_exit "Session file not found in either location:
  - $source_dir/src/base/bittorrent/sessionimpl.cpp (v5.x+)
  - $source_dir/src/base/bittorrent/session.cpp (v4.x)"
    fi
    
    info "Extracting protocol information from session file..."
    
    # Extract user-agent pattern
    local user_agent_line
    user_agent_line=$(grep -E "(QBT_USER_AGENT|USER_AGENT)" "$session_file" | head -1 | tr -d ' ')
    
    if [[ -z "$user_agent_line" ]]; then
        error_exit "Could not find user-agent definition in $session_file"
    fi
    
    # Extract peer-id pattern
    local peer_id_line
    peer_id_line=$(grep -E "PEER_ID" "$session_file" | head -1 | tr -d ' ')
    
    if [[ -z "$peer_id_line" ]]; then
        error_exit "Could not find peer-id definition in $session_file"
    fi
    
    # Parse version components
    local major minor patch
    IFS='.' read -ra version_parts <<< "$version"
    major="${version_parts[0]}"
    minor="${version_parts[1]}"
    patch="${version_parts[2]}"
    
    # Generate peer-id prefix
    local peer_id_prefix
    peer_id_prefix=$(libtorrent__compute_peer_id_prefix "$major" "$minor" "$patch")
    
    # Generate key format
    local key_format
    key_format=$(libtorrent_get_key_format "$major" "$minor")
    
    # Create result object using jq to properly escape JSON
    jq -n \
        --arg version "$version" \
        --arg user_agent_pattern "$user_agent_line" \
        --arg peer_id_pattern "$peer_id_line" \
        --arg peer_id_prefix "$peer_id_prefix" \
        --arg key_format "$key_format" \
        --arg session_file_used "$(basename "$session_file")" \
        '{
            version: $version,
            user_agent_pattern: $user_agent_pattern,
            peer_id_pattern: $peer_id_pattern,
            peer_id_prefix: $peer_id_prefix,
            key_format: $key_format,
            session_file_used: $session_file_used
        }'
}

# Generate complete client configuration file
generate_client_config() {
    local version="$1"
    local peer_id_prefix="$2"
    
    # Create the peer ID regex pattern (single escaping for final JSON)
    local peer_id_regex="${peer_id_prefix}[A-Za-z0-9_~\\(\\)\\!\\.\\*-]{12}"
    
    # Generate the complete client configuration
    jq -n \
        --arg version "$version" \
        --arg peer_id_regex "$peer_id_regex" \
        '{
            keyGenerator: {
                algorithm: {
                    type: "HASH_NO_LEADING_ZERO",
                    length: 8
                },
                refreshOn: "TORRENT_PERSISTENT",
                keyCase: "upper"
            },
            peerIdGenerator: {
                algorithm: {
                    type: "REGEX",
                    pattern: $peer_id_regex
                },
                refreshOn: "NEVER",
                shouldUrlEncode: false
            },
            urlEncoder: {
                encodingExclusionPattern: "[A-Za-z0-9_~\\(\\)\\!\\.\\*-]",
                encodedHexCase: "lower"
            },
            query: "info_hash={infohash}&peer_id={peerid}&port={port}&uploaded={uploaded}&downloaded={downloaded}&left={left}&corrupt=0&key={key}&event={event}&numwant={numwant}&compact=1&no_peer_id=1&supportcrypto=1&redundant=0",
            numwant: 200,
            numwantOnStop: 0,
            requestHeaders: [
                { name: "User-Agent", value: ("qBittorrent/" + $version) },
                { name: "Accept-Encoding", value: "gzip" },
                { name: "Connection", value: "close" }
            ]
        }'
}

# Format output
format_output() {
    local format="$1"
    local data="$2"
    
    case "$format" in
        "json")
            echo "$data" | jq '.'
            ;;
        "csv")
            echo "version,user_agent_pattern,peer_id_pattern,peer_id_prefix,key_format,session_file"
            echo "$data" | jq -r '[.version, .user_agent_pattern, .peer_id_pattern, .peer_id_prefix, .key_format, .session_file_used] | @csv'
            ;;
        "text"|*)
            echo "$data" | jq -r '
"qBittorrent Analysis Results:
=============================
Version: " + .version + "
User-Agent Pattern: " + .user_agent_pattern + "
Peer-ID Pattern: " + .peer_id_pattern + "
Peer-ID Prefix: " + .peer_id_prefix + "
Key Format: " + .key_format + "
Session File Used: " + .session_file_used + "
"'
            ;;
    esac
}

# Show usage information
usage() {
    cat << EOF
Usage: $0 [OPTIONS] [URL]

Analyze qBittorrent source code to extract client identification information.
Supports simple version selection (e.g., '5.0.4') or direct tarball URLs.

Options:
    -h, --help              Show this help message
    -v, --verbose           Enable verbose output
    -f, --force             Force re-download (ignore cache)
    -o, --output FORMAT     Output format: text, json, csv (default: text)
    --major-version NUM     Get latest stable release for major version (e.g., 4)
    --list-releases         List available releases and exit
    --force-latest          Use absolute latest release (including pre-releases)
    --no-cache              Disable caching
    --clear-cache           Clear download cache and exit

Arguments:
    URL|VERSION             GitHub API tarball URL, release API endpoint, or version number
                           Examples: 5.0.4, v5.0.4, release-5.0.4
                           If not provided, uses latest stable release

Examples:
    $0                                          # Analyze latest stable release
    $0 5.0.4                                    # Analyze specific version (simple!)
    $0 v5.0.4                                   # Also works with v prefix
    $0 release-5.0.4                            # Also works with release- prefix
    $0 --major-version 4                        # Latest v4.x stable release
    $0 --list-releases                          # Show available releases
    $0 --clear-cache                            # Clear download cache
    $0 --output json 5.0.4                      # Specific version with JSON output
    $0 https://api.github.com/repos/qbittorrent/qBittorrent/tarball/refs/tags/release-4.6.2

EOF
}

# Main function
main() {
    local url=""
    local list_releases_flag=false
    local force_latest=false
    local clear_cache_flag=false
    
    # Parse command line arguments
    while [[ $# -gt 0 ]]; do
        case $1 in
            -h|--help)
                usage
                exit 0
                ;;
            -v|--verbose)
                VERBOSE=true
                shift
                ;;
            -f|--force)
                FORCE_DOWNLOAD=true
                shift
                ;;
            -o|--output)
                OUTPUT_FORMAT="$2"
                shift 2
                ;;
            --major-version)
                MAJOR_VERSION="$2"
                shift 2
                ;;
            --list-releases)
                list_releases_flag=true
                shift
                ;;
            --force-latest)
                force_latest=true
                shift
                ;;
            --no-cache)
                USE_CACHE=false
                shift
                ;;
            --clear-cache)
                clear_cache_flag=true
                shift
                ;;
            -*)
                error_exit "Unknown option: $1"
                ;;
            *)
                url="$1"
                shift
                ;;
        esac
    done
    
    # Validate output format
    case "$OUTPUT_FORMAT" in
        text|json|csv) ;;
        *) error_exit "Invalid output format: $OUTPUT_FORMAT" ;;
    esac
    
    # Check dependencies
    check_dependencies
    
    # Setup cache
    if [[ "$USE_CACHE" == true ]]; then
        setup_cache
    fi
    
    info "Starting qBittorrent analysis..."
    
    # Handle list releases
    if [[ "$list_releases_flag" == true ]]; then
        list_releases
        exit 0
    fi
    
    # Handle clear cache
    if [[ "$clear_cache_flag" == true ]]; then
        clear_cache
        exit 0
    fi
    
    # Determine URL to analyze
    if [[ -n "$url" ]]; then
        # Check what type of input the user provided
        if is_tarball_url "$url"; then
            # User provided a direct tarball URL
            info "Using provided tarball URL"
        elif is_version_number "$url"; then
            # User provided a version number (e.g., "5.0.4")
            info "Looking up version: $url"
            url=$(find_release_by_version "$url")
            if [[ $? -ne 0 ]]; then
                exit 1  # Error already shown by find_release_by_version
            fi
        else
            error_exit "Invalid input format. Expected:
  - Version number (e.g., 5.0.4, v5.0.4, release-5.0.4)
  - Tarball URL (e.g., https://api.github.com/.../tarball/refs/tags/release-X.Y.Z)
  
Use --list-releases to see available versions."
        fi
    elif [[ -n "$MAJOR_VERSION" ]]; then
        # Find latest release for specific major version
        url=$(find_release_by_major_version "$MAJOR_VERSION")
    elif [[ "$force_latest" == true ]]; then
        # Get absolute latest (including pre-releases)
        url=$(get_latest_release_url)
    else
        # Get latest stable release (default)
        url=$(get_latest_stable_release_url)
    fi
    
    # Extract version from URL if it's a direct tarball URL
    local extracted_version=""
    if is_tarball_url "$url"; then
        extracted_version=$(extract_version_from_url "$url")
        if [[ -n "$extracted_version" ]]; then
            info "Detected version from URL: $extracted_version"
        fi
    fi
    
    # Download and extract source
    local source_dir
    source_dir=$(download_and_extract "$url")
    
    # Extract version information
    local version
    version=$(extract_version_info "$source_dir")
    
    # Validate extracted version against URL version if available
    if [[ -n "$extracted_version" ]] && [[ "$version" != "$extracted_version" ]]; then
        warn "Version mismatch: URL suggests $extracted_version, source contains $version"
    fi
    
    # Extract protocol information
    local result
    result=$(extract_protocol_info "$source_dir" "$version")
    
    # Extract peer_id_prefix from the result for client config generation
    local peer_id_prefix
    peer_id_prefix=$(echo "$result" | jq -r '.peer_id_prefix')
    
    # Generate client configuration file
    local client_filename="qbittorrent-${version}.client"
    info "Generating client configuration file: $client_filename"
    
    generate_client_config "$version" "$peer_id_prefix" > "$client_filename"
    
    # Format and display output
    format_output "$OUTPUT_FORMAT" "$result"
    
    info "Client configuration saved to: $client_filename"
    info "Analysis complete!"
}

# Run main function with all arguments
main "$@"
