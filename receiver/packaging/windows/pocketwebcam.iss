; Pocket Webcam installer (Inno Setup 6). CI builds it from the packaged folder:
;   iscc /DAppVersion=0.1.0 /DDistDir=<repo>\dist\PocketWebcam pocketwebcam.iss
; One admin prompt installs the app, registers the camera DLL, adds a firewall rule for
; phone discovery and a Start menu entry. The uninstaller undoes all of it.

#ifndef AppVersion
  #define AppVersion "0.0.0-dev"
#endif
#ifndef DistDir
  #define DistDir "..\..\..\dist\PocketWebcam"
#endif

[Setup]
; Never change AppId: Windows uses it to find the installed app for upgrades and uninstall.
AppId={{2F36B8F5-E020-4688-988D-34AE05E389AC}
AppName=Pocket Webcam
AppVersion={#AppVersion}
AppPublisher=Pocket Webcam
AppPublisherURL=https://pocketwebcam.com
AppSupportURL=https://pocketwebcam.com/help
; The camera DLL must live outside user profiles: Windows' camera service loads it.
DefaultDirName={autopf}\Pocket Webcam
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=admin
; Intel and AMD PCs only, not ARM (Snapdragon) ones: Windows' camera service is an ARM
; program there and can't load our x64 camera DLL, so the camera would never work.
; Change back to x64compatible once an ARM64 build of pocketwebcam_camera.dll ships.
ArchitecturesAllowed=x64os
ArchitecturesInstallIn64BitMode=x64os
; The virtual camera API needs Windows 11.
MinVersion=10.0.22000
CloseApplications=yes
RestartApplications=no
UsedUserAreasWarning=no
SetupIconFile=pocketwebcam.ico
UninstallDisplayIcon={app}\pocketwebcam.ico
UninstallDisplayName=Pocket Webcam
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
OutputDir=output
OutputBaseFilename=PocketWebcam-Setup-{#AppVersion}

[Tasks]
Name: desktopicon; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
; The zip's manual camera scripts and README aren't needed: the installer does their job.
Source: "{#DistDir}\*"; DestDir: "{app}"; Excludes: "pocketwebcam_camera.dll,install-camera.cmd,uninstall-camera.cmd,README.txt"; Flags: ignoreversion recursesubdirs createallsubdirs
; regserver: registers the camera (regsvr32) and unregisters it on uninstall. Windows'
; camera service may have the DLL open, so replace or delete it at the next restart if needed.
Source: "{#DistDir}\pocketwebcam_camera.dll"; DestDir: "{app}"; Flags: ignoreversion regserver restartreplace uninsrestartdelete
Source: "pocketwebcam.ico"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Pocket Webcam"; Filename: "{app}\pocketwebcam-app.exe"; IconFilename: "{app}\pocketwebcam.ico"
Name: "{autodesktop}\Pocket Webcam"; Filename: "{app}\pocketwebcam-app.exe"; IconFilename: "{app}\pocketwebcam.ico"; Tasks: desktopicon

[Registry]
; Created by the app's "Start when I log in" setting; removed on uninstall.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueName: "Pocket Webcam"; ValueType: none; Flags: uninsdeletevalue dontcreatekey

[Run]
; Phone discovery (mDNS) needs incoming UDP 5353; the video connection to the phone is
; outgoing and needs no rule. Delete first so upgrades don't add duplicates.
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""Pocket Webcam"""; Flags: runhidden
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall add rule name=""Pocket Webcam"" dir=in action=allow program=""{app}\pocketwebcam-app.exe"" protocol=UDP localport=5353 profile=private,domain"; Flags: runhidden
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall add rule name=""Pocket Webcam"" dir=in action=allow program=""{app}\pocketwebcam.exe"" protocol=UDP localport=5353 profile=private,domain"; Flags: runhidden
Filename: "{app}\pocketwebcam-app.exe"; Description: "Open Pocket Webcam"; Flags: postinstall nowait skipifsilent runasoriginaluser

[UninstallRun]
; The uninstaller doesn't close running apps (Setup does): a copy left in the tray keeps its
; files, and the folder stays behind.
Filename: "{sys}\taskkill.exe"; Parameters: "/f /im pocketwebcam-app.exe /im pocketwebcam.exe"; Flags: runhidden; RunOnceId: "CloseApp"
; Before the DLL goes: take the camera out of Windows' camera list.
Filename: "{app}\pocketwebcam.exe"; Parameters: "--remove-camera"; Flags: runhidden; RunOnceId: "RemoveCamera"
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""Pocket Webcam"""; Flags: runhidden; RunOnceId: "RemoveFirewallRule"

[UninstallDelete]
; GStreamer's plugin cache and the lock file. Settings in %APPDATA% are kept.
Type: filesandordirs; Name: "{localappdata}\Pocket Webcam"
