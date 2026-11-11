#Requires -RunAsAdministrator
# Undo setup.ps1: uninstall the APO, delete the tasks, the helper, and the crash-dump setting.
# Logs and registry backups in C:\ProgramData\WinFX-dev are kept; delete that folder by hand.
$ErrorActionPreference = 'Continue'

$Prog = 'C:\Program Files\WinFX-dev'
if (Test-Path "$Prog\helper.ps1") {
    & "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -ExecutionPolicy Bypass -File "$Prog\helper.ps1" -Action uninstall
}

$svc = New-Object -ComObject Schedule.Service
$svc.Connect()
try {
    $folder = $svc.GetFolder('\WinFX-dev')
    foreach ($name in 'apo-install', 'apo-uninstall', 'audio-restart') {
        try { $folder.DeleteTask($name, 0) } catch {}
    }
    $svc.GetFolder('\').DeleteFolder('WinFX-dev', 0)
} catch {}

Remove-Item -Path 'HKLM:\SOFTWARE\Microsoft\Windows\Windows Error Reporting\LocalDumps\audiodg.exe' -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item -Path $Prog -Recurse -Force -ErrorAction SilentlyContinue

Write-Host 'WinFX APO dev setup removed. Logs and backups remain in C:\ProgramData\WinFX-dev.' -ForegroundColor Green
Read-Host 'Press Enter to close'
