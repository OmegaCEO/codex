param(
    [string]$BuiltCodexExe = "C:\cb\release\codex-chima-v1.exe",
    [string]$DestinationRoot = "$env:LOCALAPPDATA\OpenAI\Codex-CHIMA-v1",
    [string]$CodexHome = "$env:USERPROFILE\.codex-chima-v1",
    [string]$SourceAppDir = ""
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $BuiltCodexExe)) {
    throw "Built Codex exe not found: $BuiltCodexExe"
}

if (-not $SourceAppDir) {
    $packageRoot = Get-ChildItem -LiteralPath "$env:ProgramFiles\WindowsApps" -Directory -Filter "OpenAI.Codex_*__2p2nqsd0c76g0" |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1

    if (-not $packageRoot) {
        throw "Could not find installed OpenAI Codex package under $env:ProgramFiles\WindowsApps. Pass -SourceAppDir explicitly."
    }

    $SourceAppDir = Join-Path $packageRoot.FullName "app"
}

if (-not (Test-Path -LiteralPath $SourceAppDir)) {
    throw "Source app directory not found: $SourceAppDir"
}

$destinationAppDir = Join-Path $DestinationRoot "app"
New-Item -ItemType Directory -Force -Path $DestinationRoot | Out-Null

if (Test-Path -LiteralPath $destinationAppDir) {
    $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
    $backupDir = Join-Path $DestinationRoot "app.backup-$stamp"
    Move-Item -LiteralPath $destinationAppDir -Destination $backupDir
}

robocopy $SourceAppDir $destinationAppDir /E /COPY:DAT /DCOPY:DAT /R:1 /W:1 /NFL /NDL /NP | Out-Null
if ($LASTEXITCODE -gt 7) {
    throw "robocopy failed with exit code $LASTEXITCODE"
}

$resourcesDir = Join-Path $destinationAppDir "resources"
if (-not (Test-Path -LiteralPath $resourcesDir)) {
    throw "Copied app has no resources directory: $resourcesDir"
}

Copy-Item -LiteralPath $BuiltCodexExe -Destination (Join-Path $resourcesDir "codex.exe") -Force

$launcherPath = Join-Path $DestinationRoot "Launch-Codex-CHIMA-v1.ps1"
$launcher = @"
`$env:CODEX_HOME = "$CodexHome"
Start-Process -FilePath "$destinationAppDir\Codex.exe" -WorkingDirectory "$destinationAppDir"
"@
Set-Content -LiteralPath $launcherPath -Value $launcher -Encoding UTF8

[pscustomobject]@{
    SourceAppDir = $SourceAppDir
    DestinationAppDir = $destinationAppDir
    PatchedCodexExe = (Join-Path $resourcesDir "codex.exe")
    LauncherPath = $launcherPath
    CodexHome = $CodexHome
}
