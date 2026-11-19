@echo off
rem Installs the Pocket Webcam virtual camera (test builds; the installer will do this later).
rem Windows only loads cameras from a protected folder, so this needs administrator rights:
rem right-click install-camera.cmd and choose "Run as administrator".
net session >nul 2>&1 || (echo Please right-click install-camera.cmd and choose "Run as administrator". & pause & exit /b 1)
set "DEST=%ProgramFiles%\Pocket Webcam"
if not exist "%DEST%" mkdir "%DEST%"
copy /y "%~dp0pocketwebcam_camera.dll" "%DEST%\" >nul || (echo Could not copy the camera. If an app is using it, close that app and try again. & pause & exit /b 1)
regsvr32 /s "%DEST%\pocketwebcam_camera.dll" || (echo Registering the camera failed. & pause & exit /b 1)
echo The Pocket Webcam camera is installed. In Pocket Webcam, press "Start virtual camera".
pause
