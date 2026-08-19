; Nib installer — Inno Setup.
;
; Why Inno rather than WiX: Inno Setup is free for any use including commercial, permanently, with
; no maintenance-fee model (WiX v6+ requires the paid Open Source Maintenance Fee, and v5 is
; MS-RL). It is also what VS Code ships with, so it is well-trodden for exactly this case.
;
; Deliberately a PER-USER install (PrivilegesRequired=lowest):
;   * no UAC prompt, so a curious person can try an unsigned binary without a scary elevation,
;   * everything lands under %LOCALAPPDATA%, alongside where the app already stores its model,
;   * uninstall removes only this user's copy.
;
; Ships exactly the runtime set the app needs. Only TWO DLLs are required: HoldToSpeak-engine.exe
; imports sherpa-onnx-c-api.dll, which imports onnxruntime.dll. The cxx-api and
; providers_shared DLLs in the build output are referenced by nothing we link, so they stay out.
;
; The ~460 MB speech model is NOT bundled: it is separately licensed (CC-BY-4.0) and is fetched on
; first run against a pinned SHA-256. That keeps the download small and keeps us out of the
; business of redistributing someone else's weights.

#define AppName "HoldToSpeak"
#define AppPublisher "rootMonsteR"
#ifndef AppVersion
  #define AppVersion "0.1.3"
#endif
#ifndef BinDir
  #define BinDir "..\target\release"
#endif
#ifndef DocsDir
  #define DocsDir ".."
#endif
; The app icon. Shared with the exe (embedded by nib-core's build.rs) so the installer, the
; Start-menu shortcut, Add/Remove Programs and the tray all show the same mark.
#ifndef IconFile
  #define IconFile "..\assets\HoldToSpeak.ico"
#endif

[Setup]
AppId={{8F3A5C21-9D74-4E6B-B2A8-1C5E7D0F4A93}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
DefaultDirName={localappdata}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=out
OutputBaseFilename=HoldToSpeak-{#AppVersion}-x64-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Shown in the wizard so the licence is accepted, not just shipped.
LicenseFile={#DocsDir}\LICENSE
; Surfaces the CC-BY-4.0 model attribution and the GPL-free runtime note before install.
InfoBeforeFile={#DocsDir}\THIRD-PARTY-NOTICES.md
UninstallDisplayName={#AppName}
UninstallDisplayIcon={app}\HoldToSpeak.exe
; The wizard, and its Add/Remove entry, carry the same mark as the app itself.
SetupIconFile={#IconFile}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "startupicon"; Description: "Start {#AppName} when I sign in to Windows"; GroupDescription: "Additional options:"; Flags: unchecked

[Files]
Source: "{#BinDir}\HoldToSpeak.exe";        DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\HoldToSpeak-engine.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\sherpa-onnx-c-api.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\onnxruntime.dll";       DestDir: "{app}"; Flags: ignoreversion
; Licence obligations travel with the binaries, not just the repo.
Source: "{#DocsDir}\LICENSE";                DestDir: "{app}"; Flags: ignoreversion
Source: "{#DocsDir}\THIRD-PARTY-NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#DocsDir}\PRIVACY.md";             DestDir: "{app}"; Flags: ignoreversion
Source: "{#DocsDir}\README.md";              DestDir: "{app}"; Flags: ignoreversion

[InstallDelete]
; Pre-0.1.4 installs shipped the binaries under their old internal names. Inno only cleans up
; files the CURRENT install owns, so without this they would linger beside the renamed ones.
Type: files; Name: "{app}\nib-core.exe"
Type: files; Name: "{app}\nib-asr-sidecar.exe"

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\HoldToSpeak.exe"
Name: "{userstartup}\{#AppName}"; Filename: "{app}\HoldToSpeak.exe"; Tasks: startupicon

[Run]
Filename: "{app}\HoldToSpeak.exe"; Description: "Start {#AppName} now"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; The model, dictionary and settings live outside {app}; leave them alone on uninstall so a
; reinstall does not re-download 460 MB or lose the user's learned vocabulary. Removing them is
; documented in PRIVACY.md as a manual step.
Type: files; Name: "{app}\*.log"
