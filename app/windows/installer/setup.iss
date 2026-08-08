; RCH — Inno Setup 安装脚本
; 版本由发布工作流通过 /DMyAppVersion= 注入（去掉 tag 的 v 前缀），缺省回退。

#define MyAppName "RCH"
#ifndef MyAppVersion
#define MyAppVersion "0.4.1"
#endif
#ifndef OutputBaseFilename
#define OutputBaseFilename "RCH-" + MyAppVersion + "-windows-x64"
#endif
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
OutputBaseFilename={#OutputBaseFilename}
Compression=lzma2/max
SolidCompression=yes
PrivilegesRequired=admin
Uninstallable=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; 应用内更新依赖：静默安装时自动关闭正在运行的 RCH（随后由 [Run] 重新启动）。
CloseApplications=yes
RestartApplications=no
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
Filename: "{app}\{#MyAppExeName}"; Description: "Launch {#MyAppName}"; Flags: nowait postinstall

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
