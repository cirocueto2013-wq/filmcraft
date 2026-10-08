[CmdletBinding()]
param([switch]$Services)
$ErrorActionPreference = 'Stop'
$Info = Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot 'BUILD_INFO.json') | ConvertFrom-Json
foreach ($File in $Info.files.PSObject.Properties) {
    $Path = Join-Path $PSScriptRoot $File.Name
    if (-not (Test-Path -LiteralPath $Path)) { throw "Missing bundle file: $($File.Name)" }
    if ((Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant() -ne $File.Value) { throw "Bundle checksum mismatch: $($File.Name)" }
}
& (Join-Path $PSScriptRoot 'filmcraft-cli.exe') --version
if ($LASTEXITCODE -ne 0) { throw 'The FilmCraft CLI could not run.' }
$Catalog = & (Join-Path $PSScriptRoot 'filmcraft-cli.exe') commands
if ($LASTEXITCODE -ne 0 -or ($Catalog -join "`n") -notmatch 'ai\.autopilot') { throw 'This CLI does not contain the expected AI/MCP commands.' }
Write-Output "FilmCraft AI bundle verified: commit $($Info.commit), Windows x64."
if ($Services) {
    $Voices = Invoke-RestMethod 'http://127.0.0.1:8880/v1/audio/voices' -TimeoutSec 30
    if (-not $Voices.voices) { throw 'Kokoro returned no voices.' }
    $Comfy = Invoke-RestMethod 'http://127.0.0.1:8188/system_stats' -TimeoutSec 30
    if (-not $Comfy.system) { throw 'ComfyUI did not return system information.' }
    Write-Output 'Kokoro and ComfyUI respond. Verify real narration and the bundled workflow from the Assistant panel.'
}
