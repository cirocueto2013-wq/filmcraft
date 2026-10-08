param([ValidateSet('x64')][string]$Arch = 'x64', [switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$Target = 'x86_64-pc-windows-msvc'
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root 'target' }
$Dist = if ($env:DIST) { $env:DIST } else { Join-Path $Root 'dist\ai' }
$env:FILMCRAFT_FORK_BUILD = '1'
if (-not $SkipBuild) {
    $env:CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS = '-C target-feature=+crt-static'
    $env:FILMCRAFT_REQUIRE_WINRES = '1'
    & cargo build --release --locked -p filmcraft -p filmcraft-cli --target $Target
    if ($LASTEXITCODE -ne 0) { throw 'Windows AI build failed.' }
}
$Bin = Join-Path $TargetDir "$Target\release"
& (Join-Path $PSScriptRoot 'sign.ps1') (Join-Path $Bin 'filmcraft.exe') (Join-Path $Bin 'filmcraft-cli.exe')
Push-Location $Root
try {
    $Json = & python (Join-Path $PSScriptRoot 'create_ai_bundle.py') --bin-dir $Bin --dist $Dist --wix-format 4
    if ($LASTEXITCODE -ne 0) { throw 'AI bundle staging failed.' }
    $Info = ($Json -join "`n") | ConvertFrom-Json
    if ($Info.sourceDirty) { throw 'A distributable bundle needs a committed source tree.' }
    $Msi = [IO.Path]::ChangeExtension($Info.wxs, '.msi')
    & wix build $Info.wxs -arch x64 -o $Msi
    if ($LASTEXITCODE -ne 0) { throw 'AI MSI build failed.' }
    & (Join-Path $PSScriptRoot 'sign.ps1') $Msi
    & (Join-Path $Info.stage 'Verify-Installation.ps1')
    Get-ChildItem -LiteralPath $Dist -File | Where-Object { $_.Extension -in '.msi','.zip' } | ForEach-Object {
        "$((Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant())  $($_.Name)"
    } | Set-Content -Encoding ASCII (Join-Path $Dist 'SHA256SUMS.txt')
    Get-Item $Msi, $Info.zip | Format-Table Name, Length
} finally { Pop-Location }
