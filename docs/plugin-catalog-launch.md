# Launching the plugin catalog (#785)

The app fetches the signed plugin catalog from `https://banditf.github.io/foxglove/plugins/v2/`
(`root.json`, `catalog.json` and their `.minisig` files; see [local-api.md](local-api.md#signing-159)).
The workflow `.github/workflows/plugin-catalog.yml` ("Plugin catalog" in the Actions tab) signs
`plugins/catalog/catalog.json` and publishes those four files.

## Until these steps are done, it does nothing

Each run starts with a gate job. It ends the run, green and with a notice, unless all of these hold:

- the `FOXGLOVE_CATALOG_KEY` secret exists;
- GitHub Pages is on, with **Source: GitHub Actions**;
- `app/resources/plugin-trust/root.pub` is a real key, not the placeholder, and `root.json` next to
  it is signed.

It runs by hand ("Run workflow"), when a push to `main` changes `plugins/catalog/`, and on a schedule
on the 1st and 15th of each month at 06:17 UTC, which re-signs the catalog before it expires. The
same gate applies to all three. The signing job never sees the root key; it only has the catalog key, which it reads from
the secret into a private temp file and deletes when the step ends.

## Two keys

- The **root key** stays offline, always. It signs `root.json`, which names the catalog key. Its
  public half is pinned in the app (`root.pub`).
- The **catalog key** signs `catalog.json`. It goes into the repository's Actions secrets. If it
  ever leaks, a new `root.json` (signed offline) names a new one, with no app release.

## The user's steps

Run the commands in PowerShell, in `C:\Users\fuchs\splitkeeb\foxglove`, on an up-to-date `main`.
`E:\foxglove-keys` stands for a folder on a USB stick; any folder outside the repository works.

### 1. Turn on GitHub Pages

1. Open https://github.com/BanditF/foxglove/settings/pages.
2. Under **Build and deployment**, set **Source** to **GitHub Actions**.

### 2. Make the keys offline

1. While still online, build the tool once:
   `cargo build -p foxglove-plugin-trust --features sign --bin plugin-sign`
2. Turn off the network (Wi-Fi off or cable out).
3. Make the root key. It asks for a password twice; keep that password somewhere safe:
   `cargo run --offline -q -p foxglove-plugin-trust --features sign --bin plugin-sign -- keygen E:\foxglove-keys root`
4. Make the catalog key, without a password (the Actions secret protects it):
   `cargo run --offline -q -p foxglove-plugin-trust --features sign --bin plugin-sign -- keygen E:\foxglove-keys catalog --no-password`
5. Sign `root.json` and pin the root key in the app. It asks for the root key's password:
   `cargo run --offline -q -p foxglove-plugin-trust --features sign --bin plugin-sign -- root --key E:\foxglove-keys\root.key --catalog-pub E:\foxglove-keys\catalog.pub --out app\resources\plugin-trust --pin app\resources\plugin-trust\root.pub`
6. Turn the network back on.
7. Hand the three changed files to the Hub to commit (`app/resources/plugin-trust/root.pub`,
   `root.json`, `root.json.minisig`). They're public. The `.key` files never go in the repository.

`root.json` lasts two years (`--days 730`). Re-sign it the same way, offline, before then.

### 3. Add the signing secret

1. Copy the catalog key to the clipboard:
   `Get-Content E:\foxglove-keys\catalog.key -Raw | Set-Clipboard`
2. Open https://github.com/BanditF/foxglove/settings/secrets/actions and click **New repository secret**.
3. **Name:** `FOXGLOVE_CATALOG_KEY`. **Secret:** paste (both lines). Click **Add secret**.
4. Clear the clipboard: `Set-Clipboard -Value ' '`

If the catalog key was made with a password after all, add it as a second secret named
`FOXGLOVE_CATALOG_KEY_PASSWORD`.

### 4. Publish

Do this after the commit from step 2 is on `main`.

1. Open https://github.com/BanditF/foxglove/actions/workflows/plugin-catalog.yml.
2. Click **Run workflow**, keep **Branch: main**, and click the green **Run workflow**.
3. When it's green, https://banditf.github.io/foxglove/plugins/v2/catalog.json shows the catalog.
4. In Foxglove, on the Plugins page, click **Check now** next to the catalog line. It says "Catalog: up to date".

### 5. Ship the app build with the real key

Only after step 4, when the first catalog is live: release an app build that carries the real
`root.pub`. The real key ends #779's grandfathering of unsigned plugins, so a build that ships it
before a signed catalog is published would have nothing to trust them by.

## After the launch

- **Expiry:** a catalog expires 30 days after it's signed, and the app refuses an expired one. The
  schedule re-signs it every two weeks. If that stops, the app's health view warns in the last 7
  days: "plugin catalog expires in N days; publish a fresh one". Then run step 4 by hand.
- **Adding plugins:** the plugin entries go in `plugins/catalog/catalog.json`
  (`plugin-catalog add plugins/catalog/catalog.json --dir … --zip … --url …`, after the release
  exists). A push to `main` that changes it publishes it. The workflow sets the sequence (one above
  what's published) and the expiry; whatever the source file says for those is ignored.
- `scripts\publish-plugins.ps1 -Pages` pushes to a `gh-pages` branch, which Pages doesn't serve
  with Source: GitHub Actions. Use the workflow instead.
