# Testing on a Windows PC

Before a release, every step here must work on a real 64-bit Windows 10 or
11 PC. Best on a user account that has never had Editor, LaTeX or Typst.
Anything that goes wrong: Help > Report a problem, in the app itself.

## Get the installer

GitHub > Actions > Windows preview > Run workflow, on the branch to test.
When it is done, download `Editor-windows-setup` from the run and unpack it.

## 1. Install

- [ ] Open the installer. SmartScreen warns: More info > Run anyway works.
- [ ] No administrator password is asked for.
- [ ] Editor is in the Start menu with the pencil icon; the taskbar shows the same icon.
- [ ] `editor .` in a new terminal opens that folder.

## 2. The window

- [ ] The title bar has minimize, maximize and close; all three work.
- [ ] Resting the pointer on maximize shows the snap layouts (Windows 11).
- [ ] Double click on the bar maximizes and restores; dragging the bar moves the window.
- [ ] Focus mode keeps the three buttons at the top right.
- [ ] Code looks like on the Mac (DejaVu Sans Mono); no console window ever flashes up.

## 3. Setup, without anything installed

- [ ] The setup offers winget as the recommended way, when winget is there.
- [ ] LaTeX compact (Tectonic) installs; the test document becomes a PDF (the first one takes 2 to 3 minutes).
- [ ] LaTeX complete (MiKTeX) installs without a password; the test document becomes a PDF.
- [ ] Typst installs and its test document becomes a PDF.
- [ ] Without internet, the test stops with a clear message instead of hanging.

## 4. Writing

- [ ] A project in a folder with spaces and umlauts, such as `C:\Users\<you>\Documents\Übung 1`, compiles.
- [ ] An error shows the error card at the right line; Go to line works.
- [ ] Click in the PDF jumps to the source and back (SyncTeX).
- [ ] A file with Windows line endings keeps them after editing (check in another editor).
- [ ] Markdown preview shows tables and typeset formulas.
- [ ] Export as PDF and as Word (Pandoc) work.

## 5. Files from Explorer

- [ ] Double click on a `.typ` file opens it in Editor.
- [ ] With Editor open behind another window, double click on a `.tex` (Open with > Editor) brings Editor to the front with the file.
- [ ] Right click on a folder > Open in Editor works.

## 6. Live session, Mac and Windows

- [ ] Windows asks once about the network; after Allow, a session from the Mac can be joined with its code.
- [ ] Typing on either side shows on the other within a moment; cursors and comments come through.

## 7. Git and the keychain

- [ ] Signing in to GitHub in the git panel works, and the login is still there after a restart (Credential Manager).

## 8. Update and uninstall

- [ ] With a newer release published, the update bar appears; Install replaces Editor and starts it again.
- [ ] Settings > Apps > Editor > Uninstall removes it, the Start menu entry and the `editor` command.
