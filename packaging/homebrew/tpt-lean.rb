# SPDX-License-Identifier: MIT OR Apache-2.0
# Homebrew formula for tpt-lean. The version and hashes are filled from a release by
# scripts/update-manifests.sh; do not edit them by hand.
class TptLean < Formula
  desc "Rust-powered toolbox for Lean 4 (Lean remains the proof checker)"
  homepage "https://github.com/tpt-solutions/tpt-lean"
  version "@VERSION@"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/tpt-solutions/tpt-lean/releases/download/v@VERSION@/tpt-lean-aarch64-apple-darwin.tar.gz"
      sha256 "@SHA256:aarch64-apple-darwin@"
    end
    on_intel do
      url "https://github.com/tpt-solutions/tpt-lean/releases/download/v@VERSION@/tpt-lean-x86_64-apple-darwin.tar.gz"
      sha256 "@SHA256:x86_64-apple-darwin@"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/tpt-solutions/tpt-lean/releases/download/v@VERSION@/tpt-lean-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "@SHA256:aarch64-unknown-linux-gnu@"
    end
    on_intel do
      url "https://github.com/tpt-solutions/tpt-lean/releases/download/v@VERSION@/tpt-lean-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "@SHA256:x86_64-unknown-linux-gnu@"
    end
  end

  def install
    bin.install "tpt-lean"
    bin.install "tlean"
  end

  test do
    assert_match "tpt-lean", shell_output("#{bin}/tpt-lean --help")
  end
end
