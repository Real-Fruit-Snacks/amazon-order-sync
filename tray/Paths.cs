using System.Diagnostics;
using System.Reflection;
using System.Text.Json;

namespace AmazonOrderSync.Tray;

/// <summary>Where everything lives. Mirrors server.js's Windows layout:
/// config under %APPDATA%, mutable data under %LOCALAPPDATA%.</summary>
internal static class Paths
{
    public const string AppName = "amazon-order-sync";

    public static string Version =>
        Assembly.GetExecutingAssembly().GetName().Version?.ToString(3) ?? "2.2.0";

    public static string ExeDir =>
        AppContext.BaseDirectory.TrimEnd(Path.DirectorySeparatorChar);

    public static string ConfigDir =>
        Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), AppName);

    public static string ConfigFile => Path.Combine(ConfigDir, "config.json");

    public static string DataDir =>
        Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), AppName);

    public static string ServerLog => Path.Combine(DataDir, "server.log");
    public static string TrayLog => Path.Combine(DataDir, "tray.log");

    public static string DashboardUrl(int port) => $"http://127.0.0.1:{port}/";

    /// <summary>The Node server entry point. AOS_SERVER_DIR wins; otherwise walk
    /// up from the exe looking for server\server.js (the exe sits at the
    /// project root next to server\ and browser-extension\).</summary>
    public static string? FindServerScript()
    {
        var env = Environment.GetEnvironmentVariable("AOS_SERVER_DIR");
        if (!string.IsNullOrWhiteSpace(env))
        {
            var p = Path.Combine(env, "server.js");
            if (File.Exists(p)) return p;
        }
        var dir = new DirectoryInfo(ExeDir);
        for (int i = 0; i < 5 && dir != null; i++, dir = dir.Parent)
        {
            var candidate = Path.Combine(dir.FullName, "server", "server.js");
            if (File.Exists(candidate)) return candidate;
        }
        return null;
    }

    /// <summary>node.exe: PATH first, then the standard installer locations.</summary>
    public static string? FindNode()
    {
        var pathEnv = Environment.GetEnvironmentVariable("PATH") ?? "";
        foreach (var dir in pathEnv.Split(';', StringSplitOptions.RemoveEmptyEntries))
        {
            try
            {
                var p = Path.Combine(dir.Trim(), "node.exe");
                if (File.Exists(p)) return p;
            }
            catch { /* malformed PATH entry */ }
        }
        var candidates = new[]
        {
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), "nodejs", "node.exe"),
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFilesX86), "nodejs", "node.exe"),
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Programs", "nodejs", "node.exe"),
        };
        foreach (var c in candidates) if (File.Exists(c)) return c;
        return null;
    }

    public static string ExpandHome(string p)
    {
        if (p == "~") return Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
        if (p.StartsWith("~/") || p.StartsWith("~\\"))
            return Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), p[2..]);
        return p;
    }

    public static void OpenInBrowser(string url) =>
        Process.Start(new ProcessStartInfo(url) { UseShellExecute = true });

    public static void OpenFolder(string path)
    {
        Directory.CreateDirectory(path);
        Process.Start(new ProcessStartInfo("explorer.exe", $"\"{path}\"") { UseShellExecute = true });
    }

    public static void OpenFile(string path)
    {
        if (!File.Exists(path)) File.WriteAllText(path, "");
        Process.Start(new ProcessStartInfo("notepad.exe", $"\"{path}\"") { UseShellExecute = true });
    }
}

/// <summary>The subset of config.json the tray needs. Everything else is the
/// server's business; we never rewrite the file.</summary>
internal sealed class Settings
{
    public int Port { get; init; } = 27483;
    public string NotesPath { get; init; } =
        Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), "AmazonOrders");

    public static Settings Load()
    {
        try
        {
            if (!File.Exists(Paths.ConfigFile)) return new Settings();
            using var doc = JsonDocument.Parse(File.ReadAllText(Paths.ConfigFile));
            var root = doc.RootElement;
            int port = 27483;
            if (root.TryGetProperty("port", out var p) && p.ValueKind == JsonValueKind.Number && p.TryGetInt32(out var pv) && pv > 0 && pv < 65536)
                port = pv;
            string notes = new Settings().NotesPath;
            if (root.TryGetProperty("notesPath", out var n) && n.ValueKind == JsonValueKind.String && !string.IsNullOrWhiteSpace(n.GetString()))
                notes = Paths.ExpandHome(n.GetString()!);
            return new Settings { Port = port, NotesPath = notes };
        }
        catch (Exception e)
        {
            Log.Write($"config.json unreadable, using defaults: {e.Message}");
            return new Settings();
        }
    }

    /// <summary>First run: write a default config so the server and tray agree
    /// on the notes path. Never overwrites an existing file.</summary>
    public static void EnsureDefault(string notesPath)
    {
        if (File.Exists(Paths.ConfigFile)) return;
        Directory.CreateDirectory(Paths.ConfigDir);
        var json = JsonSerializer.Serialize(new
        {
            notesPath,
            geminiApiKey = "",
            port = 27483,
            autoSyncEnabled = false,
            autoSyncIntervalHours = 24,
            notificationsEnabled = true
        }, new JsonSerializerOptions { WriteIndented = true });
        File.WriteAllText(Paths.ConfigFile, json + Environment.NewLine);
        Log.Write($"Wrote default config to {Paths.ConfigFile}");
    }
}

internal static class Log
{
    private static readonly object Gate = new();

    public static void Write(string line)
    {
        try
        {
            lock (Gate)
            {
                Directory.CreateDirectory(Paths.DataDir);
                var fi = new FileInfo(Paths.TrayLog);
                if (fi.Exists && fi.Length > 2 * 1024 * 1024)
                    File.Move(Paths.TrayLog, Paths.TrayLog + ".1", overwrite: true);
                File.AppendAllText(Paths.TrayLog, $"{DateTime.Now:yyyy-MM-dd HH:mm:ss} {line}{Environment.NewLine}");
            }
        }
        catch { /* logging is best-effort */ }
    }
}
