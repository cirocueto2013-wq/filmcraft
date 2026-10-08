[CmdletBinding()]
param([ValidateRange(1,65535)][int]$Port = 9876)
$ErrorActionPreference = 'Stop'
function Inspect-Editor {
    $Client = New-Object Net.Sockets.TcpClient
    try {
        $Connect = $Client.ConnectAsync('127.0.0.1', $Port)
        if (-not $Connect.Wait(500)) { return $null }
        $Stream = $Client.GetStream(); $Stream.ReadTimeout = 2000; $Stream.WriteTimeout = 2000
        $Writer = New-Object IO.StreamWriter($Stream); $Writer.AutoFlush = $true
        $Reader = New-Object IO.StreamReader($Stream)
        $Writer.WriteLine('{"id":1,"method":"engine.execute","params":{"command":"ai.inspect","params":{}}}')
        $Text = New-Object Text.StringBuilder
        $Deadline = [DateTime]::UtcNow.AddSeconds(3)
        for ($Count = 0; $Count -lt 65536 -and [DateTime]::UtcNow -lt $Deadline; $Count++) {
            $Char = $Reader.Read()
            if ($Char -lt 0) { return $null }
            if ($Char -eq 10) { return ($Text.ToString() | ConvertFrom-Json) }
            [void]$Text.Append([char]$Char)
        }
        return $null
    } catch { return $null } finally { $Client.Dispose() }
}
$Existing = Inspect-Editor
if ($Existing) {
    if (-not $Existing.ok) { throw "Port $Port belongs to an editor without AI support. Close it or choose another matching port in both launcher and Codex config." }
    Write-Output "FilmCraft AI is already running on port $Port."; return
}
$App = Join-Path $PSScriptRoot 'filmcraft.exe'
if (-not (Test-Path -LiteralPath $App)) { throw 'filmcraft.exe is missing. Install/extract the complete AI bundle.' }
$Data = Join-Path $env:LOCALAPPDATA 'FilmCraftAI\AppData'
New-Item -ItemType Directory -Force -Path $Data | Out-Null
$Process = Start-Process -FilePath $App -ArgumentList @('--control', "$Port", '--data-dir', ('"' + $Data + '"')) -WorkingDirectory $PSScriptRoot -PassThru
$Deadline = [DateTime]::UtcNow.AddSeconds(30)
while ([DateTime]::UtcNow -lt $Deadline) {
    if ($Process.HasExited) { throw "FilmCraft exited during startup (exit $($Process.ExitCode))." }
    $Ready = Inspect-Editor
    if ($Ready -and $Ready.ok) { Write-Output "FilmCraft AI ready for Codex on 127.0.0.1:$Port."; return }
    Start-Sleep -Milliseconds 250
}
throw 'FilmCraft opened, but its MCP bridge did not become ready within 30 seconds. Check the app and port.'
