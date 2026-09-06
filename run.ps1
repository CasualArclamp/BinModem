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

.PARAMETER Live
    Put a real modem on a real line instead of replaying a capture. Offers a
    menu of the machine's audio devices, and offers to start a second modem on
    the same line so that there is something to dial.

.PARAMETER Telnet
    Open a terminal onto a board over a socket, with no modem and no line
    anywhere in it. For working on the terminal rather than on the modem: a
    board sends the same ANSI either way, but over a socket every byte arrives,
    so anything that draws wrongly is the terminal's fault and not the line's.
    Takes a host, or nothing to choose one in the window.

.PARAMETER Carrier
    Modulation for a live call: B103, V22B or V32. Both ends have to agree, so
    this sets it for the board as well.

.EXAMPLE
    .\run.ps1
.EXAMPLE
    .\run.ps1 -Vector v34-33600
.EXAMPLE
    .\run.ps1 -Live
.EXAMPLE
    .\run.ps1 -Telnet
.EXAMPLE
    .\run.ps1 -Telnet vert.synchro.net
#>
param(
    [string] $Vector = "",
    [switch] $Dev,
    [switch] $List,
    [switch] $Live,
    [Parameter()] [AllowEmptyString()]
    [string] $Telnet,
    [switch] $TelnetOnly,
    [ValidateSet("B103", "V22B", "V32")]
    [string] $Carrier = "V22B"
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

# ---- a board over a socket ------------------------------------------------
# No audio at all, so none of the device business below applies: build, launch,
# done. The window opens with the host box empty unless one was named.
if ($TelnetOnly -or $Telnet) {
    $profileName = "release"
    if ($Dev) { $profileName = "debug" }
    Write-Host ""
    Write-Step "building gui ($profileName)"
    $buildArgs = @("build", "-p", "gui")
    if (-not $Dev) { $buildArgs += "--release" }
    & cargo @buildArgs
    if ($LASTEXITCODE -ne 0) { Write-Fail "build failed"; exit 1 }

    $scope = Join-Path $root "target\$profileName\modem-scope.exe"
    if (-not (Test-Path $scope)) { Write-Fail "built, but $scope is missing"; exit 1 }

    $scopeArgs = @("--telnet")
    if ($Telnet) { $scopeArgs += $Telnet }
    Write-Host ""
    Write-Step "terminal only: no modem, no line, every byte arrives"
    & $scope @scopeArgs
    exit $LASTEXITCODE
}

# ---- a real line ----------------------------------------------------------
if ($Live) {
    $profileName = "release"
    if ($Dev) { $profileName = "debug" }
    Write-Host ""
    Write-Step "building gui and modem ($profileName)"
    $buildArgs = @("build", "-p", "gui", "-p", "modem")
    if (-not $Dev) { $buildArgs += "--release" }
    & cargo @buildArgs
    if ($LASTEXITCODE -ne 0) { Write-Fail "build failed"; exit 1 }

    $scope = Join-Path $root "target\$profileName\modem-scope.exe"
    $answer = Join-Path $root "target\$profileName\modem-answer.exe"
    if (-not (Test-Path $scope)) { Write-Fail "built, but $scope is missing"; exit 1 }

    # Ask the binary rather than keeping a second list here: it is the thing
    # that has to open the device, so its names are the ones that matter.
    $listing = & $scope --devices
    $inputs = @()
    $outputs = @()
    $section = ""
    foreach ($line in $listing) {
        if ($line -match "^input devices")  { $section = "in";  continue }
        if ($line -match "^output devices") { $section = "out"; continue }
        $name = $line.Trim()
        if (-not $name) { continue }
        if ($section -eq "in")  { $inputs  += $name }
        if ($section -eq "out") { $outputs += $name }
    }
    if ($inputs.Count -eq 0 -or $outputs.Count -eq 0) {
        Write-Fail "no audio devices found"
        exit 1
    }

    # A virtual cable is almost always the right answer, so it is the default.
    function Choose($what, $names, $prefer) {
        $default = 0
        for ($i = 0; $i -lt $names.Count; $i++) {
            if ($names[$i] -like $prefer) { $default = $i; break }
        }
        Write-Host ""
        Write-Host "  which $what?" -ForegroundColor White
        Write-Host ""
        for ($i = 0; $i -lt $names.Count; $i++) {
            $marker = " "
            if ($i -eq $default) { $marker = "*" }
            Write-Host ("   {0}{1}) {2}" -f $marker, ($i + 1), $names[$i])
        }
        Write-Host ""
        $reply = Read-Host ("  number, or Enter for " + ($default + 1))
        if (-not $reply) { return $names[$default] }
        $index = 0
        if (-not [int]::TryParse($reply, [ref]$index) -or $index -lt 1 -or $index -gt $names.Count) {
            Write-Fail "not a choice: $reply"
            exit 1
        }
        return $names[$index - 1]
    }

    $inName = Choose "input (what the line says)" $inputs "*CABLE Output*"
    $outName = Choose "output (what the modem says)" $outputs "*CABLE Input*"

    Write-Host ""
    $board = Read-Host "  start a board on the same line to dial? [Y/n]"
    if ($board -ne "n" -and $board -ne "N") {
        if (-not (Test-Path $answer)) {
            Write-Fail "built, but $answer is missing"
            exit 1
        }
        Write-Step "starting the board in its own window"
        Start-Process -FilePath $answer -ArgumentList @(
            "--in", $inName, "--out", $outName, "--carrier", $Carrier
        )
        # Let it get its streams open before the caller starts listening.
        Start-Sleep -Milliseconds 700
    }

    Write-Host ""
    Write-Host "  in the terminal pane: AT+MS=$Carrier then ATD5551234" -ForegroundColor DarkGray
    Write-Host "  +++ escapes to command state, ATH hangs up." -ForegroundColor DarkGray
    Write-Host ""
    & $scope --live --in $inName --out $outName
    exit $LASTEXITCODE
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
