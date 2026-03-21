class Termojinal < Formula
  desc "GPU-accelerated terminal emulator with AI agent coordination"
  homepage "https://github.com/KikuchiTomo/termojinal"
  TERMOJINAL_VERSION = "0.2.5-beta"
  version TERMOJINAL_VERSION
  license "MIT"

  # Pre-built universal binaries from GitHub Releases (built by CI)
  url "https://github.com/KikuchiTomo/termojinal/releases/download/v#{TERMOJINAL_VERSION}/termojinal-#{TERMOJINAL_VERSION}-cli-macos-universal.tar.gz"
  sha256 "ee436d2ff1ba4c09fe822f1ce6aa5dcd31c984906a4cc745da84377171c27be6"

  # The .app bundle is a separate download
  resource "app" do
    url "https://github.com/KikuchiTomo/termojinal/releases/download/v#{TERMOJINAL_VERSION}/termojinal-#{TERMOJINAL_VERSION}-macos-universal.tar.gz"
    sha256 "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
  end

  def install
    # Install pre-built CLI binaries
    bin.install "termojinal"
    bin.install "termojinald"
    bin.install "tm"
    bin.install "termojinal-mcp"
    bin.install "termojinal-sign"

    # Extract and install the .app bundle
    resource("app").stage do
      prefix.install "Termojinal.app"
    end

    # Install default config example
    (pkgshare/"config.example.toml").write default_config
  end

  def default_config
    <<~TOML
      # termojinal configuration
      # Copy to ~/.config/termojinal/config.toml and customize

      [font]
      family = "monospace"
      size = 14.0
      line_height = 1.2

      [window]
      opacity = 0.95

      [quick_terminal]
      enabled = true
      hotkey = "ctrl+`"
      animation = "slide_down"
      height_ratio = 0.4
    TOML
  end

  # launchd plist for termojinald daemon
  service do
    run [opt_bin/"termojinald"]
    keep_alive true
    log_path var/"log/termojinal/termojinald.log"
    error_log_path var/"log/termojinal/termojinald.err.log"
    environment_variables RUST_LOG: "info"
    working_dir HOMEBREW_PREFIX
  end

  def post_install
    (var/"log/termojinal").mkpath

    # Copy Termojinal.app to /Applications (not symlink — avoids macOS App Translocation loops)
    app_source = prefix/"Termojinal.app"
    app_target = Pathname.new("/Applications/Termojinal.app")
    if app_source.exist?
      begin
        rm_rf app_target if app_target.exist?
        cp_r app_source, app_target
        system "xattr", "-cr", app_target.to_s
        ohai "Installed Termojinal.app to /Applications"
      rescue StandardError => e
        opoo "Could not copy Termojinal.app to /Applications: #{e.message}"
        opoo "Run: rm -rf '#{app_target}' && cp -r '#{app_source}' /Applications/Termojinal.app"
      end
    end

    # Create config directory
    config_dir = Pathname.new(Dir.home)/".config/termojinal"
    config_dir.mkpath unless config_dir.exist?
  end

  def caveats
    msg = <<~EOS
      Run `tm setup` to configure Claude Code hooks and bundled commands.

      To start the daemon (enables Ctrl+` global hotkey):
        brew services start termojinal

      To configure:
        cp #{opt_pkgshare}/config.example.toml ~/.config/termojinal/config.toml
    EOS

    app_target = Pathname.new("/Applications/Termojinal.app")
    if app_target.exist?
      msg += "\n  Termojinal.app is available in /Applications.\n"
    else
      msg += <<~EOS

        To add Termojinal.app to /Applications:
          cp -r #{opt_prefix}/Termojinal.app /Applications/Termojinal.app
      EOS
    end
    msg
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/tm --version 2>&1", 0)
  end
end
