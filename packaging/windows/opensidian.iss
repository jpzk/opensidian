; SPDX-License-Identifier: GPL-3.0-or-later
;
; opensidian: hand-written Inno Setup script for the Windows installer. No tauri bundler, no
; recompile: it packages the opensidian.exe that `cargo build --locked --release` produced (and
; that cargo test ran against), the same rule as the .deb, .rpm, AppImage and macOS .app.
; Compiled by the Inno Setup 6 already on the GitHub Windows image (windows.yml); nothing is
; downloaded at build time.
;
;   ISCC.exe /DAppVersion=0.9.6 /DAppExe=<path to opensidian.exe> /O<out dir> packaging\windows\opensidian.iss
;
; What it installs: one file, opensidian.exe (WebView2Loader is linked in), per user, no admin
; rights, in %LOCALAPPDATA%\Programs\Opensidian; a Start menu shortcut; an optional desktop
; shortcut; an entry in Settings > Apps that uninstalls it. Uninstalling removes the program and
; the shortcuts only: vaults and %USERPROFILE%\.opensidian.json are left alone.
; WebView2 ships with Windows 11 and current Windows 10; if the setup does not find it, it says
; so and lets the user go on (the app does not start without it).
; UNSIGNED: Windows SmartScreen warns before running the setup.

#ifndef AppVersion
  #error Pass the version: ISCC /DAppVersion=x.y.z
#endif
#ifndef AppExe
  #error Pass the binary: ISCC /DAppExe=<path to opensidian.exe>
#endif
#define Root AddBackslash(SourcePath) + "..\.."

[Setup]
; Never change AppId: Windows uses it to find the installed copy for upgrades and uninstall.
AppId={{19472948-A3AC-46CD-9CE1-9F5D376342C9}
AppName=Opensidian
AppVersion={#AppVersion}
AppVerName=Opensidian {#AppVersion}
AppPublisher=opensidian
AppPublisherURL=https://github.com/jpzk/opensidian
AppSupportURL=https://github.com/jpzk/opensidian/issues
AppUpdatesURL=https://github.com/jpzk/opensidian/releases
VersionInfoVersion={#AppVersion}
VersionInfoDescription=Opensidian setup
DefaultDirName={autopf}\Opensidian
DefaultGroupName=Opensidian
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
LicenseFile={#Root}\LICENSE
SetupIconFile={#Root}\src-tauri\icons\icon.ico
UninstallDisplayIcon={app}\opensidian.exe
UninstallDisplayName=Opensidian
OutputBaseFilename=opensidian-{#AppVersion}-windows-x86_64-unsigned-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#AppExe}"; DestDir: "{app}"; DestName: "opensidian.exe"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Opensidian"; Filename: "{app}\opensidian.exe"
Name: "{autodesktop}\Opensidian"; Filename: "{app}\opensidian.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\opensidian.exe"; Description: "{cm:LaunchProgram,Opensidian}"; Flags: nowait postinstall skipifsilent

[Code]
// The WebView2 Runtime registers its version under this client id (the detection Microsoft
// documents for distributing apps that use WebView2), per machine or per user.
const
  WV2 = 'Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}';

function WebView2Version(Root: Integer; Key: String): String;
begin
  if not RegQueryStringValue(Root, Key, 'pv', Result) then
    Result := '';
end;

function HasWebView2(): Boolean;
var
  V: String;
begin
  V := WebView2Version(HKLM32, 'SOFTWARE\' + WV2);
  if (V = '') or (V = '0.0.0.0') then
    V := WebView2Version(HKCU, 'Software\' + WV2);
  Result := (V <> '') and (V <> '0.0.0.0');
end;

function InitializeSetup(): Boolean;
begin
  Result := True;
  if not HasWebView2() then
    Result := SuppressibleMsgBox(
      'The Microsoft Edge WebView2 Runtime was not found. Opensidian needs it to start.' + #13#10 +
      'It ships with Windows 11; on Windows 10 it can be installed from Microsoft.' + #13#10#13#10 +
      'Install Opensidian anyway?',
      mbConfirmation, MB_YESNO, IDYES) = IDYES;
end;
