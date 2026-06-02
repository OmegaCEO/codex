param(
    [string]$BinaryPath = "C:\cb\release\codex-chima-v1.exe",
    [string]$CodexHome = "$env:USERPROFILE\.codex-chima-v1",
    [switch]$AppServer,
    [switch]$RemoteControl,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$CodexArgs
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path -LiteralPath $BinaryPath)) {
    throw "CHIMA Codex binary not found: $BinaryPath"
}

New-Item -ItemType Directory -Force -Path $CodexHome | Out-Null
$env:CODEX_HOME = $CodexHome

if ($AppServer) {
    $args = @("app-server")
    if ($RemoteControl) {
        $args += @("--remote-control")
    }
    if ($CodexArgs) {
        $args += $CodexArgs
    }
    & $BinaryPath @args
    exit $LASTEXITCODE
}

if ($CodexArgs) {
    & $BinaryPath @CodexArgs
} else {
    & $BinaryPath
}
exit $LASTEXITCODE
