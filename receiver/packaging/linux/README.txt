Pocket Webcam for Linux - test version
=======================================

Use your iPhone as a webcam on this computer, over Wi-Fi.


Install (about 5 minutes)
-------------------------
1. Unpack this folder (you've probably done that already).
2. Open a terminal in this folder and run:

       ./install.sh

   It asks for your password once. It installs the video libraries and the
   virtual camera driver with your package manager, sets up the
   "Pocket Webcam Camera", and installs the app for your user.

   Works on Ubuntu 22.04 or newer, Debian 12+, Linux Mint 21+, Pop!_OS, Arch and
   Manjaro. Fedora works too after enabling RPM Fusion (the installer tells you).
   64-bit Intel/AMD computers only.

3. On the iPhone: install "TestFlight" from the App Store, open the test
   invitation link, then install Pocket Webcam from TestFlight.


Use it
------
1. Open "Pocket Webcam" from your app launcher.
2. Open Pocket Webcam on the iPhone. Allow Camera and Local Network when asked.
   Both devices must be on the same Wi-Fi.
3. The phone shows up at the top of the desktop app; it connects by itself.
4. In Zoom, Meet, Teams, Discord, OBS or the browser, choose
   "Pocket Webcam Camera" as your camera.

Tip: in a browser, close and reopen the tab if the camera doesn't show up in the list
the first time.


If something doesn't work
-------------------------
* The phone doesn't show up: check that both are on the same Wi-Fi (not a guest
  network). Some routers block discovery; then use "Connect by IP address" in the
  phone menu at the top of the desktop app. The phone shows its address in the app.
  If you use a firewall (ufw, firewalld), allow mDNS (UDP port 5353).

* The phone shows up but doesn't connect by itself: only the phone you used last
  does that (so the app never grabs somebody else's phone on a shared network).
  Pick the other phone from the menu at the top once, and it becomes the one.

* If the camera doesn't load ("Other apps can't see your phone yet"):
  - Secure Boot: on Ubuntu, the driver must be approved once. During install you may
    have been asked to choose a password; after the next restart a blue screen
    ("Perform MOK management") asks for it: choose Enroll MOK, Continue, enter the
    password, Reboot. Then run:  sudo ./setup-v4l2loopback.sh
  - Just updated the kernel? Restart the computer, then run:
    sudo ./setup-v4l2loopback.sh
  - "v4l2loopback already has settings that Pocket Webcam didn't write": you (or a
    guide for OBS's virtual camera) set the driver up by hand before. The message
    shows how to add the Pocket Webcam camera to those settings; nothing is changed
    for you, so your other virtual camera keeps working.

* Anything else: send feedback from the TestFlight app on the phone ("Send Beta
  Feedback"), or email support@pocketwebcam.com. Screenshots help a lot, and so does
  the app's log: right-click the tray icon, choose "Open log folder", and attach
  pocketwebcam.log (it's in ~/.local/state/pocketwebcam). It lists what connected
  and what went wrong; no pictures or sound.


Remove it
---------
    ./uninstall.sh

Your recordings and snapshots (Videos and Pictures › Pocket Webcam) are kept.


Thank you for testing!
