; RCH v0.3.5 — Inno Setup 安装脚本

#define MyAppName "RCH"
#define MyAppVersion "0.3.5"
#define MyAppPublisher "RCH"
#define MyAppURL "https://github.com/ChangfengluoO71/RCH"
#define MyAppExeName "RCH.exe"
#define MySourceDir "..\..\build\windows\x64\runner\Release"

[Setup]
AppId={{E7D3A1B2-5F8C-4A1D-9E2B-6A7C8D9E0F1A}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
AppUpdatesURL={#MyAppURL}
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
OutputDir=..\..\..\dist
OutputBaseFilename=RCH-v0.3.5-windows-x64
Compression=lzma2/max
SolidCompression=yes
PrivilegesRequired=admin
Uninstallable=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
WizardStyle=modern

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional:"

[Files]
Source: "{#MySourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\Uninstall {#MyAppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "Launch {#MyAppName}"; Flags: nowait postinstall skipifsilent

[Code]
function InitializeSetup: Boolean;
begin
  if not IsWin64 then
  begin
    MsgBox('RCH requires 64-bit Windows 10/11.', mbCriticalError, MB_OK);
    Result := False;
    Exit;
  end;
  Result := True;
end;
