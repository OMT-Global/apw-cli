class Apw < Formula
  desc "Apple Password CLI and daemon (macOS-first)"
  homepage "https://github.com/omt-global/apw-native"
  version "1.2.0"
  url "https://github.com/omt-global/apw-native/archive/refs/tags/v1.2.0.tar.gz"
  sha256 "<replace-with-release-tarball-sha256>"
  license "GPL-3.0-only"

  depends_on "rust" => :build

  on_macos do
    # macOS keychain path integration requires macOS.
  end

  def install
    system "bash", "./scripts/build-native-host.sh"
    system "cargo", "build", "--manifest-path", "rust/Cargo.toml", "--release"
    bin.install "rust/target/release/apw"
    libexec.install "native-host/dist/APWNativeHost.app"
  end

  service do
    run [opt_bin/"apw", "start", "--runtime-mode", "native", "--bind", "127.0.0.1", "--port", "10000"]
    keep_alive true
    run_type :immediate
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/apw --version")
    assert_match "\"host\"", shell_output("#{bin}/apw status --json 2>&1")
  end
end
