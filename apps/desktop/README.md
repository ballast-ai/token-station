# Token Station Desktop

Use this command to start the official desktop development environment:

```bash
npm ci
npm run tauri:dev
```

The command first builds five official WASI adapters from the lock file. It
then starts Tauri with the `bundled-plugins` feature. The development app uses
the same built-in plugins as the release package. It does not require manual
copies in the AppData plugin directory.

Do not run `npx tauri dev` directly. A normal Cargo development build does not
enable the built-in plugin feature. It cannot verify the plugin set in the
release app. Run `npm run dev` only for frontend development.

Install the complete toolchain before you start. On Windows, also install the
required Tauri dependencies. Run all project gates before you submit changes.
