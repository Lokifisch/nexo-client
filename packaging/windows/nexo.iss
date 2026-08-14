; Windows installer for Nexo, built with Inno Setup 6.
;
; Sits alongside packaging/debian and packaging/aur; the Makefile is Unix-only
; and deliberately has nothing to do with this.
;
; Build it after `cargo build --release`:
;
;   iscc packaging\windows\nexo.iss
;
; or from Linux, against a cross-compiled binary:
;
;   wine .../ISCC.exe packaging/windows/nexo.iss "/DBinDir=target/x86_64-pc-windows-msvc/release"
;
; The version is never written here. It is read out of the built nexo.exe,
; which gets it from Cargo.toml via crates/nexo-app/build.rs — so there is one
; source of truth and no packaging script to forget to bump.

#ifndef BinDir
  #define BinDir "..\..\target\release"
#endif

#define Exe BinDir + "\nexo.exe"

#ifnexist Exe
  #error Build the release binary first: cargo build --release
#endif

#define AppName "Nexo"
#define AppVersion GetVersionNumbersString(Exe)
#define Publisher "Lokifisch"
#define AppUrl "https://github.com/Lokifisch/nexo-client"

[Setup]
; Never change AppId. It is the key Windows matches an upgrade against;
; a new one makes the next version install *beside* this one instead of
; over it, leaving two Nexos in the Start menu and two uninstall entries.
AppId={{8F3C6B41-2D9E-4A57-9C18-6E4B0D7A52F3}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#Publisher}
AppPublisherURL={#AppUrl}
AppSupportURL={#AppUrl}/issues
AppUpdatesURL={#AppUrl}/releases
VersionInfoVersion={#AppVersion}

; Per-user install, so there is no UAC prompt and no admin account needed.
; This is how Discord and the mainstream Minecraft launchers install, and it
; is what lets Nexo replace its own binary later — see
; crates/nexo-core/src/self_update.rs, which refuses to update an install it
; does not own.
PrivilegesRequired=lowest
DefaultDirName={autopf}\{#AppName}
DisableProgramGroupPage=yes
DefaultGroupName={#AppName}

; x64 only. There is no 32-bit build, and without this the installer would
; happily run on a system it has no binary for.
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

; Shut Nexo down before overwriting it, and restart it afterwards, rather
; than failing the update because the .exe is in use.
CloseApplications=yes
RestartApplications=yes

Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
SetupIconFile=..\..\assets\nexo.ico
UninstallDisplayIcon={app}\nexo.exe
UninstallDisplayName={#AppName} {#AppVersion}
LicenseFile=..\..\LICENSE
OutputDir=..\..\dist
OutputBaseFilename=Nexo-Setup-{#AppVersion}

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "de"; MessagesFile: "compiler:Languages\German.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#Exe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\nexo.exe"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\nexo.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\nexo.exe"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; The binary Windows forced a self-update to leave behind, if Nexo was
; updated but never restarted. `clear_previous` in self_update.rs normally
; removes it at the next start; this covers uninstalling before that happens.
Type: files; Name: "{app}\nexo.exe.old"

; Deliberately absent: anything under {userappdata}\Nexo. That is instances,
; worlds, accounts and the shared asset cache — potentially many GB, and
; reinstalling is a completely normal reason to run the uninstaller. Game data
; is not the installer's to delete.
