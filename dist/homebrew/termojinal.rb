class Termojinal < Formula
  desc "GPU-accelerated terminal emulator with AI agent coordination (CLI tools)"
  homepage "https://github.com/KikuchiTomo/termojinal"
  TERMOJINAL_VERSION = "0.2.5-beta"
  version TERMOJINAL_VERSION
  license "MIT"

  # Pre-built universal binaries from GitHub Releases (built by CI)
  url "https://github.com/KikuchiTomo/termojinal/releases/download/v#{TERMOJINAL_VERSION}/termojinal-#{TERMOJINAL_VERSION}-cli-macos-universal.tar.gz"
  sha256 "ee436d2ff1ba4c09fe822f1ce6aa5dcd31c984906a4cc745da84377171c27be6"

  def install
    # Install pre-built CLI binaries
    bin.install "termojinal"
    bin.install "termojinald"
    bin.install "tm"
    bin.install "termojinal-mcp"
    bin.install "termojinal-sign"

    # Install default config example
    (pkgshare/"config.example.toml").write default_config
  end

  def default_config
    <<~TOML
      # termojinal configuration
      # Copy to ~/.config/termojinal/config.toml and customize.
      # All values shown are defaults — uncomment and change as needed.

      # ── Startup ────────────────────────────────────────────────
      [startup]
      # "default" = $HOME, "fixed" = use directory below, "restore" = last session cwd
      mode = "default"
      directory = "~"

      # ── Font ──────────────────────────────────────────────────
      [font]
      family = "monospace"
      size = 14.0
      line_height = 1.2
      max_size = 72.0
      size_step = 1.0

      # ── Window ────────────────────────────────────────────────
      [window]
      width = 960
      height = 640
      opacity = 1.0
      padding_x = 1.0
      padding_y = 0.5

      # ── Theme (Catppuccin Mocha) ──────────────────────────────
      [theme]
      background = "#1E1E2E"
      foreground = "#CDD6F4"
      cursor = "#F5E0DC"
      selection_bg = "#45475A"
      preedit_bg = "#313244"
      search_highlight_bg = "#F9E2AF"
      search_highlight_fg = "#1E1E2E"
      bold_brightness = 1.2
      dim_opacity = 0.6
      # auto_switch = false
      # dark = ""
      # light = ""
      # ANSI colors
      black = "#45475A"
      bright_black = "#585B70"
      red = "#F38BA8"
      bright_red = "#F38BA8"
      green = "#A6E3A1"
      bright_green = "#A6E3A1"
      yellow = "#F9E2AF"
      bright_yellow = "#F9E2AF"
      blue = "#89B4FA"
      bright_blue = "#89B4FA"
      magenta = "#F5C2E7"
      bright_magenta = "#F5C2E7"
      cyan = "#94E2D5"
      bright_cyan = "#94E2D5"
      white = "#BAC2DE"
      bright_white = "#A6ADC8"

      # ── Tab Bar ───────────────────────────────────────────────
      [tab_bar]
      # format variables: {title}, {cwd_base}, {index}
      format = "{title|cwd_base|Tab {index}}"
      always_show = false
      position = "top"
      height = 36.0
      max_width = 200.0
      min_tab_width = 60.0
      new_tab_button_width = 32.0
      bg = "#1A1A1F"
      active_tab_bg = "#2E2E38"
      active_tab_fg = "#F2F2F8"
      inactive_tab_fg = "#8C8C99"
      accent_color = "#4D8CFF"
      separator_color = "#383840"
      close_button_fg = "#808088"
      new_button_fg = "#808088"
      padding_x = 6.0
      padding_y = 6.0
      accent_height = 2
      bottom_border = true
      bottom_border_color = "#2A2A34"

      # ── Sidebar ───────────────────────────────────────────────
      [sidebar]
      width = 240.0
      min_width = 120.0
      max_width = 400.0
      bg = "#0D0D12"
      active_entry_bg = "#1A1A24"
      active_fg = "#F2F2F8"
      inactive_fg = "#8C8C99"
      dim_fg = "#666670"
      git_branch_fg = "#5AB3D9"
      separator_color = "#333338"
      notification_dot = "#FF941A"
      git_dirty_color = "#CCB34D"
      top_padding = 6.0
      side_padding = 6.0
      entry_gap = 4.0
      info_line_gap = 1.0
      allow_accent_color = "#4FC1FF"
      allow_hint_fg = "#7DC8FF"
      agent_status_enabled = true
      agent_indicator_style = "pulse"  # "pulse", "color", "none"
      agent_pulse_speed = 2.0
      agent_active_color = "#A78BFA"
      agent_idle_color = "#FBBF24"

      # ── Pane ──────────────────────────────────────────────────
      [pane]
      separator_color = "#4D4D4D"
      focus_border_color = "#3399FFCC"
      separator_width = 2
      focus_border_width = 2
      separator_tolerance = 4.0
      scrollbar_thumb_opacity = 0.5
      scrollbar_track_opacity = 0.1
      # "inherit" = current pane cwd, "home" = $HOME, "fixed" = fixed_directory
      working_directory = "inherit"
      # fixed_directory = "~/projects"

      # ── Status Bar ────────────────────────────────────────────
      [status_bar]
      enabled = true
      height = 28.0
      background = "#141420"
      padding_x = 8.0
      top_border = true
      top_border_color = "#2A2A34"
      # Segments: {user}, {host}, {cwd_short}, {git_branch}, {git_status},
      #           {ports}, {shell}, {pane_size}, {font_size}, {time}
      [[status_bar.left]]
      content = "{user}@{host}"
      fg = "#FFFFFF"
      bg = "#3A3AFF"
      [[status_bar.left]]
      content = "{cwd_short}"
      fg = "#CCCCCC"
      bg = "#2A2A34"
      [[status_bar.left]]
      content = "{git_branch} {git_status}"
      fg = "#A6E3A1"
      bg = "#1A1A24"
      [[status_bar.right]]
      content = "{ports}"
      fg = "#94E2D5"
      bg = "#1A1A24"
      [[status_bar.right]]
      content = "{shell}"
      fg = "#888888"
      bg = "#2A2A34"
      [[status_bar.right]]
      content = "{pane_size}"
      fg = "#888888"
      bg = "#1A1A24"
      [[status_bar.right]]
      content = "{font_size}px"
      fg = "#888888"
      bg = "#2A2A34"
      [[status_bar.right]]
      content = "{time}"
      fg = "#FFFFFF"
      bg = "#3A3AFF"

      # ── Search ────────────────────────────────────────────────
      [search]
      bar_bg = "#262633F2"
      input_fg = "#F2F2F2"
      border_color = "#4D4D66"

      # ── Command Palette ───────────────────────────────────────
      [palette]
      bg = "#1F1F29F2"
      border_color = "#4D4D66"
      input_fg = "#F2F2F2"
      separator_color = "#40404D"
      command_fg = "#CCCCD1"
      selected_bg = "#383852"
      description_fg = "#808088"
      overlay_color = "#00000080"
      max_height = 400.0
      max_visible_items = 10
      width_ratio = 0.6
      corner_radius = 12.0
      blur_radius = 20.0
      shadow_radius = 8.0
      shadow_opacity = 0.3
      border_width = 1.0

      # ── Quick Terminal (Ctrl+`) ────────────────────────────────
      [quick_terminal]
      enabled = true
      hotkey = "ctrl+`"
      animation = "slide_down"  # "slide_down", "slide_up", "fade", "none"
      animation_duration_ms = 200
      height_ratio = 0.4
      width_ratio = 1.0
      position = "center"       # "left", "center", "right"
      screen_edge = "top"       # "top", "bottom"
      hide_on_focus_loss = false
      dismiss_on_esc = true
      show_sidebar = false
      show_tab_bar = false
      show_status_bar = true
      window_level = "floating"  # "normal", "floating", "above_all"
      corner_radius = 12.0
      own_workspace = true

      # ── Notifications ─────────────────────────────────────────
      [notifications]
      enabled = true
      sound = false
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

    # Create config directory
    config_dir = Pathname.new(Dir.home)/".config/termojinal"
    config_dir.mkpath unless config_dir.exist?
  end

  def caveats
    <<~EOS
      CLI tools installed: termojinal, termojinald, tm, termojinal-mcp, termojinal-sign

      To install the GUI app (Termojinal.app):
        brew install --cask termojinal-app

      To start the daemon (enables Ctrl+` global hotkey):
        brew services start termojinal

      Run `tm setup` to configure Claude Code hooks and bundled commands.

      To configure:
        cp #{opt_pkgshare}/config.example.toml ~/.config/termojinal/config.toml
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/tm --version 2>&1", 0)
  end
end
