# 08-09-sync-rework data repair: reset sync state (ADR-028 12.5)
#
# Background: remote rev22 is all-empty, local base stuck at rev21; the next
# successful sync would tombstone the remaining local data. This script resets
# ONE device side only:
#   1) backup database.db to <dir>/sync-reset-backup/;
#   2) clear sync_base / sync_meta / sync_pending_apply (keep sync_devices id).
# The remote WebDAV dir and the other device must be handled manually.
#
# Usage: close RCH app (or at least disable auto-sync) before running.
# NOTE: keep this file ASCII-only (Windows PowerShell 5.1 parses ANSI).
$ErrorActionPreference = 'Stop'

$db = 'D:\Documents\RCH\database.db'
if (!(Test-Path -LiteralPath $db)) {
    throw "database.db not found: $db (edit `$db at the top if the data dir differs)"
}

$bakDir = Join-Path (Split-Path -Parent $db) 'sync-reset-backup'
New-Item -ItemType Directory -Force -Path $bakDir | Out-Null
$bak = Join-Path $bakDir ("database-{0:yyyyMMdd-HHmmss}.db" -f (Get-Date))
Copy-Item -LiteralPath $db -Destination $bak
Write-Host "Backup created: $bak"

if (Get-Command sqlite3 -ErrorAction SilentlyContinue) {
    "DELETE FROM sync_base; DELETE FROM sync_meta; DELETE FROM sync_pending_apply;" |
        sqlite3 $db
} else {
    @"
import sqlite3
con = sqlite3.connect(r"$db")
con.executescript("DELETE FROM sync_base; DELETE FROM sync_meta; DELETE FROM sync_pending_apply;")
con.commit()
con.close()
"@ | python -
}

Write-Host 'Local sync state reset (device identity in sync_devices kept).'
Write-Host ''
Write-Host 'Next mandatory steps (otherwise the old all-empty rev22 is still treated as "remote deleted everything"):'
Write-Host '  1) empty/rename the remote WebDAV dir RCH-sync (manifest.json, state/, devices/ all removed);'
Write-Host '  2) run this script on the other device too (change $db to its database.db path);'
Write-Host '  3) keep auto-sync OFF on both devices; manually sync the device with the most data first;'
Write-Host '  4) local sources are re-scanned automatically before sync (rootHash short-circuit fixed).'
