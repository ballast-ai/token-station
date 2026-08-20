# Install Token Station

1. Drag `token-station.app` to `Applications`.
2. Open it from Applications. If macOS blocks it, right-click the App and choose **Open**.

If it still will not open, paste this line into Terminal:

```bash
sudo xattr -dr com.apple.quarantine "/Applications/token-station.app" && open "/Applications/token-station.app"
```

This preview is unsigned and not notarized. Verify its SHA-256 and use the command only for a copy from the official GitHub Release. Terminal hides password characters while you type.
