// lazyrunner's faults on the calls between apps (#105). lazyrunner builds this into a startup hook and loads it into a
// .NET app (DOTNET_STARTUP_HOOKS) once a fault is set on one of its calls, or on the calls that come to it. Its own
// calls: it listens to HttpClient's diagnostics; for a call a fault in LAZYRUNNER_FAULTS matches, it waits (delay),
// throws (drop), or sends the call to lazyrunner's fault server instead (fail, mock). The calls that come to it from a
// browser app (their Origin or Referer is that app's port): a hosting startup (ASPNETCORE_HOSTINGSTARTUPASSEMBLIES)
// puts a middleware first in its pipeline that waits, drops the connection, or answers with the fault server's answer
// and the CORS headers a browser needs to see it. The app's own code doesn't change.
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Net.Http;
using System.Runtime.Loader;
using System.Text.Json;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.AspNetCore.Builder;
using Microsoft.AspNetCore.Hosting;
using Microsoft.AspNetCore.Http;
using Microsoft.Extensions.DependencyInjection;

[assembly: HostingStartup(typeof(Arrivals))]

internal static class StartupHook
{
    public static void Initialize()
    {
        try
        {
            var own = typeof(StartupHook).Assembly;
            AssemblyLoadContext.Default.Resolving += (context, name) => name.Name == own.GetName().Name ? own : null;
            DiagnosticListener.AllListeners.Subscribe(new Listeners());
        }
        catch
        {
        }
    }
}

/// The faults in LAZYRUNNER_FAULTS, read again as the file changes; the fault server's port; which app this is.
internal static class Faults
{
    private static readonly string? Source = Environment.GetEnvironmentVariable("LAZYRUNNER_FAULTS");
    public static readonly string App = Environment.GetEnvironmentVariable("LAZYRUNNER_APP") ?? "";
    private static readonly object Gate = new object();
    private static DateTime read;
    private static List<JsonElement> list = new List<JsonElement>();
    public static int Port;

    public static List<JsonElement> Current()
    {
        lock (Gate)
        {
            Load();
            return list;
        }
    }

    public static string Text(JsonElement fault, string name) =>
        fault.TryGetProperty(name, out var value) && value.ValueKind == JsonValueKind.String ? value.GetString() ?? "" : "";

    public static int Number(JsonElement fault, string name) =>
        fault.TryGetProperty(name, out var value) && value.ValueKind == JsonValueKind.Number ? value.GetInt32() : 0;

    public static bool Rolled(JsonElement fault) => Random.Shared.Next(100) < Number(fault, "share");

    /// A delay's wait: its ms, or somewhere up to its ms_max (a slow network).
    public static int Wait(JsonElement fault)
    {
        var least = Number(fault, "ms");
        var most = Number(fault, "ms_max");
        return most > least ? Random.Shared.Next(least, most + 1) : least;
    }

    public static bool Local(string host) =>
        host == "localhost" || host == "127.0.0.1" || host == "::1" || host == "[::1]";

    public static string Answering(JsonElement fault) =>
        $"http://127.0.0.1:{Port}/__lazyrunner_fault/{fault.GetProperty("id").GetInt64()}";

    private static void Load()
    {
        if (Source == null || !System.IO.File.Exists(Source))
        {
            list = new List<JsonElement>();
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
            Port = document.RootElement.GetProperty("port").GetInt32();
            var loaded = new List<JsonElement>();
            foreach (var fault in document.RootElement.GetProperty("faults").EnumerateArray())
            {
                loaded.Add(fault.Clone());
            }
            list = loaded;
            read = when;
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

/// The app's own calls.
internal sealed class Calls : IObserver<KeyValuePair<string, object?>>
{
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
        switch (Faults.Text(fault, "kind"))
        {
            case "delay":
                Thread.Sleep(Faults.Wait(fault));
                break;
            case "drop":
                throw new HttpRequestException("lazyrunner dropped this call: a fault on its stack map (K)");
            default:
                request.RequestUri = new Uri(Faults.Answering(fault));
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
        foreach (var fault in Faults.Current())
        {
            if (Faults.Text(fault, "caller") != Faults.App || !Targets(Faults.Text(fault, "target"), uri))
            {
                continue;
            }
            if (!uri.AbsolutePath.StartsWith(Faults.Text(fault, "path"), StringComparison.OrdinalIgnoreCase))
            {
                continue;
            }
            if (Faults.Rolled(fault))
            {
                return fault;
            }
        }
        return null;
    }

    private static bool Targets(string target, Uri uri)
    {
        var at = target.LastIndexOf(':');
        var host = at > 0 ? target.Substring(0, at) : target;
        var wanted = at > 0 ? target.Substring(at + 1) : "";
        var same = string.Equals(host, uri.Host, StringComparison.OrdinalIgnoreCase) || (Faults.Local(host) && Faults.Local(uri.Host));
        return same && (wanted.Length == 0 || wanted == uri.Port.ToString());
    }
}

/// The calls that come to it: a middleware first in its pipeline.
internal sealed class Arrivals : IHostingStartup, IStartupFilter
{
    private const string Faulted = "x-lazyrunner-faulted";
    private static readonly HttpClient Client = new HttpClient();

    public void Configure(IWebHostBuilder builder) =>
        builder.ConfigureServices(services => services.Insert(0, ServiceDescriptor.Transient<IStartupFilter, Arrivals>()));

    public Action<IApplicationBuilder> Configure(Action<IApplicationBuilder> next) => app =>
    {
        app.Use(Arrive);
        next(app);
    };

    private static async Task Arrive(HttpContext context, Func<Task> next)
    {
        var found = Find(context.Request);
        if (found == null)
        {
            await next();
            return;
        }
        var fault = found.Value;
        switch (Faults.Text(fault, "kind"))
        {
            case "delay":
                await Task.Delay(Faults.Wait(fault));
                await next();
                return;
            case "drop":
                context.Abort();
                return;
        }
        using var answer = await Client.GetAsync(Faults.Answering(fault));
        var response = context.Response;
        response.StatusCode = (int)answer.StatusCode;
        response.ContentType = answer.Content.Headers.ContentType?.ToString() ?? "application/json";
        response.Headers["x-lazyrunner-fault"] = fault.GetProperty("id").GetInt64().ToString();
        var origin = context.Request.Headers["Origin"].ToString();
        if (origin.Length > 0)
        {
            response.Headers["Access-Control-Allow-Origin"] = origin;
            response.Headers["Access-Control-Allow-Credentials"] = "true";
            response.Headers["Vary"] = "Origin";
        }
        var body = await answer.Content.ReadAsByteArrayAsync();
        await response.Body.WriteAsync(body, 0, body.Length);
    }

    /// The fault on a call from a browser app: its Origin (or Referer) is that app's port. A preflight passes, and so
    /// does a call a Node app's hook faulted on its way already.
    private static JsonElement? Find(HttpRequest request)
    {
        if (HttpMethods.IsOptions(request.Method) || request.Headers.ContainsKey(Faulted))
        {
            return null;
        }
        var from = PortOf(request.Headers["Origin"].ToString()) ?? PortOf(request.Headers["Referer"].ToString());
        if (from == null)
        {
            return null;
        }
        var path = request.PathBase.Add(request.Path).Value ?? "/";
        foreach (var fault in Faults.Current())
        {
            if (Faults.Text(fault, "to_app") != Faults.App || Faults.Number(fault, "from_port") != from)
            {
                continue;
            }
            if (!path.StartsWith(Faults.Text(fault, "path"), StringComparison.OrdinalIgnoreCase))
            {
                continue;
            }
            if (Faults.Rolled(fault))
            {
                return fault;
            }
        }
        return null;
    }

    private static int? PortOf(string url) =>
        Uri.TryCreate(url, UriKind.Absolute, out var uri) && Faults.Local(uri.Host) ? uri.Port : null;
}
