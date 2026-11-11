# WinFX APO dev helper. Runs as SYSTEM from on-demand scheduled tasks
# (\WinFX-dev\apo-install, apo-uninstall, audio-restart) created by setup.ps1.
# setup.ps1 copies this file to C:\Program Files\WinFX-dev, which only admins can
# change, so each task can only ever do the fixed action below.
param(
    [Parameter(Mandatory)]
    [ValidateSet('install', 'uninstall', 'restart')]
    [string]$Action
)
$ErrorActionPreference = 'Stop'

$Source = 'C:\path\to\winfx\target\release\winfx_apo.dll'
$Dir = 'C:\Program Files\WinFX-dev\apo'
$Dll = Join-Path $Dir 'winfx_apo.dll'
$LogDir = 'C:\ProgramData\WinFX-dev\logs'
$Log = Join-Path $LogDir 'helper.log'
$Status = Join-Path $LogDir 'helper-status.json'

# The APO's COM classes. Both point at the same DLL; DllGetClassObject decides.
$Classes = [ordered]@{
    '{77069DD4-922E-4A92-8B31-EA606FEDCE4B}' = 'WinFX capture effects (SFX)'
    '{B1392A38-EBF1-43A8-B2BB-0EC0E7E87D37}' = 'WinFX capture effects (MFX)'
}
$IID_IAudioProcessingObject = '{FD7F2B29-24D0-4B5C-B177-592C39F9CA10}'

function Write-Log([string]$m) {
    "$(Get-Date -Format o) [$Action] $m" | Add-Content -Path $Log -Encoding utf8
}

function Stop-Audio {
    Write-Log 'stopping Audiosrv'
    Stop-Service -Name Audiosrv -Force
    $deadline = (Get-Date).AddSeconds(10)
    while ((Get-Process audiodg -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 200
    }
    if (Get-Process audiodg -ErrorAction SilentlyContinue) {
        Write-Log 'audiodg still running; killing it'
        Stop-Process -Name audiodg -Force
        Start-Sleep -Milliseconds 500
    }
}

function Start-Audio {
    Write-Log 'starting Audiosrv'
    Start-Service -Name Audiosrv
}

function Register-Apo {
    foreach ($clsid in $Classes.Keys) {
        $name = $Classes[$clsid]
        $c = "HKLM:\SOFTWARE\Classes\CLSID\$clsid"
        New-Item -Path "$c\InprocServer32" -Force | Out-Null
        Set-ItemProperty -Path $c -Name '(default)' -Value $name
        Set-ItemProperty -Path "$c\InprocServer32" -Name '(default)' -Value $Dll
        Set-ItemProperty -Path "$c\InprocServer32" -Name 'ThreadingModel' -Value 'Both'

        $a = "HKLM:\SOFTWARE\Classes\AudioEngine\AudioProcessingObjects\$clsid"
        New-Item -Path $a -Force | Out-Null
        $vals = [ordered]@{
            FriendlyName = @('String', $name)
            Copyright = @('String', 'WinFX')
            MajorVersion = @('DWord', 1)
            MinorVersion = @('DWord', 0)
            Flags = @('DWord', 14)                 # APO_FLAG_DEFAULT
            MinInputConnections = @('DWord', 1)
            MaxInputConnections = @('DWord', 1)
            MinOutputConnections = @('DWord', 1)
            MaxOutputConnections = @('DWord', 1)
            MaxInstances = @('DWord', 0xffffffff)
            NumAPOInterfaces = @('DWord', 1)
            APOInterface0 = @('String', $IID_IAudioProcessingObject)
        }
        foreach ($k in $vals.Keys) {
            New-ItemProperty -Path $a -Name $k -PropertyType $vals[$k][0] -Value $vals[$k][1] -Force | Out-Null
        }
        Write-Log "registered $clsid"
    }
}

function Unregister-Apo {
    foreach ($clsid in $Classes.Keys) {
        foreach ($k in "HKLM:\SOFTWARE\Classes\CLSID\$clsid", "HKLM:\SOFTWARE\Classes\AudioEngine\AudioProcessingObjects\$clsid") {
            if (Test-Path $k) { Remove-Item -Path $k -Recurse -Force }
        }
        Write-Log "unregistered $clsid"
    }
}

$result = [ordered]@{ action = $Action; ok = $false; time = (Get-Date -Format o); error = $null; dll_sha256 = $null }
try {
    New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
    switch ($Action) {
        'install' {
            if (-not (Test-Path $Source)) { throw "DLL not built: $Source" }
            Stop-Audio
            New-Item -ItemType Directory -Force -Path $Dir | Out-Null
            Copy-Item -Path $Source -Destination $Dll -Force
            Register-Apo
            Start-Audio
            $result.dll_sha256 = (Get-FileHash -Algorithm SHA256 $Dll).Hash
        }
        'uninstall' {
            Stop-Audio
            Unregister-Apo
            if (Test-Path $Dll) { Remove-Item -Path $Dll -Force }
            Start-Audio
        }
        'restart' {
            Stop-Audio
            Start-Audio
        }
    }
    $result.ok = $true
    Write-Log 'done'
}
catch {
    $result.error = "$_"
    Write-Log "ERROR: $_"
    # Never leave the machine without audio.
    try { Start-Service -Name Audiosrv } catch { Write-Log "could not restart Audiosrv: $_" }
}
$result | ConvertTo-Json | Set-Content -Path $Status -Encoding utf8
