var builder = WebApplication.CreateBuilder(args);
var app = builder.Build();

app.MapGet("/", () => "Billing API");

app.MapGet("/api/invoices/{id:int}", (int id, ILogger<Program> log) =>
{
    log.LogInformation("Loading invoice {Id}", id);
    if (id % 7 == 0)
        throw new InvalidOperationException($"Invoice {id} has no currency.");
    return Results.Ok(new Invoice(id, 1000 + id, Math.Round(10 + id * 3.25, 2), "EUR"));
});

app.Run();

record Invoice(int Id, int OrderId, double Amount, string Currency);
