; Kaseer per-user installer (Inno Setup 6). Built by scripts/package-windows.sh.
#ifndef AppVersion
  #error Pass /DAppVersion=X.Y.Z
#endif
#ifndef Arch
  #define Arch "x64"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\dist\Kaseer"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif
#ifndef OutputName
  #define OutputName "Kaseer-Setup-" + Arch
#endif

[Setup]
AppId={{9B1E4C27-5D3A-4F8E-A6C1-2E7D0B4F8A31}
AppName=Kaseer
AppVersion={#AppVersion}
AppVerName=Kaseer {#AppVersion}
AppPublisher=Dzulfikar Ats Tsauri
AppPublisherURL=https://kaseer.site
AppSupportURL=https://github.com/dzulfikar08/kaseer-rs/issues
AppCopyright=Copyright (C) Dzulfikar Ats Tsauri. AGPL-3.0-or-later.
VersionInfoVersion={#AppVersion}
DefaultDirName={autopf}\Kaseer
DefaultGroupName=Kaseer
DisableProgramGroupPage=yes
DisableDirPage=auto
DisableReadyPage=yes
PrivilegesRequired=lowest
#if Arch == "arm64"
ArchitecturesAllowed=arm64
ArchitecturesInstallIn64BitMode=arm64
#else
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
#endif
MinVersion=10.0.17763
OutputDir={#OutputDir}
OutputBaseFilename={#OutputName}
SetupIconFile=kaseer.ico
UninstallDisplayIcon={app}\kaseer.exe
UninstallDisplayName=Kaseer
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs

[Icons]
Name: "{autoprograms}\Kaseer"; Filename: "{app}\kaseer.exe"
Name: "{autodesktop}\Kaseer"; Filename: "{app}\kaseer.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\kaseer.exe"; Description: "{cm:LaunchProgram,Kaseer}"; Flags: nowait postinstall skipifsilent
