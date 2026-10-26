using System.Diagnostics;
using System.Net.Http;
using System.Text;
using System.Text.Json;

namespace AmazonOrderSync.Tray;

internal sealed record Health(bool Ok, string Version, int Pid, string NotesPath, bool BrowserFound, bool HeadlessRunning);
internal sealed record Notification(long Index, string Title, string Body);

/// <summary>Owns the Node server child process and the localhost HTTP client.
/// The server is "adopted" rather than spawned when something already answers
/// on the port (e.g. an orphan from a crashed tray), so there is never a
/// second daemon racing on the notes folder.</summary>
internal sealed class ServerProcess : IDisposable
{
    private readonly HttpClient _http = new() { Timeout = TimeSpan.FromSeconds(4) };
    private readonly HttpClient _httpLong = new() { Timeout = TimeSpan.FromMinutes(30) };
    private Process? _child;
    private StreamWriter? _log;
    private readonly object _gate = new();

    public int Port { get; }
    public string BaseUrl => $"http://127.0.0.1:{Port}";
    public bool Adopted { get; private set; }
    public string? LastStartError { get; private set; }

    public ServerProcess(int port) { Port = port; }

    public bool IsChildRunning
    {
        get { try { return _child is { HasExited: false }; } catch { return false; } }
    }

    // ----- lifecycle ------------------------------------------------------

    public async Task<bool> StartAsync()
    {
        LastStartError = null;
        if (await HealthAsync() is { Ok: true })
        {
            Adopted = true;
            Log.Write($"Adopted an already-running server on port {Port}");
            return true;
        }
        Adopted = false;

        var node = Paths.FindNode();
        if (node is null)
        {
            LastStartError = "Node.js was not found. Install it (winget install OpenJS.NodeJS.LTS) and restart the tray app.";
            Log.Write(LastStartError);
            return false;
        }
        var script = Paths.FindServerScript();
        if (script is null)
        {
            LastStartError = $"server\\server.js was not found next to {Paths.ExeDir}. Set AOS_SERVER_DIR or reinstall.";
            Log.Write(LastStartError);
            return false;
        }

        Directory.CreateDirectory(Paths.DataDir);
        RotateServerLog();

        var psi = new ProcessStartInfo
        {
            FileName = node,
            WorkingDirectory = Path.GetDirectoryName(script)!,
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            StandardOutputEncoding = Encoding.UTF8,
            StandardErrorEncoding = Encoding.UTF8,
        };
        psi.ArgumentList.Add(script);
        // Point the server at the same config file the tray reads.
        psi.Environment["AMAZON_SYNC_CONFIG"] = Paths.ConfigFile;

        try
        {
            lock (_gate)
            {
                _log = new StreamWriter(new FileStream(Paths.ServerLog, FileMode.Append, FileAccess.Write, FileShare.ReadWrite)) { AutoFlush = true };
                _log.WriteLine($"==== {DateTime.Now:yyyy-MM-dd HH:mm:ss} starting: \"{node}\" \"{script}\"");
            }
            var proc = new Process { StartInfo = psi, EnableRaisingEvents = true };
            proc.OutputDataReceived += (_, e) => AppendServerLog(e.Data);
            proc.ErrorDataReceived += (_, e) => AppendServerLog(e.Data);
            proc.Exited += (_, _) =>
            {
                int code = -1;
                try { code = proc.ExitCode; } catch { }
                Log.Write($"Server process exited with code {code}");
                AppendServerLog($"==== exited with code {code}");
                Exited?.Invoke(this, code);
            };
            proc.Start();
            proc.BeginOutputReadLine();
            proc.BeginErrorReadLine();
            _child = proc;
            Log.Write($"Spawned node pid {proc.Id}: {script}");
        }
        catch (Exception e)
        {
            LastStartError = $"Could not start Node: {e.Message}";
            Log.Write(LastStartError);
            return false;
        }

        // Wait up to ~8 s for /api/health.
        for (int i = 0; i < 40; i++)
        {
            if (!IsChildRunning)
            {
                LastStartError = "The server exited right after starting. See server.log (port in use? bad config.json?).";
                return false;
            }
            if (await HealthAsync() is { Ok: true }) return true;
            await Task.Delay(200);
        }
        LastStartError = "The server did not answer on its port in time. See server.log.";
        return false;
    }

    public event EventHandler<int>? Exited;

    /// <summary>Clean stop: ask the daemon to shut itself down (it kills any
    /// headless browser and releases its lock), then fall back to killing
    /// the tree. An adopted server is left alone unless <paramref name="force"/>.</summary>
    public async Task StopAsync(bool force = false)
    {
        if (Adopted && !force) return;
        try
        {
            using var cts = new CancellationTokenSource(3000);
            await _http.PostAsync($"{BaseUrl}/api/shutdown", null, cts.Token);
        }
        catch { /* not answering — fall through to kill */ }

        var child = _child;
        if (child is null) return;
        try
        {
            if (!child.WaitForExit(5000))
            {
                Log.Write("Server did not exit after /api/shutdown — killing process tree");
                child.Kill(entireProcessTree: true);
                child.WaitForExit(3000);
            }
        }
        catch (Exception e) { Log.Write($"Stop error: {e.Message}"); }
        finally
        {
            _child = null;
            lock (_gate) { _log?.Dispose(); _log = null; }
        }
    }

    // ----- API ------------------------------------------------------------

    public async Task<Health?> HealthAsync()
    {
        try
        {
            using var cts = new CancellationTokenSource(2000);
            var json = await _http.GetStringAsync($"{BaseUrl}/api/health", cts.Token);
            using var doc = JsonDocument.Parse(json);
            var r = doc.RootElement;
            return new Health(
                r.TryGetProperty("ok", out var ok) && ok.GetBoolean(),
                Str(r, "version"),
                r.TryGetProperty("pid", out var pid) && pid.TryGetInt32(out var pv) ? pv : 0,
                Str(r, "notesPath"),
                r.TryGetProperty("browserFound", out var bf) && bf.ValueKind == JsonValueKind.True,
                r.TryGetProperty("headlessRunning", out var hr) && hr.ValueKind == JsonValueKind.True);
        }
        catch { return null; }
    }

    private long _notifyCursor = -1;

    /// <summary>Poll new notifications. The first call only primes the cursor
    /// so a tray restart never replays toasts.</summary>
    public async Task<List<Notification>> PollNotificationsAsync()
    {
        var result = new List<Notification>();
        try
        {
            var url = _notifyCursor < 0 ? $"{BaseUrl}/api/notifications" : $"{BaseUrl}/api/notifications?after={_notifyCursor}";
            var json = await _http.GetStringAsync(url);
            using var doc = JsonDocument.Parse(json);
            var r = doc.RootElement;
            if (r.TryGetProperty("items", out var items) && items.ValueKind == JsonValueKind.Array && _notifyCursor >= 0)
            {
                foreach (var it in items.EnumerateArray())
                    result.Add(new Notification(it.GetProperty("i").GetInt64(), Str(it, "title"), Str(it, "body")));
            }
            if (r.TryGetProperty("next", out var next) && next.TryGetInt64(out var nv)) _notifyCursor = nv;
        }
        catch { /* server down — keep the cursor, try next tick */ }
        return result;
    }

    /// <summary>POST a JSON-less action. Returns (status, body-text).</summary>
    public async Task<(int Status, string Body)> PostAsync(string path, bool longPoll = false)
    {
        try
        {
            var client = longPoll ? _httpLong : _http;
            using var res = await client.PostAsync($"{BaseUrl}{path}", new StringContent("{}", Encoding.UTF8, "application/json"));
            return ((int)res.StatusCode, await res.Content.ReadAsStringAsync());
        }
        catch (Exception e) { return (0, e.Message); }
    }

    public static string ErrorFromBody(string body, string fallback)
    {
        try
        {
            using var doc = JsonDocument.Parse(body);
            if (doc.RootElement.TryGetProperty("error", out var err) && err.ValueKind == JsonValueKind.String)
                return err.GetString() ?? fallback;
        }
        catch { }
        return string.IsNullOrWhiteSpace(body) ? fallback : body;
    }

    private static string Str(JsonElement r, string name) =>
        r.TryGetProperty(name, out var v) && v.ValueKind == JsonValueKind.String ? v.GetString() ?? "" : "";

    // ----- logging --------------------------------------------------------

    private void AppendServerLog(string? line)
    {
        if (line is null) return;
        lock (_gate) { try { _log?.WriteLine(line); } catch { } }
    }

    private static void RotateServerLog()
    {
        try
        {
            var fi = new FileInfo(Paths.ServerLog);
            if (fi.Exists && fi.Length > 5 * 1024 * 1024)
                File.Move(Paths.ServerLog, Paths.ServerLog + ".1", overwrite: true);
        }
        catch { }
    }

    public void Dispose()
    {
        _http.Dispose();
        _httpLong.Dispose();
        lock (_gate) { _log?.Dispose(); _log = null; }
    }
}
