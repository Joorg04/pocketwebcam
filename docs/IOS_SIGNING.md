# iOS signing (no Mac needed)

The iPhone app is signed in GitHub Actions with the paid Apple Developer account
(individual, Team ID `3Q86L64LTN`). Everything is done from Linux.

## What exists (created 2026-10-20, all expire 2027-10-20)

| Item | Where |
|---|---|
| Apple Distribution certificate | developer.apple.com → Certificates |
| App ID `com.pocketwebcam.ios` | developer.apple.com → Identifiers (no capabilities) |
| Registered test iPhone (its UDID) | developer.apple.com → Devices |
| Profile "Pocket Webcam Ad Hoc" (Distribution, that iPhone) | developer.apple.com → Profiles |
| Profile "Pocket Webcam App Store" (App Store Connect) | developer.apple.com → Profiles |

Local copies live in `~/keys/apple/` (never commit them; back them up):
`distribution.key` (private key), `distribution.csr`, `distribution.cer`, `distribution.p12` +
`distribution.p12.password`, and both `.mobileprovision` files.

## GitHub secrets (repo `Joorg04/pocketwebcam`)

| Secret | Content |
|---|---|
| `IOS_DIST_P12` | `distribution.p12`, base64 |
| `IOS_DIST_P12_PASSWORD` | contents of `distribution.p12.password` |
| `IOS_PROFILE_ADHOC` | Ad Hoc `.mobileprovision`, base64 |
| `IOS_PROFILE_APPSTORE` | App Store `.mobileprovision`, base64 |

For TestFlight uploads (App Store Connect API key "GitHub Actions", App Manager, created
2026-10-20; the `.p8` file can only be downloaded once, so keep a backup):

| Secret | Content |
|---|---|
| `ASC_KEY_ID` | the key ID shown in App Store Connect → Users and Access → Integrations |
| `ASC_ISSUER_ID` | the issuer ID shown on the same page |
| `ASC_KEY_P8` | the `.p8` file, base64 |

Set them by piping from the files so values never reach the screen:

```bash
cd ~/keys/apple; R=Joorg04/pocketwebcam
base64 -w0 distribution.p12 | gh secret set IOS_DIST_P12 -R $R
gh secret set IOS_DIST_P12_PASSWORD -R $R < distribution.p12.password
base64 -w0 Pocket_Webcam_Ad_Hoc.mobileprovision | gh secret set IOS_PROFILE_ADHOC -R $R
base64 -w0 Pocket_Webcam_App_Store.mobileprovision | gh secret set IOS_PROFILE_APPSTORE -R $R
```

## Build and install on the iPhone

```bash
gh workflow run "iOS build (signed)" -R Joorg04/pocketwebcam
gh run list -R Joorg04/pocketwebcam -w "iOS build (signed)" -L 1      # wait for success
gh run download <id> -R Joorg04/pocketwebcam -n PocketWebcam-adhoc-ipa
ideviceinstaller install PocketWebcam.ipa                             # iPhone unlocked, "Trust" accepted
```

`ideviceinstaller` comes from the AUR: `yay -S ideviceinstaller-git` (run in a real terminal; yay
asks questions). The "could not locate ...sinf" warning is normal for non-App-Store builds.
"Mux error -8" means the phone is locked: unlock it and re-plug the cable.

The build number (`CFBundleVersion`) is the workflow run number, so every upload to
TestFlight is unique. `MARKETING_VERSION` in `ios/project.yml` is the user-facing version.

## TestFlight

App record "Pocket Webcam" (SKU `pocketwebcam-ios`) exists in App Store Connect. To upload:

```bash
gh workflow run "iOS build (signed)" -R Joorg04/pocketwebcam -f testflight=true
```

The build appears under TestFlight after Apple processes it (5-30 minutes). Each run's
number is the build number, so uploads never clash.

## Adding a tester's iPhone (Ad Hoc)

Register its UDID under Devices, edit the "Pocket Webcam Ad Hoc" profile to include it,
download it again, re-set `IOS_PROFILE_ADHOC`, rebuild. (TestFlight needs no UDIDs.)

## Renewal (before 2027-10-20)

Make a new CSR + key (`openssl genrsa` / `openssl req`), create a new Apple Distribution
certificate, regenerate both profiles, rebuild the `.p12` with
`-certpbe PBE-SHA1-3DES -keypbe PBE-SHA1-3DES -macalg sha1` (macOS keychain can't import
OpenSSL 3's default AES p12), and re-set all four secrets.
