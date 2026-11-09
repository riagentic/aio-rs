# Installs aio-rs @VERSION@ (the `ax` command), prebuilt: made by
# `ax release --aio`, which pinned each build's SHA-256 here. Nothing
# runs before its checksum matches.
#   powershell -ExecutionPolicy Bypass -File install.ps1   (beside the release's files: offline)
#   irm <url>/install.ps1 | iex                            (aio's one line)
#   $env:AIO_DOWNLOAD = '<url>' first for another url, or a folder holding the release's files
# ax goes where cargo puts what it installs ($env:CARGO_HOME\bin), then
# `ax setup` checks and fetches what making apps needs. A failure throws:
# under iex the window stays open, under -File the exit code is 1.
function Install-Ax {
  $ErrorActionPreference = 'Stop'
  $version = '@VERSION@'
  $pins = @{
@PINS@
  }
  $here = $PSScriptRoot
  $base = if ($env:AIO_DOWNLOAD) { $env:AIO_DOWNLOAD } elseif ('@URL@') { '@URL@' } else { $here }
  $sourceBuild = "from source instead (minutes): cargo install --locked --git https://github.com/riagentic/aio-rs --tag v$version aio-rs-cli"
  $triple = if ($env:PROCESSOR_ARCHITECTURE -eq 'AMD64') { 'x86_64-pc-windows-gnu' } else { $env:PROCESSOR_ARCHITECTURE }
  $want = $pins[$triple]
  if (-not $want) { throw "ax: no prebuilt aio-rs $version for Windows $triple; $sourceBuild" }
  $cargo = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $HOME '.cargo' }
  $bin = Join-Path $cargo 'bin'
  if (-not (Test-Path $bin)) { throw "ax: no ${bin}: install Rust first (ax builds apps with cargo): winget install Rustlang.Rustup" }
  $file = "aio-rs-$version-$triple.zip"
  # Run beside the release's files, it takes them (the pinned url is for a download).
  if (-not $env:AIO_DOWNLOAD -and $here -and (Test-Path (Join-Path $here $file))) { $base = $here }
  $tmp = Join-Path ([IO.Path]::GetTempPath()) ("ax-install-" + [Guid]::NewGuid())
  New-Item -ItemType Directory $tmp | Out-Null
  try {
    try {
      if ($base -match '^(https?|file)://') {
        Invoke-WebRequest -UseBasicParsing -Uri "$base/$file" -OutFile "$tmp\$file"
      } else {
        Copy-Item (Join-Path $base $file) "$tmp\$file"
      }
    } catch {
      throw "ax: could not get $base/${file}: $($_.Exception.Message); $sourceBuild"
    }
    $got = (Get-FileHash -Algorithm SHA256 "$tmp\$file").Hash.ToLower()
    if ($got -ne $want) { throw "ax: $file has SHA-256 $got, expected ${want}: not installed" }
    Expand-Archive "$tmp\$file" -DestinationPath "$tmp\x"
    Move-Item -Force "$tmp\x\ax.exe" (Join-Path $bin 'ax.exe')
  } finally {
    Remove-Item -Recurse -Force $tmp
  }
  Write-Host "aio-rs $version installed: $bin\ax.exe"
  & (Join-Path $bin 'ax.exe') setup
  if ($LASTEXITCODE -ne 0) { throw "ax is installed; ax setup says above what is still missing" }
}

Install-Ax
