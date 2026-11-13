// lazyrunner's faults on the calls between apps (#105). lazyrunner builds this into a startup hook and loads it into a
// .NET app (DOTNET_STARTUP_HOOKS) once a fault is set on one of its calls. It listens to HttpClient's diagnostics; for
// a call a fault in LAZYRUNNER_FAULTS matches, it waits (delay), throws (drop), or sends the call to lazyrunner's
// fault server instead (fail, mock). The app's own code doesn't change.
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Net.Http;
using System.Text.Json;
using System.Threading;

internal static class StartupHook
{
    public static void Initialize()
    {
        try
        {
            DiagnosticListener.AllListeners.Subscribe(new Listeners());
        }
        catch
        {
        }
    }
}

internal sealed class Listeners : IObserver<DiagnosticListener>
{
    public void OnNext(DiagnosticListener listener)
    {
        if (listener.Name == "HttpHandlerDiagnosticListener")
        {
            listener.Subscribe(new Calls(), name => name == "System.Net.Http.Request");
        }
    }

    public void OnError(Exception error)
    {
    }

    public void OnCompleted()
    {
    }
}

internal sealed class Calls : IObserver<KeyValuePair<string, object?>>
{
    private static readonly string? Source = Environment.GetEnvironmentVariable("LAZYRUNNER_FAULTS");
    private static readonly string App = Environment.GetEnvironmentVariable("LAZYRUNNER_APP") ?? "";
    private static readonly object Gate = new object();
    private static DateTime read;
    private static List<JsonElement> faults = new List<JsonElement>();
    private static int port;

    public void OnNext(KeyValuePair<string, object?> pair)
    {
        var request = pair.Value?.GetType().GetProperty("Request")?.GetValue(pair.Value) as HttpRequestMessage;
        var uri = request?.RequestUri;
        if (request == null || uri == null || !uri.IsAbsoluteUri)
        {
            return;
        }
        var found = Find(uri);
        if (found == null)
        {
            return;
        }
        var fault = found.Value;
        switch (fault.GetProperty("kind").GetString())
        {
            case "delay":
                var least = fault.GetProperty("ms").GetInt32();
                var most = fault.TryGetProperty("ms_max", out var max) ? max.GetInt32() : least;
                Thread.Sleep(most > least ? Random.Shared.Next(least, most + 1) : least);
                break;
            case "drop":
                throw new HttpRequestException("lazyrunner dropped this call: a fault on its stack map (K)");
            default:
                request.RequestUri = new Uri($"http://127.0.0.1:{port}/__lazyrunner_fault/{fault.GetProperty("id").GetInt64()}");
                break;
        }
    }

    public void OnError(Exception error)
    {
    }

    public void OnCompleted()
    {
    }

    private static JsonElement? Find(Uri uri)
    {
        lock (Gate)
        {
            Load();
            foreach (var fault in faults)
            {
                if (fault.GetProperty("caller").GetString() != App || !Targets(fault.GetProperty("target").GetString() ?? "", uri))
                {
                    continue;
                }
                if (!uri.AbsolutePath.StartsWith(fault.GetProperty("path").GetString() ?? "", StringComparison.OrdinalIgnoreCase))
                {
                    continue;
                }
                return Random.Shared.Next(100) < fault.GetProperty("share").GetInt32() ? fault : null;
            }
            return null;
        }
    }

    private static bool Local(string host) => host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "[::1]";

    private static bool Targets(string target, Uri uri)
    {
        var at = target.LastIndexOf(':');
        var host = at > 0 ? target.Substring(0, at) : target;
        var wanted = at > 0 ? target.Substring(at + 1) : "";
        var same = string.Equals(host, uri.Host, StringComparison.OrdinalIgnoreCase) || (Local(host) && Local(uri.Host));
        return same && (wanted.Length == 0 || wanted == uri.Port.ToString());
    }

    private static void Load()
    {
        if (Source == null || !System.IO.File.Exists(Source))
        {
            faults = new List<JsonElement>();
            return;
        }
        var when = System.IO.File.GetLastWriteTimeUtc(Source);
        if (when == read)
        {
            return;
        }
        try
        {
            using var document = JsonDocument.Parse(System.IO.File.ReadAllText(Source));
            port = document.RootElement.GetProperty("port").GetInt32();
            var loaded = new List<JsonElement>();
            foreach (var fault in document.RootElement.GetProperty("faults").EnumerateArray())
            {
                loaded.Add(fault.Clone());
            }
            faults = loaded;
            read = when;
        }
        catch
        {
        }
    }
}
