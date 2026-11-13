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
; No AppMutex: in a silent install (winget) its "WinFX is running" prompt
; answers Cancel. PrepareToInstall closes WinFX instead, and a silent install
; starts it again afterwards.
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
; A silent install or upgrade (winget) puts back a WinFX it closed.
Filename: "{app}\winfx.exe"; Parameters: "--hidden"; Flags: nowait; Check: RestartHidden

[Registry]
; Start at login is set from inside WinFX; only clean it up on uninstall.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueName: "WinFX"; ValueType: none; Flags: uninsdeletevalue dontcreatekey

[UninstallRun]
; Direct mode lives in the Windows audio engine (machine-wide): removing it needs admin.
Filename: "{app}\winfx.exe"; Parameters: "--apo-remove"; Verb: "runas"; Flags: shellexec waituntilterminated runhidden; Check: ApoInstalled; RunOnceId: "RemoveApo"

[Code]
const
  // Both names are in crates\app\src\main.rs and system.rs.
  AppMutexName = 'WinFX-single-instance';
  QuitEventName = 'Local\WinFX-quit';
  EVENT_MODIFY_STATE = $0002;

var
  WinFXWasRunning: Boolean;

function OpenEvent(dwDesiredAccess: DWORD; bInheritHandle: BOOL; lpName: String): THandle;
  external 'OpenEventW@kernel32.dll stdcall';
function SetEvent(hEvent: THandle): BOOL;
  external 'SetEvent@kernel32.dll stdcall';
function CloseHandle(hObject: THandle): BOOL;
  external 'CloseHandle@kernel32.dll stdcall';

function ApoInstalled: Boolean;
begin
  Result := RegKeyExists(HKLM64, 'SOFTWARE\Classes\CLSID\{77069DD4-922E-4A92-8B31-EA606FEDCE4B}');
end;

function WaitForWinFXExit(TimeoutMs: Integer): Boolean;
var
  Waited: Integer;
begin
  Waited := 0;
  while CheckForMutexes(AppMutexName) and (Waited < TimeoutMs) do
  begin
    Sleep(100);
    Waited := Waited + 100;
  end;
  Result := not CheckForMutexes(AppMutexName);
end;

// Ask a running WinFX to quit (it saves its settings first). 0.1.0 has no
// quit event, and closing its window only hides it in the tray, so end it if
// asking doesn't work. Direct mode keeps running either way.
function CloseWinFX: Boolean;
var
  Event: THandle;
  ResultCode: Integer;
begin
  Result := True;
  if not CheckForMutexes(AppMutexName) then
    Exit;
  WinFXWasRunning := True;
  Event := OpenEvent(EVENT_MODIFY_STATE, False, QuitEventName);
  if Event <> 0 then
  begin
    SetEvent(Event);
    CloseHandle(Event);
    if WaitForWinFXExit(10000) then
      Exit;
  end;
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM winfx.exe /FI "USERNAME eq ' + GetUserNameString + '"',
    '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Result := WaitForWinFXExit(5000);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  if CloseWinFX then
    Result := ''
  else
    Result := 'WinFX is still running. Quit it from its tray icon, then run Setup again.';
end;

function RestartHidden: Boolean;
begin
  Result := WinFXWasRunning and WizardSilent;
end;

// After the "are you sure" prompt, so cancelling leaves WinFX running.
procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    CloseWinFX;
end;
