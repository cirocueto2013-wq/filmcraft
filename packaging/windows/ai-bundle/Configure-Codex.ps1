[CmdletBinding(SupportsShouldProcess)]
param([string]$CodexCommand = 'codex', [ValidateRange(1,65535)][int]$Port = 9876)
$ErrorActionPreference = 'Stop'
$Cli = Join-Path $PSScriptRoot 'filmcraft-cli.exe'
if (-not (Test-Path -LiteralPath $Cli)) { throw 'filmcraft-cli.exe is missing. Install/extract the complete AI bundle.' }
$ConfigRoot = if ($env:CODEX_HOME) { $env:CODEX_HOME } else { Join-Path $env:USERPROFILE '.codex' }
$Config = Join-Path $ConfigRoot 'config.toml'
if (-not $PSCmdlet.ShouldProcess($Config, "Register only MCP server filmcraft-ai: $Cli mcp --bridge 127.0.0.1:$Port")) { return }
$Codex = Get-Command $CodexCommand -ErrorAction Stop
if (Test-Path -LiteralPath $Config) {
    $Backup = "$Config.filmcraft-$(Get-Date -Format yyyyMMdd-HHmmss-ffff).bak"
    Copy-Item -LiteralPath $Config -Destination $Backup
    Write-Output "Existing Codex configuration backed up to $Backup"
}
& $Codex.Source mcp add filmcraft-ai -- $Cli mcp --bridge "127.0.0.1:$Port"
if ($LASTEXITCODE -ne 0) { throw "codex mcp add failed (exit $LASTEXITCODE). The backup was preserved." }
Write-Output 'FilmCraft MCP registered. Restart Codex and use your existing ChatGPT login; no API key is required for Codex editing.'
Write-Output 'Start FilmCraft-Codex.cmd before using its MCP tools. This does not change your model, login, permissions or other MCP servers.'
