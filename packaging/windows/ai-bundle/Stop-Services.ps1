[CmdletBinding(SupportsShouldProcess)]
param([switch]$Kokoro, [switch]$ComfyUI)
$ErrorActionPreference = 'Stop'
if (-not $Kokoro -and -not $ComfyUI) { $Kokoro = $true; $ComfyUI = $true }
if ($Kokoro -and $PSCmdlet.ShouldProcess('filmcraft-ai-kokoro', 'Stop only this Docker container')) {
    & docker stop filmcraft-ai-kokoro
    if ($LASTEXITCODE -ne 0) { throw 'Could not stop the FilmCraft Kokoro container.' }
}
if ($ComfyUI) {
    $Record = Join-Path $env:LOCALAPPDATA 'FilmCraftAI\Services\comfy-process.json'
    if (-not (Test-Path -LiteralPath $Record)) { Write-Output 'No owned ComfyUI process is recorded.'; return }
    $Owned = Get-Content -Raw -LiteralPath $Record | ConvertFrom-Json
    $Expected = Join-Path $env:LOCALAPPDATA 'FilmCraftAI\Services\comfy-venv\Scripts\python.exe'
    if ($Owned.executable -ne $Expected) { throw 'Recorded executable is outside the owned ComfyUI environment. Nothing was stopped.' }
    $Process = Get-Process -Id $Owned.pid -ErrorAction SilentlyContinue
    if (-not $Process) { Write-Output 'Recorded ComfyUI process has already stopped.'; return }
    if ($Process.Path -ne $Owned.executable -or $Process.StartTime.ToUniversalTime().ToString('o') -ne $Owned.started) { throw 'Process identity changed. Nothing was stopped.' }
    if ($PSCmdlet.ShouldProcess("ComfyUI PID $($Owned.pid)", 'Stop only the recorded FilmCraft service')) { Stop-Process -Id $Owned.pid -ErrorAction Stop }
}
