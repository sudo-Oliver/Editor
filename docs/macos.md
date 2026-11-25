# Editor on macOS

[Back to the README](../README.md) · [Editor on Windows](windows.md) · [User guide](guide.md)

Editor runs on Apple silicon and Intel Macs with macOS 11 or later.

## Install

Download `Editor-<version>.dmg` from the [latest release](https://github.com/sudo-Oliver/Editor/releases/latest), open it and drag Editor into Applications.

With Homebrew, this does the same and adds the `editor` command:

```sh
brew install --cask sudo-oliver/tap/editor
```

If Editor starts from the dmg or from Downloads, a bar offers to move it to Applications. It copies itself there, starts again and ejects the dmg. Only from Applications can it update itself.

## First open

Editor is free and not registered with Apple, so macOS warns the first time you open it. That is expected, and macOS asks only once. The dmg has the same steps in "First open.txt".

1. Drag Editor onto the Applications folder in the dmg window, then open Editor from Applications.
2. macOS says it cannot check Editor for malicious software. Click **Done**.
3. Open **System Settings**, then **Privacy & Security**, and scroll down to "Editor was blocked to protect your Mac". Click **Open Anyway** and confirm with your password or Touch ID.
4. Confirm once more in the last question. From now on Editor opens like any other app.

On macOS 14 and earlier, right-click Editor in Applications, choose **Open**, and click **Open** in the question instead.

If you are at home in Terminal, this line does the same in one step:

```sh
xattr -dr com.apple.quarantine /Applications/Editor.app
```

## Open files and folders

Files open from Finder (Open With, a double click, or drag them onto the app), and from the terminal like VS Code's `code`:

```sh
editor .            # this folder as a project
editor main.tex     # the file, in the project it belongs to
```

The Homebrew cask adds the `editor` command. Without it, choose Help, "Add editor command for Terminal…".

## Setup

On the first start, a short setup asks whether you write LaTeX, Typst or both, and installs exactly that. Nothing has to be on the Mac beforehand.

For LaTeX it offers two choices:

- **MacTeX**, the complete LaTeX (about 7 GB, every package offline). This is the recommended choice. You wait once for its download, about 15 minutes on a fast connection, and the setup shows how long it still takes. After that everything works offline.
- **Tectonic**, the compact LaTeX (about 20 MB). It fetches packages as documents need them.

The setup shows the download and disk size of every part and checks the free space before anything starts. Then it installs in one of these ways:

- **With Homebrew**, the default when Homebrew is already there: `brew install tectonic`, `typst` or `pandoc`.
- **In the app's folder**, the default when Homebrew is not there. Nobody without Homebrew has to know what it is. The official programs go into `~/Library/Application Support/io.github.sudo-oliver.editor/tools`, are checked against a fingerprint, and can be removed under Settings, Writing tools. No password is needed. The packages that Tectonic and Typst download go to `~/Library/Caches/io.github.sudo-oliver.editor`. Removing either program removes its packages too.
- **Install Homebrew first**, under "Other ways to install". First Apple's developer tools through Apple's own dialog, then Homebrew's official installer, which asks for your password once, then the formulas.
- **By hand**, also under "Other ways to install". The setup lists each file with its download link and says where it goes: programs into `~/Applications/Editor Tools` (packed or unpacked), MacTeX through its own installer. The setup notices the files on its own, checks them and makes them runnable.

MacTeX always comes from its official package. The package is checked against CTAN's checksum and its author's signature, and installs to `/Library/TeX` after macOS asks for your password.

At the end, a test document compiles in each chosen language, so that the setup only says "all set" when it works. To add a language later, open the setup again from the project hub. Pandoc, which only the Word and HTML exports need, installs on the first such export. If you already have MacTeX, Typst or Homebrew, the setup sees them and the app uses them.

Git and GitHub are not part of the setup. The git panel sets them up when you want them: "Set up git" opens Apple's dialog for its developer tools, and "Put it on GitHub" signs you in through the browser, creates a private repository and pushes.

Then a welcome document opens, and a tour shows each part of the editor. Help, Editor tour plays it again.

## Updates

The app asks GitHub for the newest release a few seconds after it starts, and then every six hours. When there is a newer one, a bar offers "Update and restart".

The app then downloads the dmg and checks it: against the checksum GitHub lists for it, its bundle identifier, its version and its signature. The signature must be intact, and from the same Apple team as the running app when that one has a team. A release signed ad hoc has none. Then the app puts the new version in place of the old one and starts again. Open files are saved first. If anything fails, the old app stays as it was.

An app installed with the Homebrew cask updates through `brew upgrade --cask` instead, so that Homebrew stays in step. Only an app bundle updates itself, never a build run with cargo. The only request is one to `api.github.com`, with nothing about you in it.

### LaTeX updates

MacTeX keeps itself current from inside the app, without Homebrew or the Terminal.

- Once a week the app asks TeX Live's package manager, `tlmgr`, which packages have newer versions. Asking needs no password. Without internet the app tries again later.
- Settings, Writing tools shows the installed TeX Live year, when the app last checked and how many updates there are. "Update LaTeX packages" first says why the password is needed. Then macOS asks for it once, since MacTeX belongs to the system, and `tlmgr` updates itself and every package. Closing the password dialog changes nothing.
- When a document stops because a package is missing or too old, the error offers the same update.
- Each spring tug.org publishes the TeX Live of the new year, which the old one cannot update to. The weekly check notices, and the app offers "Install MacTeX 2027" (or whichever year), the same download as in the setup. Your LaTeX keeps working until the new one is installed.
- The old year stays on the disk until you remove it. The settings show how much space it takes, and remove it after you confirm and enter your password. Nothing is ever deleted on its own.

## Where Editor keeps its files

| What | Where |
| --- | --- |
| Settings, recent projects, recovery copies | `~/Library/Application Support/io.github.sudo-oliver.editor` |
| Programs the setup installed | `~/Library/Application Support/io.github.sudo-oliver.editor/tools` |
| Caches and downloads | `~/Library/Caches/io.github.sudo-oliver.editor` |
| Project settings | `.editor.toml` in the project |
| Build output | `.build/` in the project |

Versions before 0.1.0 kept these in `~/.config/editor` and `~/.cache/editor`. The app moves them on its first start.

## Uninstall

Move Editor from Applications to the Bin, and remove the folders in the table above if you want. With Homebrew, `brew uninstall --zap --cask editor` removes the app and its folders. MacTeX stays installed, since other apps can use it too.
