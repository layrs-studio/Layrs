#!/usr/bin/env bash
# One verified filesystem-CAS snapshot. No pruning, installation or scheduler changes.
set -euo pipefail
umask 077
backup_root=${LAYRS_BACKUP_ROOT:?Set an absolute private backup directory}
server_binary=${LAYRS_SERVER_EXECUTABLE:-/usr/local/bin/layrs-server}
[[ "$backup_root" == /* && "$backup_root" != / && ! -L "$backup_root" ]] || { echo 'Invalid backup directory' >&2; exit 64; }
[[ "$server_binary" == /* && -x "$server_binary" ]] || { echo 'Invalid absolute server executable' >&2; exit 64; }
mkdir -p -- "$backup_root"
exec 9>"$backup_root/.scheduled-backup.lock"
flock -n 9 || { echo '{"event":"backup_busy"}' >&2; exit 75; }
backup_id="backup-$(date -u +%Y%m%dT%H%M%S%NZ)-$$"
backup_target="$backup_root/$backup_id"
backup_log="$backup_root/$backup_id.log"
started_unix=$(date +%s)
finish() {
  result=$?
  trap - EXIT
  set +e
  finished_unix=$(date +%s)
  last_success=0
  if [[ -f "$backup_root/last-success-unix" ]]; then read -r last_success < "$backup_root/last-success-unix"; fi
  [[ "$last_success" =~ ^[0-9]+$ ]] || last_success=0
  state=failed
  success=0
  if [[ $result == 0 ]]; then
    state=verified
    success=1
    last_success=$finished_unix
    printf '%s\n' "$last_success" > "$backup_root/.last-success-unix.$$"
    mv -T -- "$backup_root/.last-success-unix.$$" "$backup_root/last-success-unix"
  fi
  printf '{"event":"scheduled_backup","state":"%s","backup":"%s","startedUnix":%s,"finishedUnix":%s,"durationSeconds":%s,"lastSuccessUnix":%s}\n' "$state" "$backup_id" "$started_unix" "$finished_unix" "$((finished_unix-started_unix))" "$last_success" > "$backup_root/.last-backup.$$"
  mv -T -- "$backup_root/.last-backup.$$" "$backup_root/last-backup.json"
  printf 'layrs_backup_last_attempt_success %s\nlayrs_backup_last_success_unixtime %s\nlayrs_backup_duration_seconds %s\n' "$success" "$last_success" "$((finished_unix-started_unix))" > "$backup_root/.backup-metrics.$$"
  mv -T -- "$backup_root/.backup-metrics.$$" "$backup_root/backup.prom"
  cat "$backup_root/last-backup.json"
  exit "$result"
}
trap finish EXIT
"$server_binary" backup "$backup_target" > "$backup_log" 2>&1
"$server_binary" verify-backup "$backup_target" >> "$backup_log" 2>&1
