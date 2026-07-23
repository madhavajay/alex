; Alex Windows installer (Inno Setup 6).
; Per-user install: no admin, LocalAppData bin, user PATH, Task Scheduler
; daemon via `alex service install`, uninstaller reverses everything.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef AlexExe
  #define AlexExe "alex.exe"
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif
; Arch is "arm64" or "x64"; the release asset name uses x86_64 for x64.
#ifndef Arch
  #define Arch "arm64"
#endif
#if Arch == "x64"
  #define ArchTag "x86_64"
#else
  #define ArchTag "arm64"
#endif

[Setup]
AppId={{7A4E9A1C-33D6-4E1B-9B62-ALEXCTL0001}
AppName=Alex
AppVersion={#AppVersion}
AppPublisher=Madhava Jay
AppPublisherURL=https://github.com/madhavajay/alex
DefaultDirName={localappdata}\Alex
DefaultGroupName=Alex
PrivilegesRequired=lowest
OutputDir={#OutputDir}
OutputBaseFilename=Alex-Setup-{#AppVersion}-windows-{#ArchTag}
SetupIconFile=..\..\crates\alex\assets\alex.ico
UninstallDisplayIcon={app}\bin\alex.exe
Compression=lzma2
SolidCompression=yes
#if Arch == "x64"
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
#else
ArchitecturesAllowed=arm64
ArchitecturesInstallIn64BitMode=arm64
#endif
DisableProgramGroupPage=yes
ChangesEnvironment=yes

[Files]
Source: "{#AlexExe}"; DestDir: "{app}\bin"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\Alex"; Filename: "http://127.0.0.1:4100/ui/"; IconFilename: "{app}\bin\alex.exe"
Name: "{userprograms}\Alex Trace Browser"; Filename: "http://127.0.0.1:4100/ui/#traces"; IconFilename: "{app}\bin\alex.exe"

[Registry]
; Prepend the bin directory to the user PATH if not already present.
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; \
  ValueData: "{app}\bin;{olddata}"; Check: NeedsAddPath(ExpandConstant('{app}\bin'))

[Run]
Filename: "{app}\bin\alex.exe"; Parameters: "service install"; \
  StatusMsg: "Installing the Alex daemon (Task Scheduler)..."; Flags: runhidden
Filename: "http://127.0.0.1:4100/ui/"; Description: "Open the Alex web UI"; \
  Flags: postinstall shellexec skipifsilent

[UninstallRun]
Filename: "{app}\bin\alex.exe"; Parameters: "service uninstall"; \
  RunOnceId: "AlexServiceUninstall"; Flags: runhidden

[Code]
function NeedsAddPath(BinDir: string): boolean;
var
  Path: string;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Path) then
  begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Lowercase(BinDir) + ';', ';' + Lowercase(Path) + ';') = 0;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Path, BinDir: string;
  P: Integer;
begin
  if CurUninstallStep = usPostUninstall then
  begin
    BinDir := ExpandConstant('{app}\bin');
    if RegQueryStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Path) then
    begin
      P := Pos(';' + Lowercase(BinDir) + ';', ';' + Lowercase(Path) + ';');
      if P > 0 then
      begin
        Delete(Path, P, Length(BinDir) + 1);
        RegWriteExpandStringValue(HKEY_CURRENT_USER, 'Environment', 'Path', Path);
      end;
    end;
  end;
end;
