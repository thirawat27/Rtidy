; Inno Setup script for rtidy — per-user install, no admin needed.
; Build: set RTIDY_VERSION, then:  iscc packaging\windows\rtidy.iss

#define AppVersion GetEnv("RTIDY_VERSION")
#if AppVersion == ""
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{8C1F3A52-5B7E-4E1C-9C55-2F6A3D7B9E10}
AppName=rtidy
AppVersion={#AppVersion}
AppVerName=rtidy {#AppVersion}
AppPublisher=thirawat27
AppComments=Keeps your Rust target folders tidy
DefaultDirName={localappdata}\Programs\rtidy
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
OutputDir=..\..\dist
OutputBaseFilename=rtidy-setup-x64
SetupIconFile=..\..\assets\icon.ico
UninstallDisplayIcon={app}\rtidy.exe
UninstallDisplayName=rtidy
WizardStyle=modern
WizardImageFile=wizard-large.bmp,wizard-large-250.bmp
WizardSmallImageFile=wizard-small.bmp,wizard-small-250.bmp
Compression=lzma2/max
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
ChangesEnvironment=yes
CloseApplications=yes

[Messages]
WelcomeLabel2=This will install rtidy {#AppVersion}.%n%nrtidy stops Rust builds from eating your disk: one shared target folder, lean builds and a daily clean-up — set up in one click.

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; Flags: unchecked
Name: "addtopath"; Description: "Add rtidy to &PATH (use ""rtidy"" from any terminal)"

[Files]
Source: "..\..\target\release\rtidy.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
; Same path the app itself checks (Start Menu\Programs\rtidy.lnk)
Name: "{userprograms}\rtidy"; Filename: "{app}\rtidy.exe"; Comment: "Keeps your Rust target folders tidy"
Name: "{userdesktop}\rtidy"; Filename: "{app}\rtidy.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\rtidy.exe"; Description: "Open rtidy and run the setup wizard"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; remove the daily task before files go away
Filename: "{app}\rtidy.exe"; Parameters: "uninstall"; Flags: runhidden waituntilterminated; RunOnceId: "RemoveDailyTask"

[Code]
const EnvKey = 'Environment';

function PathHasDir(Dir: string): Boolean;
var Path: string;
begin
  if not RegQueryStringValue(HKCU, EnvKey, 'Path', Path) then Path := '';
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Path) + ';') > 0;
end;

procedure AddToPath(Dir: string);
var Path: string;
begin
  if PathHasDir(Dir) then exit;
  if not RegQueryStringValue(HKCU, EnvKey, 'Path', Path) then Path := '';
  if (Path <> '') and (Copy(Path, Length(Path), 1) <> ';') then Path := Path + ';';
  RegWriteExpandStringValue(HKCU, EnvKey, 'Path', Path + Dir);
end;

procedure RemoveFromPath(Dir: string);
var Path: string; P: Integer;
begin
  if not RegQueryStringValue(HKCU, EnvKey, 'Path', Path) then exit;
  Path := ';' + Path + ';';
  P := Pos(';' + Uppercase(Dir) + ';', Uppercase(Path));
  if P = 0 then exit;
  Delete(Path, P, Length(Dir) + 1);
  Path := Copy(Path, 2, Length(Path) - 2);
  RegWriteExpandStringValue(HKCU, EnvKey, 'Path', Path);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('addtopath') then
    AddToPath(ExpandConstant('{app}'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
end;
