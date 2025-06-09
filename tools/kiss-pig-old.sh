#!/bin/sh -e
#
# (c) Kiëd Llaentenn <kiedtl@tilde.team>
# See the COPYING file for copyright information.

log_no_nl() {
    fmt="$1"; shift
    # Intended.
    # shellcheck disable=SC2059
    printf "\x1b[34;1m[loap]\x1b[m $fmt" "$@" >&2
}

log() {
    log_no_nl "$@"
    printf '\n' >&2
}

checked_curl() {
    res="$(curl -sSLw '\n%{http_code}' "$@" || {
        log 'Curl failed (exit code %d)' "$?"
        exit 1
    })"

    # Shellcheck thinks $q/$d are variables, and complains.
    # shellcheck disable=SC2016
    code="$(printf '%s' "$res" | sed '$q;d')"
    # shellcheck disable=SC2016
    data="$(printf '%s' "$res" | sed '$d')"

    [ "$code" -ge 400 ] && {
        log 'Curl failed (code %d)' "$code"
        log 'Response: %s\n' "$data"
        exit 1
    }

    printf '%s' "$data"
}

fmt_size() {
    s="$1"
    (
        if   [ "$s" -ge 1000000000 ]; then printf 'scale=3; %d/1000000000; print "GB"' "$s";
        elif [ "$s" -ge 1000000    ]; then printf 'scale=2; %d/1000000; print "MB"' "$s";
        elif [ "$s" -ge 1000       ]; then printf 'scale=1; %d/1000; print "KB"' "$s";
        else printf '%d; print "bytes"' "$s";
        fi
    ) | bc | tr '\n' ' '
    printf '\n'
}

package="$1"

[ -z "$package" ] && {
    log "Usage: kiss pig <package>"
    exit 1
}

package_folder="$({
    kiss search "$package" || {
        log "Couldn't find package."
        exit 1
    }
} | head -n1)"

cd "$package_folder"
version="$(tr ' ' - < version)"

log 'Looking for builds of %s@%s' "$package" "$version"

build="$(checked_curl "https://loap.k1sslinux.org/api/b/ls?p=$package" | \
    jq "first(.[] | select(.version == \"$version\"))")"

[ -z "$build" ] && {
    log "No builds found."
    exit 1
}

b_id="$(printf '%s' "$build" | jq '.id')"
b_size="$(printf '%s' "$build" | jq '.size')"
b_builder="$(printf '%s' "$build" | jq -r '.builder_name')"

log 'Downloading \x1b[1m%s\x1b[m@\x1b[1m%s\x1b[m (size %s) built by \x1b[33m%s\x1b[m.' \
    "$package" "$version" "$(fmt_size "$b_size")" "$b_builder"

response="$(checked_curl -i "https://loap.k1sslinux.org/api/b/dl?id=$b_id" )"
url_line="$(printf '%s' "$response" | grep 'Location:' || {
    log "Couldn't parse download link."
    exit 1
})"
url="$(printf '%s' "$url_line" | cut -d' ' -f2 | tr -d '\n\r')"

curl -#SL "$url" -o "$HOME/.cache/kiss/bin/$package@$version.tar.xz"
