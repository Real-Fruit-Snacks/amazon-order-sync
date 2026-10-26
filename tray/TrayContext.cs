using System.Diagnostics;
using Microsoft.Win32;

namespace AmazonOrderSync.Tray;

/// <summary>The tray icon, its menu, and the poll loop that turns server
/// notifications into Windows toasts.</summary>
internal sealed class TrayContext : ApplicationContext
{
    private const string RunKey = @"Software\Microsoft\Windows\CurrentVersion\Run";
    private const string RunValue = "AmazonOrderSync";

    private readonly NotifyIcon _icon;
    private readonly ContextMenuStrip _menu = new();
    private readonly ToolStripMenuItem _status = new("Starting…") { Enabled = false };
    private readonly ToolStripMenuItem _syncNow = new("Sync now");
    private readonly ToolStripMenuItem _setupLogin = new("Set up Amazon login…");
    private readonly ToolStripMenuItem _startup = new("Start with Windows") { CheckOnClick = true };
    private readonly ToolStripMenuItem _restart = new("Restart server");
    private readonly System.Windows.Forms.Timer _poll = new() { Interval = 3000 };

    private Settings _settings;
    private ServerProcess _server;
    private bool _quitting;
    private bool _busy;
    private int _downTicks;

    public TrayContext()
    {
        _settings = Settings.Load();
        _server = new ServerProcess(_settings.Port);

        _icon = new NotifyIcon
        {
            Icon = LoadIcon(),
            Text = "Amazon Order Sync — starting",
            Visible = true,
            ContextMenuStrip = _menu
        };
        _icon.DoubleClick += (_, _) => OpenReviews();
        _icon.BalloonTipClicked += (_, _) => OpenReviews();

        BuildMenu();
        // Force the menu's window handle into existence now: OnServerExited
        // marshals onto the UI thread via _menu.BeginInvoke, which throws
        // until a handle exists (the server can die within milliseconds).
        _ = _menu.Handle;
        _startup.Checked = IsStartupEnabled();
        _startup.CheckedChanged += (_, _) => SetStartup(_startup.Checked);

        _poll.Tick += async (_, _) => await TickAsync();
        _ = StartServerAsync(initial: true);
    }

    // ----- menu -----------------------------------------------------------

    private void BuildMenu()
    {
        var reviews = new ToolStripMenuItem("Review your purchases on Amazon") { Font = new Font(_menu.Font, FontStyle.Bold) };
        reviews.Click += (_, _) => OpenReviews();
        var open = new ToolStripMenuItem("Open dashboard");
        open.Click += (_, _) => OpenDashboard();
        _syncNow.Click += async (_, _) => await SyncNowAsync();
        _setupLogin.Click += async (_, _) => await SetupLoginAsync();

        var notes = new ToolStripMenuItem("Open notes folder");
        notes.Click += (_, _) => Safe(() => Paths.OpenFolder(_settings.NotesPath));
        var config = new ToolStripMenuItem("Edit config.json");
        config.Click += (_, _) => Safe(() => Paths.OpenFile(Paths.ConfigFile));
        var log = new ToolStripMenuItem("View server log");
        log.Click += (_, _) => Safe(() => Paths.OpenFile(Paths.ServerLog));
        var ext = new ToolStripMenuItem("Open browser-extension folder (load in Brave)");
        ext.Click += (_, _) => Safe(() =>
        {
            var script = Paths.FindServerScript();
            var root = script is null ? Paths.ExeDir : Path.GetDirectoryName(Path.GetDirectoryName(script)!)!;
            Paths.OpenFolder(Path.Combine(root, "browser-extension"));
        });

        _restart.Click += async (_, _) => await RestartAsync();
        var quit = new ToolStripMenuItem("Quit (stops the server)");
        quit.Click += async (_, _) => await QuitAsync();

        var about = new ToolStripMenuItem($"Amazon Order Sync v{Paths.Version}") { Enabled = false };

        _menu.Items.AddRange(new ToolStripItem[]
        {
            about,
            _status,
            new ToolStripSeparator(),
            reviews,
            open,
            _syncNow,
            _setupLogin,
            new ToolStripSeparator(),
            notes, config, log, ext,
            new ToolStripSeparator(),
            _startup,
            _restart,
            quit
        });
    }

    // ----- server lifecycle ----------------------------------------------

    private async Task StartServerAsync(bool initial)
    {
        _poll.Stop();
        SetStatus("Starting server…", "Amazon Order Sync — starting");
        Settings.EnsureDefault(_settings.NotesPath);
        _settings = Settings.Load();
        if (_server.Port != _settings.Port)
        {
            _server.Dispose();
            _server = new ServerProcess(_settings.Port);
        }
        _server.Exited -= OnServerExited;
        _server.Exited += OnServerExited;

        var ok = await _server.StartAsync();
        if (ok)
        {
            var h = await _server.HealthAsync();
            var how = _server.Adopted ? "attached to running server" : "server started";
            Log.Write($"{how} v{h?.Version} pid {h?.Pid}");
            if (initial) Toast("Amazon Order Sync", $"Running — dashboard at {_server.BaseUrl}/", ToolTipIcon.Info);
            if (h is { BrowserFound: false })
                Toast("No browser found for auto-sync", "Install Brave/Chrome/Edge or set \"browserPath\" in config.json.", ToolTipIcon.Warning);
        }
        else
        {
            var msg = _server.LastStartError ?? "Unknown error";
            SetStatus("Server failed to start", "Amazon Order Sync — server not running");
            Toast("Amazon Order Sync could not start", msg, ToolTipIcon.Error);
            if (msg.Contains("Node.js"))
            {
                var r = MessageBox.Show(
                    msg + "\n\nOpen the Node.js download page now?",
                    "Amazon Order Sync", MessageBoxButtons.YesNo, MessageBoxIcon.Warning);
                if (r == DialogResult.Yes) Safe(() => Paths.OpenInBrowser("https://nodejs.org/en/download"));
            }
        }
        _downTicks = 0;
        _poll.Start();
    }

    private void OnServerExited(object? sender, int code)
    {
        if (_quitting) return;
        // Marshal to the UI thread; auto-restart once after a crash.
        if (!_menu.IsHandleCreated) { Log.Write($"Server exited (code {code}) before UI was ready"); return; }
        _menu.BeginInvoke(new Action(async () =>
        {
            SetStatus($"Server exited (code {code})", "Amazon Order Sync — server stopped");
            if (code != 0)
            {
                Toast("Amazon Order Sync server stopped", $"Exit code {code}. Restarting…", ToolTipIcon.Warning);
                await Task.Delay(2000);
                if (!_quitting) await StartServerAsync(initial: false);
            }
        }));
    }

    private async Task RestartAsync()
    {
        if (_busy) return;
        _busy = true;
        try
        {
            _poll.Stop();
            SetStatus("Restarting…", "Amazon Order Sync — restarting");
            await _server.StopAsync(force: true);
            _server.Dispose();
            _server = new ServerProcess(Settings.Load().Port);
            await StartServerAsync(initial: false);
        }
        finally { _busy = false; }
    }

    private async Task QuitAsync()
    {
        if (_quitting) return;
        _quitting = true;
        _poll.Stop();
        _icon.Text = "Amazon Order Sync — stopping";
        try { await _server.StopAsync(force: false); } catch (Exception e) { Log.Write($"Quit: {e.Message}"); }
        _icon.Visible = false;
        _icon.Dispose();
        _server.Dispose();
        ExitThread();
    }

    // ----- actions --------------------------------------------------------

    private void OpenDashboard() => Safe(() => Paths.OpenInBrowser(_server.BaseUrl + "/"));

    // The review queue on Amazon itself — the extension auto-fills the forms
    // there from saved drafts. Double-click and toast-click land here.
    private const string ReviewsUrl = "https://www.amazon.com/review/review-your-purchases/";
    private void OpenReviews() => Safe(() => Paths.OpenInBrowser(ReviewsUrl));

    private async Task SyncNowAsync()
    {
        var h = await _server.HealthAsync();
        if (h is null) { Toast("Server not running", "Use Restart server.", ToolTipIcon.Warning); return; }
        if (!h.BrowserFound)
        {
            Toast("No browser for headless sync", "Install Brave/Chrome/Edge or set browserPath in config.json. You can still sync from the extension popup.", ToolTipIcon.Warning);
            return;
        }
        Toast("Sync started", "Headless sync is running in the background.", ToolTipIcon.Info);
        // Long-poll: resolves when the run finishes.
        var (status, body) = await _server.PostAsync("/api/sync-now", longPoll: true);
        if (status == 409)
            Toast("Sync already running", ServerProcess.ErrorFromBody(body, "Another sync or the setup window is active."), ToolTipIcon.Info);
        else if (status != 200 && status != 0)
            Toast("Sync failed", ServerProcess.ErrorFromBody(body, $"HTTP {status}"), ToolTipIcon.Error);
        // 200: the completion toast arrives via /api/notifications.
    }

    private async Task SetupLoginAsync()
    {
        var (status, body) = await _server.PostAsync("/api/headless-setup");
        if (status == 200)
            Toast("Log in to Amazon", "A browser window opened. Sign in, then close it — auto-sync will reuse that login.", ToolTipIcon.Info);
        else
            Toast("Could not open setup window", ServerProcess.ErrorFromBody(body, status == 0 ? "Server not reachable" : $"HTTP {status}"), ToolTipIcon.Error);
    }

    // ----- polling --------------------------------------------------------

    private async Task TickAsync()
    {
        if (_quitting || _busy) return;
        var h = await _server.HealthAsync();
        if (h is null)
        {
            _downTicks++;
            SetStatus("Server not responding", "Amazon Order Sync — not responding");
            // Adopted server vanished (someone closed it): take over.
            if (_server.Adopted && _downTicks >= 3 && !_busy) await RestartAsync();
            return;
        }
        _downTicks = 0;
        var state = h.HeadlessRunning ? "syncing…" : "idle";
        SetStatus($"Running v{h.Version} · {state}" + (h.BrowserFound ? "" : " · no browser"),
                  $"Amazon Order Sync — {state}");
        _syncNow.Enabled = !h.HeadlessRunning;

        foreach (var n in await _server.PollNotificationsAsync())
        {
            var kind = n.Title.Contains("fail", StringComparison.OrdinalIgnoreCase) || n.Title.Contains("missing", StringComparison.OrdinalIgnoreCase)
                ? ToolTipIcon.Warning : ToolTipIcon.Info;
            Toast(n.Title, n.Body, kind);
        }
    }

    // ----- startup registration ------------------------------------------

    private static string ExePath => Environment.ProcessPath ?? Application.ExecutablePath;

    private static bool IsStartupEnabled()
    {
        try
        {
            using var key = Registry.CurrentUser.OpenSubKey(RunKey, writable: false);
            var v = key?.GetValue(RunValue) as string;
            return !string.IsNullOrEmpty(v) && v.Contains(ExePath, StringComparison.OrdinalIgnoreCase);
        }
        catch { return false; }
    }

    private void SetStartup(bool enable)
    {
        try
        {
            using var key = Registry.CurrentUser.CreateSubKey(RunKey, writable: true);
            if (enable) key.SetValue(RunValue, $"\"{ExePath}\" --autostart");
            else key.DeleteValue(RunValue, throwOnMissingValue: false);
            Log.Write($"Start with Windows: {(enable ? "enabled" : "disabled")}");
        }
        catch (Exception e)
        {
            Toast("Could not change startup setting", e.Message, ToolTipIcon.Error);
        }
    }

    // ----- helpers --------------------------------------------------------

    private void SetStatus(string menuText, string tooltip)
    {
        _status.Text = menuText;
        _icon.Text = tooltip.Length > 63 ? tooltip[..63] : tooltip; // NotifyIcon.Text max 63 chars
    }

    private void Toast(string title, string body, ToolTipIcon kind)
    {
        try
        {
            if (!_icon.Visible) return;
            _icon.ShowBalloonTip(5000, title, string.IsNullOrWhiteSpace(body) ? " " : body, kind);
        }
        catch (Exception e) { Log.Write($"Toast failed: {e.Message}"); }
    }

    private static void Safe(Action a)
    {
        try { a(); }
        catch (Exception e)
        {
            Log.Write($"Action failed: {e}");
            MessageBox.Show(e.Message, "Amazon Order Sync", MessageBoxButtons.OK, MessageBoxIcon.Error);
        }
    }

    private static Icon LoadIcon()
    {
        try
        {
            using var s = typeof(TrayContext).Assembly.GetManifestResourceStream("app.ico");
            if (s != null) return new Icon(s);
        }
        catch { }
        return SystemIcons.Application;
    }

    protected override void Dispose(bool disposing)
    {
        if (disposing)
        {
            _poll.Dispose();
            _menu.Dispose();
            _icon.Dispose();
            _server.Dispose();
        }
        base.Dispose(disposing);
    }
}
