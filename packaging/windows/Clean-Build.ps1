# Remove bulky Cargo build caches (debug alone is often several GB).
# Does NOT delete source. Safe to re-run; next build will recompile.
$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$Target = Join-Path $Root "target"

if (-not (Test-Path $Target)) {
  Write-Host "Nothing to clean: target\ missing"
  exit 0
}

$before = (Get-ChildItem $Target -Recurse -File -ErrorAction SilentlyContinue |
  Measure-Object Length -Sum).Sum

# Keep packaged folders if present; wipe compile caches.
foreach ($name in @("debug", "release", "tmp", "x86_64-unknown-linux-gnu")) {
  $p = Join-Path $Target $name
  if (Test-Path $p) {
    Write-Host "Removing $p ..."
    Remove-Item -Recurse -Force $p
  }
}

# Optional smoke/test leftovers
foreach ($name in @("setup-smoke", "test-archive", "e2e-archive")) {
  $p = Join-Path $Target $name
  if (Test-Path $p) { Remove-Item -Recurse -Force $p }
}

$after = 0
if (Test-Path $Target) {
  $after = (Get-ChildItem $Target -Recurse -File -ErrorAction SilentlyContinue |
    Measure-Object Length -Sum).Sum
}

Write-Host ("Freed ~{0:N1} MB (was {1:N1} MB under target\)" -f (($before - $after) / 1MB), ($before / 1MB))
Write-Host "Kept: target\OfficePC / OfficeRecorder if they existed."
Write-Host "Rebuild: cargo build --release   then pack-office-pc.ps1"
