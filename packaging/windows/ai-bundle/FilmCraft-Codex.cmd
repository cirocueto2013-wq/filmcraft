@echo off
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0Start-FilmCraft.ps1"
if errorlevel 1 pause
