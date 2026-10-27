var builder = WebApplication.CreateBuilder(args);
var app = builder.Build();

var orders = Enumerable.Range(1001, 40)
    .Select(id => new Order(id, $"Order {id}", Math.Round(20 + id % 17 * 7.5, 2)))
    .ToList();

app.MapGet("/", () => "Orders API");

app.MapGet("/api/orders", (ILogger<Program> log, int page = 1) =>
{
    log.LogInformation("Listing orders, page {Page}", page);
    return orders.Skip((page - 1) * 10).Take(10);
});

app.MapGet("/api/orders/{id:int}", (int id, ILogger<Program> log) =>
{
    var order = orders.FirstOrDefault(order => order.Id == id);
    if (order is null)
        return Results.NotFound();
    log.LogInformation("Loaded order {Id} ({Total})", id, order.Total);
    return Results.Ok(order);
});

app.MapPost("/api/orders", (ILogger<Program> log) =>
{
    var order = new Order(orders.Max(order => order.Id) + 1, "New order", 49.90);
    orders.Add(order);
    log.LogInformation("Created order {Id}", order.Id);
    return Results.Created($"/api/orders/{order.Id}", order);
});

app.MapGet("/api/orders/report", async (ILogger<Program> log) =>
{
    log.LogInformation("Building the monthly report");
    await Task.Delay(1400);
    log.LogWarning("The report scanned all {Count} orders - it needs an index", orders.Count);
    return Results.Ok(new { orders.Count, Total = orders.Sum(order => order.Total) });
});

app.Run();

record Order(int Id, string Name, double Total);
