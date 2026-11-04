#!/usr/bin/env bash
# kept-fixtures.sh: fixtures builds kept as release assets, so a run need not
# make them again and they do not expire as a CI artifact does (#386).
#
#   kept-fixtures.sh select                  print kept=, tag= and sha256= for $GITHUB_OUTPUT
#   kept-fixtures.sh fetch TAG SHA256 DIR    DIR/fixtures.tar.gz, refused unless its digest is SHA256
#   kept-fixtures.sh publish TARBALL RUN     keep TARBALL as this tree's build, made by CI run RUN
#   kept-fixtures.sh promote COMMIT          keep the fixtures of the merge queue run that tested COMMIT
#
# One release per fixture key (scripts/fixture-key.sh), tagged fixtures-<key>:
# a prerelease, never "latest", holding fixtures.tar.gz and the manifest of the
# key. The SHA-256 is GitHub's digest of the uploaded asset and is written in the
# release's notes. A release is never replaced: a key has one build.
#
# select answers kept=false, so the run builds, when:
#   - FRESH is true: a tag, a release or the scheduled fresh build;
#   - no release holds this tree's key: an input changed and has not landed.
#
# GH overrides the gh binary, so the guard can feed it a fake repository.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
gh="${GH:-gh}"
repo="${GITHUB_REPOSITORY:-antimatter-studios/rust-fs-bcachefs}"
# How often, and how far apart, to ask again: for an API that did not answer,
# and for a landed commit's queue run to read as finished.
tries="${KEPT_FIXTURES_TRIES:-5}"
pause="${KEPT_FIXTURES_PAUSE:-30}"

die() {
    echo "kept-fixtures: $*" >&2
    exit 1
}

sha256_of() {
    if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1
}

tag_of() {
    local key
    key="$(bash "$here/fixture-key.sh" "${1:-HEAD}")"
    echo "fixtures-${key:0:16}"
}

# The digest of TAG's fixtures.tar.gz; nothing when no release has that tag. Any
# other failure is tried again and then stops the run: a miss builds for 20
# minutes, so it is never guessed from an API that did not answer.
digest_of() {
    local err digest status attempt=0
    err="$(mktemp)"
    while :; do
        status=0
        digest="$("$gh" api "repos/$repo/releases/tags/$1" \
            --jq '.assets[] | select(.name == "fixtures.tar.gz") | .digest' 2>"$err")" || status=$?
        if [ "$status" = 0 ]; then
            rm -f "$err"
            printf '%s\n' "${digest#sha256:}"
            return 0
        fi
        if grep -q 'HTTP 404' "$err"; then
            rm -f "$err"
            return 0
        fi
        attempt=$((attempt + 1))
        [ "$attempt" -lt "$tries" ] || break
        sleep "$pause"
    done
    cat "$err" >&2
    rm -f "$err"
    die "cannot read release $1 (status $status)"
}

select_build() {
    local tag digest
    if [ "${FRESH:-false}" = true ]; then
        echo "kept-fixtures: this run asks for a fresh build" >&2
        echo "kept=false"
        return
    fi
    tag="$(tag_of)"
    digest="$(digest_of "$tag")"
    if [ -z "$digest" ]; then
        echo "kept-fixtures: no release $tag, so this run builds: $(bash "$here/fixture-key.sh")" >&2
        echo "kept=false"
        return
    fi
    echo "kept-fixtures: $tag holds this tree's fixtures, sha256:$digest" >&2
    echo "kept=true"
    echo "tag=$tag"
    echo "sha256=$digest"
}

fetch() {
    local tag="$1" want="$2" dir="$3" got attempt=0
    [ -n "$tag" ] && [ -n "$want" ] && [ -n "$dir" ] || die "fetch needs a tag, a sha256 and a directory"
    mkdir -p "$dir"
    until "$gh" release download "$tag" --repo "$repo" --pattern fixtures.tar.gz --dir "$dir" --clobber; do
        attempt=$((attempt + 1))
        [ "$attempt" -lt "$tries" ] || die "cannot download fixtures.tar.gz of $tag"
        sleep "$pause"
    done
    got="$(sha256_of "$dir/fixtures.tar.gz")"
    if [ "$got" != "$want" ]; then
        rm -f "$dir/fixtures.tar.gz"
        die "fixtures.tar.gz of $tag is sha256:$got, not sha256:$want; no fallback"
    fi
    echo "kept-fixtures: $dir/fixtures.tar.gz from $tag, sha256:$got" >&2
}

publish() {
    local tarball="$1" run="$2" commit tag key digest work status=0 err
    [ -s "$tarball" ] || die "$tarball is empty or missing"
    [ -n "$run" ] || die "publish needs the CI run that made the build"
    commit="$(git -C "$here/.." rev-parse HEAD)"
    tag="$(tag_of)"
    key="$(bash "$here/fixture-key.sh")"
    if [ -n "$(digest_of "$tag")" ]; then
        echo "kept-fixtures: $tag already holds a build; a key has one" >&2
        return 0
    fi
    digest="$(sha256_of "$tarball")"
    work="$(mktemp -d)"
    bash "$here/fixture-key.sh" --manifest >"$work/fixture-key-manifest.txt"
    # The asset takes its file's name, whatever the tarball was called here.
    ln "$tarball" "$work/fixtures.tar.gz" 2>/dev/null || cp "$tarball" "$work/fixtures.tar.gz"
    {
        echo "Fixtures kept so that merge queue runs need not build them. Test data only, not a software release."
        echo
        echo "- Fixture key: \`$key\` (\`scripts/fixture-key.sh\`; \`fixture-key-manifest.txt\` is what it hashes)."
        echo "- \`fixtures.tar.gz\` SHA-256: \`$digest\`."
        echo "- Made by CI run $run, at commit $commit."
    } >"$work/notes.md"
    err="$work/err"
    # --latest=false and --prerelease: the crate's release stays what `latest` answers.
    "$gh" release create "$tag" --repo "$repo" --target "$commit" --prerelease --latest=false \
        --title "Kept fixtures $tag" --notes-file "$work/notes.md" \
        "$work/fixtures.tar.gz" "$work/fixture-key-manifest.txt" 2>"$err" || status=$?
    if [ "$status" != 0 ]; then
        # Two landings of one key can race; whichever published holds the build.
        cat "$err" >&2
        rm -rf "$work"
        [ -n "$(digest_of "$tag")" ] || die "cannot create release $tag"
        echo "kept-fixtures: $tag was published by another run" >&2
        return 0
    fi
    rm -rf "$work"
    echo "kept-fixtures: kept $tag, sha256:$digest, from CI run $run" >&2
}

# The queue tested exactly this commit, and landed it because that run was green.
# So a landed key is kept from what was tested, rather than built again and
# hoped equal. A commit the queue landed on a later entry's verdict has no run
# of its own: the next commit with this key keeps the build, and the groups
# before that build as they would anyway, so this only warns.
promote() {
    local commit="$1" tag run="" attempt=0 dir
    [ -n "$commit" ] || die "promote needs a commit"
    tag="$(tag_of)"
    if [ -n "$(digest_of "$tag")" ]; then
        echo "kept-fixtures: $tag is already kept; nothing to promote" >&2
        return 0
    fi
    while :; do
        run="$("$gh" run list --repo "$repo" --workflow ci.yml --event merge_group --commit "$commit" \
            --status success --limit 1 --json databaseId --jq '.[0].databaseId // empty')"
        [ -z "$run" ] || break
        attempt=$((attempt + 1))
        [ "$attempt" -lt "$tries" ] || break
        sleep "$pause"
    done
    if [ -z "$run" ]; then
        echo "::warning::commit $commit has no green merge queue run, so $tag is not kept yet; the next landing with this key keeps it"
        return 0
    fi
    dir="$(mktemp -d)"
    "$gh" run download "$run" --repo "$repo" --name fixtures --dir "$dir" ||
        die "CI run $run tested $commit and holds no fixtures artifact"
    publish "$dir/fixtures.tar.gz" "$run"
    rm -rf "$dir"
}

case "${1:-}" in
    select) select_build ;;
    fetch) fetch "${2:-}" "${3:-}" "${4:-}" ;;
    publish) publish "${2:-}" "${3:-}" ;;
    promote) promote "${2:-}" ;;
    *) die "usage: kept-fixtures.sh select | fetch TAG SHA256 DIR | publish TARBALL RUN | promote COMMIT" ;;
esac
