#!/bin/sh
# Usage: make-formula.sh 0.1.0 <sha256 of the release tarball>
# Prints the Homebrew formula for that release on standard output.
set -eu

version="$1"
sha="$2"
cat <<RUBY
class LfMide < Formula
  desc "Lightweight terminal IDE for Compose Multiplatform (Android + iOS)"
  homepage "https://github.com/alifu/LF-Mide"
  url "https://github.com/alifu/LF-Mide/releases/download/v${version}/lf-mide-${version}-macos-universal.tar.gz"
  sha256 "${sha}"
  license "MIT"

  # One universal binary (Apple silicon + Intel), built for macOS 14 and later.
  depends_on macos: :sonoma
  # The editor panel runs Neovim; project search runs ripgrep.
  depends_on "neovim"
  depends_on "ripgrep"

  def install
    bin.install "lf-mide"
    doc.install "LICENSE"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/lf-mide --version")
    # Both architectures are present in the one binary.
    assert_match "arm64", shell_output("lipo -archs #{bin}/lf-mide")
    assert_match "x86_64", shell_output("lipo -archs #{bin}/lf-mide")
    assert_path_exists doc/"LICENSE"
  end
end
RUBY
