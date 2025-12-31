param(
  [string]$Version = "latest",
  [string]$InstallDir = "$env:LOCALAPPDATA\Programs\ts-bridge\bin",
  [switch]$NoVerify
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoOwner = "chojs23"
$repoName = "ts-bridge"
$releaseBase = "https://github.com/$repoOwner/$repoName/releases"
$archive = "ts-bridge-windows-x86_64.zip"

if ($Version -ne "latest" -and -not $Version.StartsWith("v")) {
  $Version = "v$Version"
}

if ($Version -eq "latest") {
  $archiveUrl = "$releaseBase/latest/download/$archive"
  $checksumUrl = "$releaseBase/latest/download/SHA256SUMS"
} else {
  $archiveUrl = "$releaseBase/download/$Version/$archive"
  $checksumUrl = "$releaseBase/download/$Version/SHA256SUMS"
}

$tempDir = Join-Path $env:TEMP ("ts-bridge-install-" + [guid]::NewGuid().ToString("N"))
$extractDir = Join-Path $tempDir "extract"
$archivePath = Join-Path $tempDir $archive
$checksumPath = Join-Path $tempDir "SHA256SUMS"

New-Item -ItemType Directory -Path $tempDir | Out-Null

try {
  Invoke-WebRequest -Uri $archiveUrl -OutFile $archivePath

  if (-not $NoVerify) {
    $checksumDownloaded = $false
    try {
      Invoke-WebRequest -Uri $checksumUrl -OutFile $checksumPath
      $checksumDownloaded = $true
    } catch {
      Write-Warning "Unable to download SHA256SUMS; skipping checksum verification."
    }

    if ($checksumDownloaded) {
      $expected = $null
      foreach ($line in Get-Content -Path $checksumPath) {
        if ($line -match "^(?<hash>[a-fA-F0-9]{64})\s+$archive$") {
          $expected = $Matches.hash.ToLower()
          break
        }
      }
      if (-not $expected) {
        throw "Checksum entry for $archive not found."
      }
      $actual = (Get-FileHash -Path $archivePath -Algorithm SHA256).Hash.ToLower()
      if ($actual -ne $expected) {
        throw "Checksum verification failed for $archive."
      }
    }
  }

  New-Item -ItemType Directory -Path $extractDir | Out-Null
  Expand-Archive -Path $archivePath -DestinationPath $extractDir -Force

  $binaryPath = Join-Path $extractDir "ts-bridge.exe"
  if (-not (Test-Path $binaryPath)) {
    throw "Expected binary not found at $binaryPath."
  }

  New-Item -ItemType Directory -Path $InstallDir | Out-Null
  Copy-Item -Path $binaryPath -Destination (Join-Path $InstallDir "ts-bridge.exe") -Force

  Write-Host "Installed ts-bridge to $InstallDir\ts-bridge.exe"
  if (-not (Get-Command ts-bridge -ErrorAction SilentlyContinue)) {
    Write-Host "Ensure $InstallDir is on your PATH."
  }
} finally {
  if (Test-Path $tempDir) {
    Remove-Item -Path $tempDir -Recurse -Force
  }
}
