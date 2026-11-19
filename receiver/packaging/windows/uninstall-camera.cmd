@echo off
rem Removes the Pocket Webcam virtual camera installed by install-camera.cmd.
rem Right-click and choose "Run as administrator". Close apps that use the camera first.
net session >nul 2>&1 || (echo Please right-click uninstall-camera.cmd and choose "Run as administrator". & pause & exit /b 1)
set "DLL=%ProgramFiles%\Pocket Webcam\pocketwebcam_camera.dll"
rem Take the camera out of Windows' camera list first, or a dead entry stays behind.
"%~dp0pocketwebcam.exe" --remove-camera >nul 2>&1
if exist "%DLL%" regsvr32 /u /s "%DLL%"
if exist "%DLL%" del "%DLL%" || (echo Could not remove the camera file. Close apps that use the camera and try again. & pause & exit /b 1)
echo The Pocket Webcam camera is removed.
pause
