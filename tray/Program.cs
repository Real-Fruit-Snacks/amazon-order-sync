using System.Threading;

namespace AmazonOrderSync.Tray;

internal static class Program
{
    private const string MutexName = @"Local\AmazonOrderSync.Tray.SingleInstance";

    [STAThread]
    private static void Main(string[] args)
    {
        using var mutex = new Mutex(initiallyOwned: true, MutexName, out bool createdNew);
        if (!createdNew)
        {
            // A second launch just pokes the running instance (open dashboard).
            try { Paths.OpenInBrowser(Paths.DashboardUrl(Settings.Load().Port)); } catch { /* ignore */ }
            return;
        }

        ApplicationConfiguration.Initialize();
        Application.SetUnhandledExceptionMode(UnhandledExceptionMode.CatchException);
        Application.ThreadException += (_, e) => Log.Write($"UI thread exception: {e.Exception}");
        AppDomain.CurrentDomain.UnhandledException += (_, e) => Log.Write($"Unhandled exception: {e.ExceptionObject}");

        Log.Write($"Tray starting v{Paths.Version} (args: {string.Join(' ', args)})");
        Application.Run(new TrayContext());
        Log.Write("Tray exited");
    }
}
