# Editor on Windows

[Back to the README](../README.md) · [Editor on macOS](macos.md) · [User guide](guide.md)

Editor runs on 64-bit Windows 10 and 11.

## Install

Download `Editor-<version>-windows-x64-setup.exe` from the [latest release](https://github.com/sudo-Oliver/Editor/releases/latest) and open it. The installer:

- installs Editor for you alone, without an administrator,
- adds Editor to the Start menu and the `editor` command to the `PATH`,
- lets `.tex`, `.typ`, `.bib` and `.md` files open in Editor, and adds "Open in Editor" to the right-click menu of folders.

The release also lists a `.sha256` file with the installer's checksum.

## First open

Editor is not signed with a paid certificate yet, so Windows SmartScreen warns the first time you open the installer. That is expected.

1. Open `Editor-<version>-windows-x64-setup.exe` from your Downloads.
2. Windows says "Windows protected your PC". Click **More info**.
3. Click **Run anyway**. The installer starts. It needs no administrator password.

The first time you start a live session, Windows asks whether Editor may use the network. Allow it, so that others can reach you.

## Open files and folders

Files open from Explorer (Open with, a double click, or drag them onto the app), and from a terminal like VS Code's `code`:

```sh
editor .            # this folder as a project
editor main.tex     # the file, in the project it belongs to
```

Open a new terminal after the installation, so that it sees the `editor` command.

## Setup

On the first start, a short setup asks whether you write LaTeX, Typst or both, and installs exactly that. Nothing has to be on the PC beforehand, and nothing needs an administrator.

For LaTeX it offers two choices:

- **TeX Live**, the complete LaTeX (about 5 GB, every package offline). This is the same LaTeX that MacTeX brings to the Mac. The setup installs it for you alone into `%LOCALAPPDATA%\Programs\TeXLive` and shows its progress and the time left. The download takes up to an hour.
- **Tectonic**, the compact LaTeX (about 20 MB). It fetches packages as documents need them. The first document takes 2 to 3 minutes.

Tectonic, Typst and Pandoc go into the app's own folder, `%LOCALAPPDATA%\Editor\Tools`, and are checked against a fingerprint. Settings, Writing tools can remove them again.

If you already have TeX Live in `C:\texlive`, the setup sees it and the app uses it. A MiKTeX that another editor installed does not count as the complete LaTeX, because Editor's build and log reading need TeX Live. The setup then offers TeX Live beside it, and the app uses TeX Live once it is there.

"Other ways to install" in the setup lists each file with its download link, if you want to download by hand. The files go into `%USERPROFILE%\Editor Tools`, and the setup notices them on its own.

At the end, a test document compiles in each chosen language, so that the setup only says "all set" when it works. To add a language later, open the setup again from the project hub. Pandoc, which only the Word and HTML exports need, installs on the first such export.

Git and GitHub are not part of the setup. In the git panel, "Set up git" installs the official MinGit into the app's folder, without an administrator. "Put it on GitHub" signs you in through the browser, creates a private repository and pushes. The Windows Credential Manager keeps the login.

Then a welcome document opens, and a tour shows each part of the editor. Help, Editor tour plays it again.

## Updates

The app asks GitHub for the newest release a few seconds after it starts, and then every six hours. When there is a newer one, a bar offers to update. The app downloads the new installer, checks it against the checksum GitHub lists for it, and runs it once the app has closed. The installer then starts the new version. Open files are saved first. The only request is one to `api.github.com`, with nothing about you in it.

### LaTeX updates

On Windows the app does not update TeX Live yet. Use TeX Live's own package manager in a terminal:

```sh
tlmgr update --self --all
```

## Where Editor keeps its files

| What | Where |
| --- | --- |
| Settings, recent projects, recovery copies | `%APPDATA%\Editor` |
| Programs the setup installed | `%LOCALAPPDATA%\Editor\Tools` |
| TeX Live | `%LOCALAPPDATA%\Programs\TeXLive` |
| Caches and downloads | `%LOCALAPPDATA%\Editor\Cache` |
| Project settings | `.editor.toml` in the project |
| Build output | `.build\` in the project |

## Uninstall

Settings, Apps, Editor, Uninstall removes Editor, its Start menu entry and the `editor` command. Remove the folders in the table above if you want them gone too.
