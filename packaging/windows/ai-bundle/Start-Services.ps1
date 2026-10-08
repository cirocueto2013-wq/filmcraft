[CmdletBinding()]
param([switch]$Kokoro, [switch]$ComfyUI)
$ErrorActionPreference = 'Stop'
if (-not $Kokoro -and -not $ComfyUI) { $Kokoro = $true; $ComfyUI = $true }
if ($Kokoro) {
    & docker start filmcraft-ai-kokoro
    if ($LASTEXITCODE -ne 0) { throw 'Kokoro could not start. Run Setup-Services.ps1 -Kokoro and check Docker Desktop.' }
}
if ($ComfyUI) {
    $Services = Join-Path $env:LOCALAPPDATA 'FilmCraftAI\Services'
    $Python = Join-Path $Services 'comfy-venv\Scripts\python.exe'
    $Source = Join-Path $Services 'ComfyUI'
    if (-not (Test-Path -LiteralPath $Python)) { throw 'ComfyUI is not installed. Run Setup-Services.ps1 -ComfyUI.' }
    try { $Ready = Invoke-RestMethod 'http://127.0.0.1:8188/system_stats' -TimeoutSec 3 } catch { $Ready = $null }
    if ($Ready) { Write-Output 'ComfyUI is already serving on port 8188.'; return }
    $Logs = Join-Path $Services 'logs'; New-Item -ItemType Directory -Force -Path $Logs | Out-Null
    $Stamp = Get-Date -Format yyyyMMdd-HHmmss-ffff
    $Process = Start-Process -FilePath $Python -ArgumentList @('main.py','--cpu','--listen','127.0.0.1','--port','8188','--disable-auto-launch','--disable-all-custom-nodes') -WorkingDirectory $Source -RedirectStandardOutput (Join-Path $Logs "comfy-$Stamp.out.log") -RedirectStandardError (Join-Path $Logs "comfy-$Stamp.err.log") -PassThru
    @{ pid = $Process.Id; started = $Process.StartTime.ToUniversalTime().ToString('o'); executable = $Python } | ConvertTo-Json | Set-Content -Encoding UTF8 (Join-Path $Services 'comfy-process.json')
    Write-Output "ComfyUI starting (PID $($Process.Id)); logs: $Logs. CPU inference is slower than GPU inference."
}
