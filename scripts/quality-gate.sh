#!/bin/bash
# Runs the seven-check quality gate from AGENTS.md, timing every step and
# sampling machine state while it runs, so a slow local run leaves evidence of
# why it was slow instead of having to be re-diagnosed from scratch.
#
# Usage: scripts/quality-gate.sh [step ...]
#   steps: fmt clippy nextest node lint typos deny (default: all, in that order)
# Logs:  $CARGO_TARGET_DIR (or ./target)/gate-logs/<timestamp>/
# GATE_SLOW_SECS (default 600) sets when a step is reported as abnormally slow.
set -u
cd "$(dirname "$0")/.." || exit 1

TARGET=${CARGO_TARGET_DIR:-$PWD/target}
LOG_DIR=$TARGET/gate-logs/$(date +%Y%m%d-%H%M%S)
SLOW_SECS=${GATE_SLOW_SECS:-600}
mkdir -p "$LOG_DIR"

if [ $# -eq 0 ]; then
  set -- fmt clippy nextest node lint typos deny
fi

step_command() {
  case $1 in
    fmt) CMD=(cargo fmt --all -- --check) ;;
    clippy) CMD=(cargo clippy --workspace --all-targets --locked -- -D warnings) ;;
    nextest) CMD=(cargo nextest run --workspace --locked --final-status-level slow) ;;
    node) CMD=(node --check crates/desktop-shell/static/app.js) ;;
    lint) CMD=(npm run lint:web) ;;
    typos) CMD=(typos) ;;
    deny) CMD=(cargo deny check) ;;
    *) return 1 ;;
  esac
}

for step in "$@"; do
  step_command "$step" || { echo "unknown step: $step" >&2; exit 2; }
done

# Machine state every 30s. Local runs have ranged from CI speed to over an hour
# on the same code, so the cause is what else the Mac is doing at the time:
# concurrent cargo builds from other worktrees, swap, power source.
sample() {
  echo "--- $(date +%H:%M:%S) step=$(cat "$LOG_DIR/current" 2>/dev/null)"
  uptime | sed 's/.*load/load/'
  sysctl -n vm.swapusage
  memory_pressure 2>/dev/null | tail -1
  pmset -g batt | head -1
  echo "cargo/rustc/nextest processes (all worktrees): $(pgrep -f 'rustc|cargo|nextest' | wc -l | tr -d ' ')"
  ps -axo pid,pcpu,rss,comm -r | head -8
}

(
  warned=""
  while true; do
    sample >> "$LOG_DIR/sampler.log"
    name="" started=""
    read -r name started 2>/dev/null < "$LOG_DIR/current"
    if [ -n "$started" ] && [ "$warned" != "$name" ] &&
      [ $(($(date +%s) - started)) -ge "$SLOW_SECS" ]; then
      warned=$name
      {
        echo
        echo "!! gate step '$name' has run for over ${SLOW_SECS}s; a quiet Mac"
        echo "!! runs the whole gate in about 3. Find out why before waiting longer:"
        echo "!!   tail -40 $LOG_DIR/sampler.log   (other cargo builds? swap? battery?)"
        echo "!!   cpu/wall well below 1 means waiting: check for stalled launches with"
        echo "!!   scripts/time-git-calls.sh (see the doc below)"
        echo "!!   sample <pid of the slowest process> 5   (where is it blocked?)"
        echo "!! See docs/TROUBLESHOOTING.md, 'The quality gate is slow locally'."
      } >&2
    fi
    sleep 30
  done
) &
SAMPLER=$!
trap 'kill $SAMPLER 2>/dev/null' EXIT

echo "gate logs: $LOG_DIR"
sample > "$LOG_DIR/sampler.log"
printf '%-8s %5s %8s %8s %8s %6s\n' step exit real user sys cpu/wall > "$LOG_DIR/summary.txt"
failed=0

for step in "$@"; do
  step_command "$step"
  echo "$step $(date +%s)" > "$LOG_DIR/current"
  printf '%-8s ... ' "$step"
  /usr/bin/time -p -o "$LOG_DIR/$step.time" "${CMD[@]}" > "$LOG_DIR/$step.log" 2>&1
  rc=$?
  real=$(awk '/^real/ {print $2}' "$LOG_DIR/$step.time")
  user=$(awk '/^user/ {print $2}' "$LOG_DIR/$step.time")
  sys=$(awk '/^sys/ {print $2}' "$LOG_DIR/$step.time")
  # Well below 1 means the step was mostly waiting (I/O, subprocesses, locks,
  # swap) rather than computing; build caching cannot help a waiting step.
  ratio=$(awk -v r="$real" -v u="$user" -v s="$sys" 'BEGIN { if (r > 0) printf "%.1f", (u + s) / r; else print "-" }')
  line=$(printf '%-8s %5s %8s %8s %8s %6s' "$step" "$rc" "$real" "$user" "$sys" "$ratio")
  echo "$line" >> "$LOG_DIR/summary.txt"
  if [ "$rc" -eq 0 ]; then
    echo "ok (${real}s)"
  else
    failed=1
    echo "FAILED (${real}s, exit $rc)"
    tail -30 "$LOG_DIR/$step.log"
  fi
done

rm -f "$LOG_DIR/current"
echo
cat "$LOG_DIR/summary.txt"
echo "logs: $LOG_DIR"
exit $failed
