#!/usr/bin/env bash
# Public-release audit (blueprint only). Run before publishing or pushing a public tag:
#   scripts/release-audit.sh            # audit; launch placeholders are warnings
#   scripts/release-audit.sh --final    # placeholders (OWNER/REPO, contacts) become failures
# Checks the working tree AND the full git history. Exit 1 on any FAIL.
set -uo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
final=false; [ "${1:-}" = "--final" ] && final=true
fails=0; warns=0
pass() { printf '  PASS  %s\n' "$*"; }
warn() { printf '  WARN  %s\n' "$*"; warns=$((warns + 1)); }
fail() { printf '  FAIL  %s\n' "$*"; fails=$((fails + 1)); }

echo "public-release audit of $(git rev-parse --short HEAD) ($(git describe --tags --always))"

# 1. Secrets: full history and working tree.
if command -v gitleaks >/dev/null; then
  gitleaks git --no-banner --redact --exit-code 1 . >/dev/null 2>&1 && pass "gitleaks: no secrets in git history" || fail "gitleaks found secrets in git history (gitleaks git --redact .)"
  gitleaks dir --no-banner --redact --exit-code 1 . >/dev/null 2>&1 && pass "gitleaks: no secrets in the working tree" || warn "gitleaks flags the working tree (untracked files included); inspect: gitleaks dir --redact ."
else
  fail "gitleaks not installed"
fi

# 2. Machine- and person-identifying strings in tracked files and in every historical diff.
user=$(id -un); host=$(hostname -s 2>/dev/null || hostname)
patterns=("$HOME" "/Users/$user" "/home/$user")
[ ${#user} -ge 4 ] && patterns+=("$user")
[ ${#host} -ge 4 ] && [ "$host" != "localhost" ] && patterns+=("$host")
for p in "${patterns[@]}"; do
  hits=$(git ls-files -z | xargs -0 grep -IlF -- "$p" 2>/dev/null | head -5)
  [ -z "$hits" ] && pass "tracked files: no '$p'" || fail "tracked files contain '$p': $(echo "$hits" | tr '\n' ' ')"
  n=$(git log -p --all 2>/dev/null | grep -cF -- "$p")
  [ "$n" = 0 ] && pass "history diffs: no '$p'" || fail "git history contains '$p' in $n diff lines (rewrite history before publishing)"
done

# 3. Identities (commit author/committer and annotated-tag tagger) become public with the history.
idents=$( { git log --all --format='%an <%ae>%n%cn <%ce>'
            git for-each-ref --format='%(taggername) %(taggeremail)' refs/tags | grep -v '^ *$'; } | sort -u)
echo "$idents" | while read -r line; do echo "        identity in history: $line"; done
if echo "$idents" | grep -qiE '\.(local|lan|home|mymodem)>|@[a-z0-9-]+\.(local|lan|home)'; then
  fail "history identities include a machine hostname email; rewrite authorship (launch: rewrite-author.sh) or confirm"
else
  warn "confirm the identities above are the ones you want public"
fi

# 4. Files that should never be public.
bad=$(git ls-files | grep -E '(^|/)\.env($|\.)|\.(pem|key|p12|pfx|sqlite|db|log)$|id_(rsa|ed25519)' | grep -v '^\.env\.example$')
[ -z "$bad" ] && pass "no env files, keys, databases or logs tracked" || fail "tracked sensitive-looking files: $bad"
big=$(git ls-files -z | xargs -0 -I{} sh -c 'test $(wc -c < "{}") -gt 1048576 && echo "{}"' 2>/dev/null)
[ -z "$big" ] && pass "no tracked file over 1 MiB" || warn "large tracked files: $big"
img=$(git ls-files | grep -iE '\.(png|jpe?g|gif|webp|mp4)$')
[ -z "$img" ] && pass "no tracked screenshots/media (review any you add for personal data)" || warn "tracked media to review for personal data: $img"

# 5. Launch placeholders that the owner must replace.
ph=$(git ls-files -z | xargs -0 grep -IlE 'OWNER/REPO|CONDUCT_CONTACT|SECURITY_CONTACT|sponsors/OWNER' 2>/dev/null | grep -v '^scripts/release-audit.sh$' | tr '\n' ' ')
if [ -n "$ph" ]; then
  if $final; then fail "launch placeholders remain in: $ph"; else warn "launch placeholders remain (owner actions) in: $ph"; fi
else
  pass "no launch placeholders"
fi
grep -qE '^[[:space:]]*(github|buy_me_a_coffee|custom):' .github/FUNDING.yml 2>/dev/null \
  && pass "FUNDING.yml configured" || { $final && warn "FUNDING.yml has no active entries (sponsor button hidden)" || warn "FUNDING.yml not configured yet (owner action)"; }

echo "release audit: $([ $fails = 0 ] && echo OK || echo FAILED) — $fails fail, $warns warn"
[ $fails = 0 ]
