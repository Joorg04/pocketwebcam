Pocket Webcam for Windows - test build
=======================================

1. Keep this whole folder together (the .dll files and the "gstreamer" folder are needed).
2. Double-click pocketwebcam-app.exe.
   - Windows may say "Windows protected your PC" because the test build isn't signed yet:
     click "More info", then "Run anyway".
   - When Windows Firewall asks, allow access on private networks. Without it, the app
     can't find your phone on the Wi-Fi.
3. Open Pocket Webcam on your phone, on the same Wi-Fi. The computer finds it by itself.

4. The virtual camera (so Zoom, Teams, Discord and browsers can use your phone):
   right-click install-camera.cmd and choose "Run as administrator" (once). Then, in
   Pocket Webcam, press "Start virtual camera". In your video app, pick
   "Pocket Webcam (Windows Virtual Camera)". While Pocket Webcam isn't running, the
   camera shows the yellow lens logo.
   To remove it again: right-click uninstall-camera.cmd, "Run as administrator".

Not in this test build yet: the microphone, and PCs with an ARM processor (Snapdragon).
On those the app runs, but Windows can't load the virtual camera.

If something doesn't work, email support@pocketwebcam.com and attach the app's log:
right-click the Pocket Webcam icon near the clock, choose "Open log folder", and send
pocketwebcam.log. It lists what connected and what went wrong; no pictures or sound.

pocketwebcam.exe is the terminal version (run it from a Command Prompt).
