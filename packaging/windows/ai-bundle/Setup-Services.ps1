[CmdletBinding(SupportsShouldProcess)]
param([switch]$Kokoro, [switch]$ComfyUI, [string]$PythonCommand = 'py')
$ErrorActionPreference = 'Stop'
if (-not $Kokoro -and -not $ComfyUI) { throw 'Select -Kokoro, -ComfyUI or both. Downloads can be several GB.' }
$Services = Join-Path $env:LOCALAPPDATA 'FilmCraftAI\Services'
$Image = 'ghcr.io/remsky/kokoro-fastapi-cpu@sha256:ee3111d6a2c903ed62f3b4fa19543c6901205ed39fce3c177e895b34a8386b9c'
$ComfyCommit = '52f98af2e2e42c421070a3e147c161c47cdeaf22'
function Run-Checked([string]$Executable, [string[]]$Arguments) {
    & $Executable @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Executable failed (exit $LASTEXITCODE). Installation can be retried; existing projects are unchanged." }
}
if ($Kokoro -and $PSCmdlet.ShouldProcess('filmcraft-ai-kokoro (Docker)', 'Pull pinned official CPU image and create localhost-only service')) {
    $Docker = (Get-Command docker -ErrorAction Stop).Source
    Run-Checked $Docker @('info', '--format', '{{.OSType}}')
    $Existing = & $Docker ps -a --filter 'name=^/filmcraft-ai-kokoro$' --format '{{.Names}}'
    if ($LASTEXITCODE -ne 0) { throw 'Cannot inspect Docker containers. Start Docker Desktop in Linux-container mode.' }
    if ($Existing) {
        $OwnedImage = & $Docker inspect --format '{{.Config.Image}}' filmcraft-ai-kokoro
        if ($LASTEXITCODE -ne 0 -or $OwnedImage -ne $Image) { throw 'A container named filmcraft-ai-kokoro already uses another image. It was not replaced.' }
        Run-Checked $Docker @('start', 'filmcraft-ai-kokoro')
    } else {
        Run-Checked $Docker @('pull', $Image)
        Run-Checked $Docker @('run','--detach','--name','filmcraft-ai-kokoro','--restart','unless-stopped','--publish','127.0.0.1:8880:8880',$Image)
    }
    Write-Output 'Kokoro created on http://127.0.0.1:8880. Initial model loading may take a few minutes.'
}
if ($ComfyUI -and $PSCmdlet.ShouldProcess($Services, 'Install pinned upstream ComfyUI and isolated CPU Python environment')) {
    $Python = (Get-Command $PythonCommand -ErrorAction Stop).Source
    $Prefix = if ([IO.Path]::GetFileNameWithoutExtension($Python) -eq 'py') { @('-3.12') } else { @() }
    $Git = (Get-Command git -ErrorAction Stop).Source
    New-Item -ItemType Directory -Force -Path $Services | Out-Null
    $Source = Join-Path $Services 'ComfyUI'
    if (-not (Test-Path -LiteralPath $Source)) {
        Run-Checked $Git @('clone','--no-checkout','https://github.com/Comfy-Org/ComfyUI.git',$Source)
        Run-Checked $Git @('-C',$Source,'checkout','--detach',$ComfyCommit)
    } else {
        $Head = & $Git -C $Source rev-parse HEAD
        if ($LASTEXITCODE -ne 0 -or $Head -ne $ComfyCommit) { throw 'Existing ComfyUI checkout differs from the pinned version. It was not reset or overwritten.' }
        $Dirty = & $Git -C $Source status --porcelain --untracked-files=no
        if ($LASTEXITCODE -ne 0 -or $Dirty) { throw 'Existing ComfyUI has local source changes. It was not overwritten.' }
    }
    $Venv = Join-Path $Services 'comfy-venv'
    $VenvPython = Join-Path $Venv 'Scripts\python.exe'
    if (-not (Test-Path -LiteralPath $VenvPython)) { Run-Checked $Python ($Prefix + @('-m','venv',$Venv)) }
    Run-Checked $VenvPython @('-m','pip','install','--upgrade','pip')
    Run-Checked $VenvPython @('-m','pip','install','torch==2.14.1','torchvision','--index-url','https://download.pytorch.org/whl/cpu')
    Run-Checked $VenvPython @('-m','pip','install','-r',(Join-Path $Source 'requirements.txt'))
    Write-Output "ComfyUI installed at $Source. Models are not included; use the bundled core workflow to verify the connection first."
}
Write-Output 'Use Start-Services.ps1, then Verify-Installation.ps1. FilmCraft defaults already point to localhost ports 8880 / 8188.'
