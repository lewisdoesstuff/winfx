# Attach or detach the WinFX APO on one capture endpoint. Runs as the normal
# user: Windows lets Users set values under an endpoint's FxProperties key.
# The original values are saved before the first attach and put back on detach.
# Run \WinFX-dev\audio-restart afterwards so the audio engine reloads.
param(
    [Parameter(Mandatory)][string]$Endpoint,       # e.g. {c950d195-7502-492a-89a4-9d30cbb65fc7}
    [Parameter(Mandatory)][ValidateSet('attach', 'detach', 'show')][string]$Mode,
    [ValidateSet('SFX', 'MFX')][string]$Slot = 'SFX'
)
$ErrorActionPreference = 'Stop'

if ($Endpoint -notmatch '^\{[0-9a-fA-F-]{36}\}$') { throw "not an endpoint GUID: $Endpoint" }
$Key = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture\$Endpoint\FxProperties"
if (-not (Test-Path $Key)) { throw "no FxProperties for $Endpoint" }

$Fx = '{d04e05a6-594b-4fb6-a80d-01af5eed7d1d}'
$Names = @{ SFX = "$Fx,5"; MFX = "$Fx,6" }
$Ours = @{ SFX = '{77069DD4-922E-4A92-8B31-EA606FEDCE4B}'; MFX = '{B1392A38-EBF1-43A8-B2BB-0EC0E7E87D37}' }
$BackupDir = Join-Path $env:LOCALAPPDATA 'winfx-dev\fx-backup'
$Backup = Join-Path $BackupDir "$Endpoint.json"

# PowerShell's Set-ItemProperty asks for more than SetValue (which is all Users
# have here), so write through .NET with exactly the rights granted.
function Open-Fx {
    $sub = $Key.Substring('HKLM:\'.Length)
    [Microsoft.Win32.Registry]::LocalMachine.OpenSubKey($sub, [Microsoft.Win32.RegistryKeyPermissionCheck]::ReadWriteSubTree, [System.Security.AccessControl.RegistryRights]'SetValue, QueryValues')
}

function Show {
    $p = Get-ItemProperty -Path $Key
    foreach ($s in 'SFX', 'MFX') {
        $v = $p.($Names[$s])
        $mark = if ($v -eq $Ours[$s]) { '  <- WinFX' } else { '' }
        "{0} {1} = {2}{3}" -f $Endpoint, $s, $v, $mark
    }
}

switch ($Mode) {
    'show' { Show }
    'attach' {
        New-Item -ItemType Directory -Force -Path $BackupDir | Out-Null
        if (-not (Test-Path $Backup)) {
            $p = Get-ItemProperty -Path $Key
            $orig = [ordered]@{ SFX = $p.($Names.SFX); MFX = $p.($Names.MFX) }
            $orig | ConvertTo-Json | Set-Content -Path $Backup -Encoding ascii
        }
        $k = Open-Fx
        $k.SetValue($Names[$Slot], $Ours[$Slot], [Microsoft.Win32.RegistryValueKind]::String)
        $k.Close()
        Show
    }
    'detach' {
        if (-not (Test-Path $Backup)) { throw "no backup for $Endpoint" }
        $orig = Get-Content $Backup -Raw | ConvertFrom-Json
        $k = Open-Fx
        foreach ($s in 'SFX', 'MFX') {
            if ($null -ne $orig.$s) {
                $k.SetValue($Names[$s], $orig.$s, [Microsoft.Win32.RegistryValueKind]::String)
            } else {
                $k.DeleteValue($Names[$s], $false)
            }
        }
        $k.Close()
        Show
    }
}
