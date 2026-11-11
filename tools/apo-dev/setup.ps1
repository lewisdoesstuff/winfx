#Requires -RunAsAdministrator
# One-time, elevated setup for APO development. It:
#  1. backs up the audio device registry (MMDevices\Audio),
#  2. copies helper.ps1 into C:\Program Files\WinFX-dev (admin-only),
#  3. creates folders under C:\ProgramData\WinFX-dev for logs, APO logs and crash dumps,
#  4. turns on crash dumps for audiodg.exe,
#  5. registers three on-demand SYSTEM tasks the user account may start:
#     \WinFX-dev\apo-install, \WinFX-dev\apo-uninstall, \WinFX-dev\audio-restart.
# Undo everything with teardown.ps1.
$ErrorActionPreference = 'Stop'

$UserSid = '<your-user-SID>'
$Prog = 'C:\Program Files\WinFX-dev'
$Data = 'C:\ProgramData\WinFX-dev'
$ts = Get-Date -Format 'yyyyMMdd-HHmmss'

foreach ($d in $Prog, "$Prog\apo", $Data, "$Data\logs", "$Data\apo-logs", "$Data\backup", "$Data\dumps") {
    New-Item -ItemType Directory -Force -Path $d | Out-Null
}

# 1. Backup first.
& reg.exe export 'HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio' "$Data\backup\MMDevices-Audio-$ts.reg" /y | Out-Null
if ($LASTEXITCODE -ne 0) { throw 'registry backup failed' }

# 2. Helper.
Copy-Item -Path (Join-Path $PSScriptRoot 'helper.ps1') -Destination "$Prog\helper.ps1" -Force

# 3. Folder access: the user reads logs/dumps/backups; the APO (inside audiodg, LOCAL SERVICE) writes apo-logs.
& icacls.exe "$Data\logs" /grant "*${UserSid}:(OI)(CI)RX" | Out-Null
& icacls.exe "$Data\backup" /grant "*${UserSid}:(OI)(CI)RX" | Out-Null
& icacls.exe "$Data\dumps" /grant "*${UserSid}:(OI)(CI)RX" | Out-Null
& icacls.exe "$Data\apo-logs" /grant "*S-1-5-19:(OI)(CI)M" "*${UserSid}:(OI)(CI)M" | Out-Null

# 4. Crash dumps for the audio engine process.
$wer = 'HKLM:\SOFTWARE\Microsoft\Windows\Windows Error Reporting\LocalDumps\audiodg.exe'
New-Item -Path $wer -Force | Out-Null
New-ItemProperty -Path $wer -Name DumpFolder -PropertyType ExpandString -Value "$Data\dumps" -Force | Out-Null
New-ItemProperty -Path $wer -Name DumpCount -PropertyType DWord -Value 5 -Force | Out-Null
New-ItemProperty -Path $wer -Name DumpType -PropertyType DWord -Value 1 -Force | Out-Null

# 5. Tasks.
$svc = New-Object -ComObject Schedule.Service
$svc.Connect()
$root = $svc.GetFolder('\')
try { $folder = $svc.GetFolder('\WinFX-dev') } catch { $folder = $root.CreateFolder('WinFX-dev') }
$sddl = "D:(A;;FA;;;BA)(A;;FA;;;SY)(A;;GRGX;;;$UserSid)"
$tasks = [ordered]@{ 'apo-install' = 'install'; 'apo-uninstall' = 'uninstall'; 'audio-restart' = 'restart' }
foreach ($name in $tasks.Keys) {
    $def = $svc.NewTask(0)
    $def.RegistrationInfo.Description = "WinFX APO development: $($tasks[$name]) (see tools\apo-dev in the WinFX repo)"
    $def.Settings.Enabled = $true
    $def.Settings.AllowDemandStart = $true
    $def.Settings.DisallowStartIfOnBatteries = $false
    $def.Settings.StopIfGoingOnBatteries = $false
    $def.Settings.ExecutionTimeLimit = 'PT5M'
    $def.Settings.MultipleInstances = 2   # ignore a new start while one is running
    $def.Principal.RunLevel = 1           # highest
    $act = $def.Actions.Create(0)
    $act.Path = "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe"
    $act.Arguments = "-NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"$Prog\helper.ps1`" -Action $($tasks[$name])"
    # 6 = create or update, 5 = service account logon
    $folder.RegisterTaskDefinition($name, $def, 6, 'SYSTEM', $null, 5, $sddl) | Out-Null
}

Write-Host ''
Write-Host 'WinFX APO dev setup complete.' -ForegroundColor Green
Write-Host "  Registry backup: $Data\backup\MMDevices-Audio-$ts.reg"
Write-Host "  Tasks: \WinFX-dev\apo-install, apo-uninstall, audio-restart"
Write-Host '  Undo with teardown.ps1.'
Write-Host ''
Read-Host 'Press Enter to close'
