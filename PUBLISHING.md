# Publishing niv on GitHub

The recommended eventual home is `b-heifets/niv`, with original development credited to `daniel-rijsketic`.

## 1. Check the local repository

```bash
cd ~/Documents/_GitHub/niv
git status --short
git branch --show-current
git log -5 --format='%h %an <%ae> %s'
git config --local user.name
git config --local user.email
```

GitHub links commits to accounts through commit email addresses. Set the local values to the author name and an email **verified on** `daniel-rijsketic` (or its GitHub noreply address) *before future commits*:

```bash
git config user.name 'Daniel Rijsketic'
git config user.email 'YOUR_VERIFIED_GITHUB_EMAIL'
```

Changing this configuration does **not** rewrite existing commits; keep the full local history. To inspect the existing authors:

```bash
git shortlog -sne --all
```

## 2. Add the publication files and test

Download and extract the latest GitHub-ready update in the existing Git working tree. The README and screenshot gallery use relative paths under `assets/`, so no external hosting is necessary.

If you already extracted the earlier v0.27.0 publication ZIP, remove its superseded screenshot after extracting the gallery update:

```bash
rm -f assets/niv-screenshot.webp
```

Then:

```bash
cargo fmt --check
cargo test
cargo build --release
niv -v
```

Commit changes in the existing local repository, rather than creating a new Git repository:

```bash
git add src Cargo.toml Cargo.lock README.md LICENSE CITATION.cff examples assets .gitignore PUBLISHING.md
git commit -m 'Prepare niv for public release and configure scroll directions'
```

**Pre-push review:** `git status`, `git ls-files` and `git log --stat` should not show secrets, user data, private project files, or giant NIfTI volumes. New `.gitignore` rules do not remove files already committed in earlier history.

## 3. Create GitHub repo and push

**Option A — direct publication** (best if you have access to the `b-heifets` account):

1. Create an empty **public** `niv` repository at https://github.com/new while authenticated as `b-heifets` (or have that account's owner create it and grant `daniel-rijsketic` write access). Git commit attribution is controlled separately by the author email.
2. Do not generate an initial README or license on GitHub, since those files already exist locally.
3. In the existing local directory:

```bash
git remote -v
git remote add origin https://github.com/b-heifets/niv.git
git push -u origin main
```

If `origin` already exists, inspect it and use `git remote set-url origin ...` only if the existing URL is not the intended destination.

**Option B — publish under your account, then transfer** (often easier if you do not control `b-heifets` credentials):

```bash
# Requires GitHub CLI authentication as daniel-rijsketic
gh repo create daniel-rijsketic/niv --public --source=. --remote=origin --push
```

Then, on the GitHub repository page, select **Settings → General → Danger Zone → Transfer ownership**, enter `b-heifets`, and complete the destination account's acceptance. GitHub preserves commits and Git history through a repository transfer. Since `b-heifets` is a personal account, the receiving owner must accept the transfer. Afterward, update your local remote:

```bash
git remote set-url origin https://github.com/b-heifets/niv.git
git remote -v
```


## 4. Media assets

The README shows four compressed, lossless WebP screenshots:

- `assets/niv-viewer.webp` — single-slice atlas and cluster overlays (hero image)
- `assets/niv-projection.webp` — clean sagittal maximum projection
- `assets/niv-montage.webp` — 3 × 4 montage
- `assets/niv-picker.webp` — searchable file picker

The four images total about 1.2 MB, so keeping them in `niv` is simpler than relying on a second repository. The earlier `assets/niv-screenshot.webp` can be removed once the README no longer refers to it.

**Before making the repo public:** confirm that screenshots are permitted to show any unpublished research data. Keep private file paths, credentials, and other sensitive files out of both the current tree and the *entire commit history*. Review the README rendering after the first push. If assets move to `b-heifets/UNRAVEL-media` later, confirm the media licensing and attribution requirements first.

## 5. Citation and project attribution

`CITATION.cff` lists Daniel Rijsketic as the software author, allowing GitHub's **Cite this repository** control to surface that attribution independent of repository ownership. The README and Git commit authorship also credit `daniel-rijsketic`.

## 6. License

The distribution uses GPL-3.0-only, matching UNRAVEL's version of GPL-3. The original local project metadata used MIT; if any code is owned by collaborators or an institution, confirm relicensing rights before publishing. Future relicensing requires permission from all relevant rightsholders. Releases already distributed under a previous valid license cannot be retroactively revoked.
