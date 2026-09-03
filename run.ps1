<#
.SYNOPSIS
    Build and launch the dialupmodem2 scope.

.DESCRIPTION
    With no arguments, offers a menu of the golden capture vectors and launches
    the scope on the chosen one. Only the Bell 103 capture decodes to text so
    far; the rest still show their handshakes on the waterfall, which is worth
    watching in its own right.

.PARAMETER Vector
    Path to a WAV to load, or the short name of one in tests\vectors
    (for example "v34-33600"). Skips the menu.

.PARAMETER Dev
    Build the debug profile instead of release. Slower, but builds faster.

.PARAMETER List
    Print the available vectors and exit.

.EXAMPLE
    .\run.ps1
.EXAMPLE
    .\run.ps1 -Vector v34-33600
#>
param(
    [string] $Vector = "",
    [switch] $Dev,
    [switch] $List
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $MyInvocation.MyCommand.Definition
Set-Location $root

function Write-Step($text) { Write-Host "  $text" -ForegroundColor DarkGray }
function Write-Fail($text) { Write-Host "  $text" -ForegroundColor Red }

# What each capture contains, so the menu is informative rather than a list of
# filenames. Kept in step with tests\vectors\README.md.
$notes = [ordered]@{
    "bell103-300"   = "300 bps FSK  - decodes to text; the login session"
    "v22bis-2400"   = "2400 bps     - two bands, frequency-division duplex"
    "v32bis-14400"  = "14.4k        - one band, echo-cancelled"
    "v34-33600"     = "33.6k        - V.8 negotiation and the probing tones"
    "v90-56k"       = "56k V.90     - V.34-style startup"
    "v92-56k"       = "56k V.92     - V.34-style startup"
}

Write-Host ""
Write-Host "dialupmodem2" -ForegroundColor Cyan

# Rust may be installed but not on PATH in a fresh shell.
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
    if (Test-Path $cargoBin) {
        $env:PATH = "$env:PATH;$cargoBin"
        Write-Step "added $cargoBin to PATH for this session"
    }
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Fail "cargo not found. Install Rust from https://rustup.rs and reopen this window."
    exit 1
}

$vectorDir = Join-Path $root "tests\vectors"
$available = @()
if (Test-Path $vectorDir) {
    $available = @(Get-ChildItem (Join-Path $vectorDir "*.wav") | Sort-Object Name)
}

if ($List) {
    Write-Host ""
    foreach ($v in $available) {
        $name = [IO.Path]::GetFileNameWithoutExtension($v.Name)
        $note = $notes[$name]
        if (-not $note) { $note = "" }
        Write-Host ("  {0,-16} {1}" -f $name, $note)
    }
    Write-Host ""
    exit 0
}

# Resolve the vector: an explicit path, a short name, or the menu.
$chosen = $null
if ($Vector) {
    if (Test-Path $Vector) {
        $chosen = (Resolve-Path $Vector).Path
    } else {
        $candidate = Join-Path $vectorDir "$Vector.wav"
        if (Test-Path $candidate) {
            $chosen = (Resolve-Path $candidate).Path
        } else {
            Write-Fail "no such vector: $Vector"
            Write-Step "run with -List to see what is available"
            exit 1
        }
    }
} elseif ($available.Count -eq 0) {
    Write-Fail "no vectors in tests\vectors. Run: python tools\extract_vectors.py"
    exit 1
} else {
    Write-Host ""
    Write-Host "  which capture?" -ForegroundColor White
    Write-Host ""
    for ($i = 0; $i -lt $available.Count; $i++) {
        $name = [IO.Path]::GetFileNameWithoutExtension($available[$i].Name)
        $note = $notes[$name]
        if (-not $note) { $note = "" }
        $marker = " "
        if ($i -eq 0) { $marker = "*" }
        Write-Host ("   {0}{1}) {2,-16} {3}" -f $marker, ($i + 1), $name, $note)
    }
    Write-Host ""
    $answer = Read-Host "  number, or Enter for 1"
    if (-not $answer) { $answer = "1" }
    $index = 0
    if (-not [int]::TryParse($answer, [ref]$index) -or $index -lt 1 -or $index -gt $available.Count) {
        Write-Fail "not a choice: $answer"
        exit 1
    }
    $chosen = $available[$index - 1].FullName
}

$profileName = "release"
if ($Dev) { $profileName = "debug" }

Write-Host ""
Write-Step "building gui ($profileName)"
$buildArgs = @("build", "-p", "gui")
if (-not $Dev) { $buildArgs += "--release" }
& cargo @buildArgs
if ($LASTEXITCODE -ne 0) {
    Write-Fail "build failed"
    exit 1
}

$exe = Join-Path $root "target\$profileName\modem-scope.exe"
if (-not (Test-Path $exe)) {
    Write-Fail "built, but $exe is missing"
    exit 1
}

Write-Step ("launching " + [IO.Path]::GetFileNameWithoutExtension($chosen))
Write-Host ""
Write-Host "  click the terminal pane and type AT, then ATD to replay." -ForegroundColor DarkGray
Write-Host "  Listen plays the line audio out of a chosen device." -ForegroundColor DarkGray
Write-Host ""

& $exe $chosen
exit $LASTEXITCODE
