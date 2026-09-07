<#
.SYNOPSIS
    Build the one file there is to hand to somebody.

.DESCRIPTION
    Leaves dist\dialupmodem2.exe: a modem, the scope around it, the telnet
    terminal, the answering board and a capture to replay, in a single
    executable that needs nothing installed beside it.

    It opens on a real line, because that is what the program is for. Every
    other mode is a flag.

    The C runtime is linked in rather than depended on -- see .cargo/config.toml
    for why -- so the only things the file imports are Windows itself.
#>
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $MyInvocation.MyCommand.Definition
Set-Location $root

Write-Host ""
Write-Host "dialupmodem2 - building one file" -ForegroundColor Cyan

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
    if (Test-Path $cargoBin) { $env:PATH = "$env:PATH;$cargoBin" }
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host "  cargo not found. Install Rust from https://rustup.rs" -ForegroundColor Red
    exit 1
}

& cargo build --release -p gui
if ($LASTEXITCODE -ne 0) { Write-Host "  build failed" -ForegroundColor Red; exit 1 }

$built = Join-Path $root "target\release\modem-scope.exe"
if (-not (Test-Path $built)) {
    Write-Host "  built, but $built is missing" -ForegroundColor Red
    exit 1
}

$dist = Join-Path $root "dist"
New-Item -ItemType Directory -Force $dist | Out-Null
$out = Join-Path $dist "dialupmodem2.exe"
# Windows will not let a running executable be overwritten, and what it says
# about it is a stack trace naming Copy-Item. The interesting fact is that the
# last build is still open, which is a thing to be told rather than to work out.
try {
    Copy-Item $built $out -Force -ErrorAction Stop
} catch [System.IO.IOException] {
    Write-Host ""
    Write-Host "  $out is running. Close it and build again." -ForegroundColor Yellow
    Write-Host "  The new build is at $built and works from there." -ForegroundColor DarkGray
    Write-Host ""
    exit 1
}

# What it imports, so that "standalone" is a measurement rather than a claim.
$bytes = [IO.File]::ReadAllBytes($out)
$text = [Text.Encoding]::ASCII.GetString($bytes)
$runtime = [regex]::Matches($text, "[A-Za-z0-9_\-]+\.dll") |
    ForEach-Object { $_.Value.ToLower() } |
    Where-Object { $_ -match "vcruntime|api-ms-win-crt|msvcp|ucrtbase" } |
    Sort-Object -Unique

Write-Host ""
Write-Host ("  {0}  ({1:N1} MB)" -f $out, ((Get-Item $out).Length / 1MB))
if ($runtime) {
    Write-Host "  needs a C runtime installed: $($runtime -join ', ')" -ForegroundColor Yellow
} else {
    Write-Host "  no C runtime dependency; Windows is all it needs" -ForegroundColor DarkGray
}
Write-Host ""
Write-Host "  dialupmodem2.exe                  a modem on a real line" -ForegroundColor DarkGray
Write-Host "  dialupmodem2.exe --devices        what audio this machine has" -ForegroundColor DarkGray
Write-Host "  dialupmodem2.exe --telnet         a board over a socket" -ForegroundColor DarkGray
Write-Host "  dialupmodem2.exe --capture        replay the golden capture" -ForegroundColor DarkGray
Write-Host ""
