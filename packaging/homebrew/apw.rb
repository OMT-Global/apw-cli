class Apw < Formula
  desc "Apple Password CLI and local app broker (macOS-first)"
  homepage "https://github.com/OMT-Global/apw"
  version "2.0.0"
  url "https://github.com/OMT-Global/apw/archive/refs/tags/v2.0.0.tar.gz"
  sha256 "<replace-with-release-tarball-sha256>"
  license "GPL-3.0-only"

  depends_on :macos
  depends_on "rust" => :build
  depends_on "openssl@3" => :build

  def install
    system "bash", "./scripts/build-native-app.sh"
    system "cargo", "build", "--manifest-path", "rust/Cargo.toml", "--release"
    bin.install "rust/target/release/apw"
    libexec.install "native-app/dist/APW.app"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/apw --version")
    assert_match "\"app\"", shell_output("#{bin}/apw status --json 2>&1")
  end
end
