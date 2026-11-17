# Packaging a release

Run these commands from the repository root on the Mac used for signing. The package uses the version from Cargo.toml and includes the architecture in the filename. Build the Apple Silicon release on an Apple Silicon Mac.

For a local packaging check, run `./package.sh --preview`. The resulting `build/*-preview.dmg` uses an ad hoc signature and is only for local testing. Run `tests/PackageTests.sh build/*-preview.dmg` with a single image to mount it, copy the app, verify the signature, and run the packaged helper and sample overlay.

## One-time Apple setup

1. Create or import a **Developer ID Application** certificate and its matching private key in the login Keychain. Run `security find-identity -v -p codesigning` and copy the full identity name. [Apple’s certificate instructions](https://developer.apple.com/help/account/certificates/create-developer-id-certificates/)
2. Run `xcrun notarytool store-credentials cc-sessions-viewer` and enter your credentials at the interactive prompts. This saves the notarization profile in Keychain. [Apple’s notarization guide](https://developer.apple.com/documentation/security/customizing-the-notarization-workflow)

## Build and publish

Set the identity to the exact name from Keychain, then run:

```sh
export SIGN_IDENTITY='Developer ID Application: Your Name (TEAMID)'
./package.sh
```

The script signs the bundled helper, app, and disk image, submits the DMG to Apple, staples the accepted ticket, and checks Gatekeeper and disk-image integrity before moving the finished DMG into `build/`. Set `NOTARY_PROFILE` if you used a different Keychain profile name.

Open the finished DMG on a clean Mac, drag the app to Applications, launch it, and check that selecting a session focuses the terminal after you grant Automation permission. In the GitHub release draft for the same version and commit, attach **only the notarized DMG**, then publish the draft and share its release page.
