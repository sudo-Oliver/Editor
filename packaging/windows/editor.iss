; The Windows installer, built with Inno Setup: iscc -DVersion=0.1.0 -DSource=<folder> editor.iss
; It installs for the user alone, so it needs no administrator, and the
; updater can run it again silently.

#ifndef Version
  #define Version "0.0.0"
#endif
#ifndef Source
  #define Source "..\..\target\windows"
#endif

[Setup]
AppId={{7E3F4A52-6C1B-4E0D-9B8A-2D5C9E1F0A13}
AppName=Editor
AppVersion={#Version}
AppPublisher=sudo-Oliver
AppPublisherURL=https://github.com/sudo-Oliver/Editor
AppSupportURL=https://github.com/sudo-Oliver/Editor/issues
DefaultDirName={localappdata}\Programs\Editor
DefaultGroupName=Editor
DisableProgramGroupPage=yes
DisableDirPage=yes
DisableReadyPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
ChangesAssociations=yes
ChangesEnvironment=yes
OutputBaseFilename=Editor-{#Version}-windows-x64-setup
OutputDir={#Source}\..
SetupIconFile=AppIcon.ico
UninstallDisplayIcon={app}\editor.exe
WizardStyle=modern
Compression=lzma2
SolidCompression=yes
CloseApplications=yes
RestartApplications=no

[Files]
Source: "{#Source}\editor.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Source}\pdfium.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "editor.cmd"; DestDir: "{app}\bin"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\Editor"; Filename: "{app}\editor.exe"
Name: "{userdesktop}\Editor"; Filename: "{app}\editor.exe"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "A shortcut on the desktop"; Flags: unchecked

[Registry]
; The file types the editor opens, for this user.
Root: HKCU; Subkey: "Software\Classes\Editor.Document"; ValueType: string; ValueData: "Editor document"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Editor.Document\DefaultIcon"; ValueType: string; ValueData: "{app}\editor.exe,0"
Root: HKCU; Subkey: "Software\Classes\Editor.Document\shell\open\command"; ValueType: string; ValueData: """{app}\editor.exe"" ""%1"""
Root: HKCU; Subkey: "Software\Classes\.tex\OpenWithProgids"; ValueType: string; ValueName: "Editor.Document"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.typ\OpenWithProgids"; ValueType: string; ValueName: "Editor.Document"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.bib\OpenWithProgids"; ValueType: string; ValueName: "Editor.Document"; ValueData: ""; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\Classes\.md\OpenWithProgids"; ValueType: string; ValueName: "Editor.Document"; ValueData: ""; Flags: uninsdeletevalue
; Typst files belong to nobody else yet: the editor opens them on a double click.
Root: HKCU; Subkey: "Software\Classes\.typ"; ValueType: string; ValueData: "Editor.Document"; Flags: createvalueifdoesntexist uninsdeletevalue
; A folder's context menu: "Open in Editor".
Root: HKCU; Subkey: "Software\Classes\Directory\shell\Editor"; ValueType: string; ValueData: "Open in Editor"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Directory\shell\Editor"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\editor.exe,0"
Root: HKCU; Subkey: "Software\Classes\Directory\shell\Editor\command"; ValueType: string; ValueData: """{app}\editor.exe"" ""%1"""
; The same on the empty space inside an open folder.
Root: HKCU; Subkey: "Software\Classes\Directory\Background\shell\Editor"; ValueType: string; ValueData: "Open in Editor"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\Directory\Background\shell\Editor"; ValueType: string; ValueName: "Icon"; ValueData: "{app}\editor.exe,0"
Root: HKCU; Subkey: "Software\Classes\Directory\Background\shell\Editor\command"; ValueType: string; ValueData: """{app}\editor.exe"" ""%V"""
; The `editor` command for the terminal, as on the Mac.
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}\bin"; Check: NeedsAddPath(ExpandConstant('{app}\bin'))

[Run]
Filename: "{app}\editor.exe"; Description: "Start Editor"; Flags: nowait postinstall skipifsilent

[Code]
function NeedsAddPath(Folder: string): Boolean;
var
  Path: string;
begin
  if not RegQueryStringValue(HKCU, 'Environment', 'Path', Path) then
  begin
    Result := True;
    exit;
  end;
  Result := Pos(';' + Uppercase(Folder) + ';', ';' + Uppercase(Path) + ';') = 0;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Path, Folder: string;
  Position: Integer;
begin
  if CurUninstallStep <> usPostUninstall then
    exit;
  Folder := ExpandConstant('{app}\bin');
  if RegQueryStringValue(HKCU, 'Environment', 'Path', Path) then
  begin
    Position := Pos(';' + Uppercase(Folder), Uppercase(Path));
    if Position > 0 then
    begin
      Delete(Path, Position, Length(Folder) + 1);
      RegWriteExpandStringValue(HKCU, 'Environment', 'Path', Path);
    end;
  end;
end;
