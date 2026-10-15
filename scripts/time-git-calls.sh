#!/bin/bash
# Runs a command with every `git` it starts timed, then prints how the call
# times are distributed. Local suites have run 5-10x slower than CI because a
# share of process launches stall for a fixed ~280 ms (most likely endpoint
# security software); that shows up here as a second cluster of slow calls.
#
# Usage: scripts/time-git-calls.sh <command ...>
# e.g.   scripts/time-git-calls.sh cargo nextest run -p workspace-engine --test checkpoints \
#          -E 'test(a_hundred_turn_session_stays_within_its_documented_storage_bound)'
set -u
[ $# -gt 0 ] || { sed -n '2,10p' "$0"; exit 2; }

real_git=$(command -v git) || { echo "git not found" >&2; exit 1; }
work=$(mktemp -d)
log=$work/calls.log
trap 'rm -rf "$work"' EXIT

mkdir "$work/bin"
cat > "$work/bin/git" <<EOF
#!/usr/bin/perl
use Time::HiRes qw(time);
my \$t = time;
my \$rc = system('$real_git', @ARGV);
my \$ms = (time - \$t) * 1000;
my (\$sub) = grep { !/^-/ && !m{/} } @ARGV;
open(my \$f, '>>', '$log');
printf \$f "%.1f %s\n", \$ms, \$sub // '';
close \$f;
exit(\$rc >> 8);
EOF
chmod +x "$work/bin/git"

PATH="$work/bin:$PATH" "$@"
rc=$?

echo
if [ ! -s "$log" ]; then
  echo "no git calls recorded"
  exit $rc
fi
sort -n "$log" | awk '
  { ms[NR] = $1; total += $1; if ($1 >= 150) { slow++; slow_total += $1 } }
  END {
    printf "git calls: %d, total %.1fs\n", NR, total / 1000
    printf "p50 %.0f ms, p90 %.0f ms, p99 %.0f ms, max %.0f ms\n",
      ms[int(NR * .5) + 1], ms[int(NR * .9) + 1], ms[int(NR * .99) + 1], ms[NR]
    printf ">=150 ms: %d calls (%.0f%%), %.0f%% of git time\n",
      slow, 100 * slow / NR, (total > 0 ? 100 * slow_total / total : 0)
  }'
exit $rc
