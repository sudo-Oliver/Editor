# Homebrew cask for Editor. scripts/release.sh fills in the version and the
# checksum and attaches the result to the release; copy it into the tap
# repository (sudo-Oliver/homebrew-tap, Casks/editor.rb) to publish it. Then:
#
#   brew install --cask sudo-oliver/tap/editor
cask "editor" do
  version "@VERSION@"
  sha256 "@SHA256@"

  url "https://github.com/sudo-Oliver/Editor/releases/download/v#{version}/Editor-#{version}.dmg"
  name "Editor"
  desc "Local LaTeX and Typst editor with a live PDF preview"
  homepage "https://github.com/sudo-Oliver/Editor"

  depends_on macos: ">= :big_sur"

  app "Editor.app"
  binary "#{appdir}/Editor.app/Contents/Resources/bin/editor"

  # Editor is not registered with Apple, so the mark of the download would make
  # macOS refuse the first start. The checksum above already checked the file.
  postflight do
    system_command "/usr/bin/xattr", args: ["-dr", "com.apple.quarantine", "#{appdir}/Editor.app"]
  end

  zap trash: "~/.config/editor"

  caveats <<~EOS
    The editor command opens files and folders: editor .
    On the first start, Editor installs LaTeX (MacTeX or Tectonic) or Typst, with Homebrew if you like.
  EOS
end
