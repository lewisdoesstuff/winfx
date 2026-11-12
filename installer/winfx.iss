; WinFX installer (Inno Setup 6). Per-user: no admin rights needed.
; Build with tools\build-release.ps1, which passes the version and paths.

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\target\release"
#endif

[Setup]
AppId={{6F1C2D4B-9E7A-4C31-8B52-3A9D0E7F1C64}
AppName=WinFX
AppVersion={#AppVersion}
AppVerName=WinFX {#AppVersion}
AppPublisher=WinFX
DefaultDirName={localappdata}\Programs\WinFX
DisableDirPage=auto
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0.17763
OutputDir=..\dist
OutputBaseFilename=WinFX-{#AppVersion}-setup
SetupIconFile=..\crates\app\res\winfx.ico
UninstallDisplayIcon={app}\winfx.exe
UninstallDisplayName=WinFX
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
; The app's single-instance mutex: setup asks to close a running WinFX.
AppMutex=WinFX-single-instance
CloseApplications=yes
LicenseFile=..\LICENSE

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\winfx.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\winfx_apo.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"
Source: "..\README.md"; DestDir: "{app}"
Source: "..\THIRD-PARTY-NOTICES.html"; DestDir: "{app}"

[Icons]
Name: "{autoprograms}\WinFX"; Filename: "{app}\winfx.exe"
Name: "{autodesktop}\WinFX"; Filename: "{app}\winfx.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\winfx.exe"; Description: "{cm:LaunchProgram,WinFX}"; Flags: nowait postinstall skipifsilent

[Registry]
; Start at login is set from inside WinFX; only clean it up on uninstall.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueName: "WinFX"; ValueType: none; Flags: uninsdeletevalue dontcreatekey

[UninstallRun]
; Direct mode lives in the Windows audio engine (machine-wide): removing it needs admin.
Filename: "{app}\winfx.exe"; Parameters: "--apo-remove"; Verb: "runas"; Flags: shellexec waituntilterminated runhidden; Check: ApoInstalled; RunOnceId: "RemoveApo"

[Code]
function ApoInstalled: Boolean;
begin
  Result := RegKeyExists(HKLM64, 'SOFTWARE\Classes\CLSID\{77069DD4-922E-4A92-8B31-EA606FEDCE4B}');
end;
