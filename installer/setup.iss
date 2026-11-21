; Inno Setup script for PD2 Mod Manager.
; Built by .github/workflows/release.yml:  iscc /DAppVersion=1.0.0 installer\setup.iss
; Installs per-user (no admin prompt) to %LOCALAPPDATA%\Programs\PD2 Mod Manager.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

#define AppName "PD2 Mod Manager"
#define AppExe "pd2-mod-manager.exe"

[Setup]
AppId={{9EC99C6A-4772-4E78-BD41-05926B52F9D2}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=catdum200
AppPublisherURL=https://github.com/catdum200/PayDay2-mod-manager
AppSupportURL=https://github.com/catdum200/PayDay2-mod-manager/issues
AppUpdatesURL=https://github.com/catdum200/PayDay2-mod-manager/releases
DefaultDirName={localappdata}\Programs\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
LicenseFile=..\LICENSE
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
OutputDir=..\dist
OutputBaseFilename=PD2-Mod-Manager-Setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
ChangesAssociations=yes
CloseApplications=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"
Name: "assocpacks"; Description: "Open .pd2pack modpack files with {#AppName}"; GroupDescription: "File types:"

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; .pd2pack -> PD2MM.Modpack (current user only, removed on uninstall)
Root: HKA; Subkey: "Software\Classes\.pd2pack"; ValueType: string; ValueName: ""; ValueData: "PD2MM.Modpack"; Flags: uninsdeletevalue; Tasks: assocpacks
Root: HKA; Subkey: "Software\Classes\PD2MM.Modpack"; ValueType: string; ValueName: ""; ValueData: "PAYDAY 2 modpack"; Flags: uninsdeletekey; Tasks: assocpacks
Root: HKA; Subkey: "Software\Classes\PD2MM.Modpack\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExe},0"; Tasks: assocpacks
Root: HKA; Subkey: "Software\Classes\PD2MM.Modpack\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""; Tasks: assocpacks

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
