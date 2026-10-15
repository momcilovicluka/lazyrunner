# A sample repo for tests that run a real lazyrunner on a project with many apps (sample.json is its profile): two tiny
# ASP.NET APIs (real dotnet build / run) and three npm apps without packages - a dev server, a watch build that an API
# serves, and one with two variants. Returns the repo folder.
param(
    [Parameter(Mandatory)][string]$Path,
    [int]$OrdersPort = 58201,
    [int]$BillingPort = 58202,
    [int]$ShopPort = 58301,
    [int]$ConsolePort = 58303
)
$ErrorActionPreference = 'Stop'
$major = ([version]((dotnet --version) -replace '-.*$', '')).Major
$framework = "net$major.0"

function Write-File([string]$Relative, [string]$Content) {
    $file = Join-Path $Path $Relative
    New-Item -ItemType Directory -Force (Split-Path $file) | Out-Null
    [IO.File]::WriteAllText($file, $Content.Replace("`r`n", "`n"))
}

function Write-Api([string]$Name, [int]$Port) {
    Write-File "services\$Name\$Name.csproj" @"
<Project Sdk="Microsoft.NET.Sdk.Web">
  <PropertyGroup>
    <TargetFramework>$framework</TargetFramework>
    <ImplicitUsings>enable</ImplicitUsings>
    <Nullable>enable</Nullable>
  </PropertyGroup>
</Project>
"@
    Write-File "services\$Name\Program.cs" @"
var builder = WebApplication.CreateBuilder(args);
var app = builder.Build();
app.MapGet("/", () => "$Name");
app.MapGet("/swagger/index.html", () => Results.Content("<html>$Name swagger</html>", "text/html"));
app.Run();
"@
    Write-File "services\$Name\Properties\launchSettings.json" @"
{
  "profiles": {
    "$Name": {
      "commandName": "Project",
      "applicationUrl": "http://localhost:$Port",
      "environmentVariables": { "ASPNETCORE_ENVIRONMENT": "Development" }
    }
  }
}
"@
}

# An npm app: package.json with the given scripts, node_modules (lazyrunner won't start one without it; these need no
# packages) and the two fake tools the scripts run.
function Write-NpmApp([string]$Dir, [hashtable]$Scripts) {
    Write-File "$Dir\package.json" (@{ name = (Split-Path -Leaf $Dir); private = $true; scripts = $Scripts } | ConvertTo-Json)
    Write-File "$Dir\node_modules\.fake" ''
    Write-File "$Dir\serve.js" @'
// A dev server: listens on the port it is given, like vite or ng serve.
const port = Number(process.argv[2]);
require('http').createServer((req, res) => res.end('ok')).listen(port, () => console.log(`Local: http://localhost:${port}/`));
'@
    Write-File "$Dir\watch.js" @'
// A watch build that never ends: writes dist/index.html, then keeps watching.
const fs = require('fs'), path = require('path');
console.log('Building...');
setTimeout(() => {
  fs.mkdirSync(path.join(__dirname, 'dist'), { recursive: true });
  fs.writeFileSync(path.join(__dirname, 'dist', 'index.html'), '<html>built</html>');
  console.log('Build complete. Watching for file changes...');
}, 1000);
setInterval(() => {}, 1 << 30);
'@
}

New-Item -ItemType Directory -Force $Path | Out-Null
Write-File 'Sample.sln' "Microsoft Visual Studio Solution File, Format Version 12.00`n"
Write-Api 'Orders' $OrdersPort
Write-Api 'Billing' $BillingPort
Write-NpmApp 'web\shop' @{ dev = "node serve.js $ShopPort" }
Write-NpmApp 'web\docs' @{ watch = 'node watch.js' }
Write-NpmApp 'web\console' @{ dev = "node serve.js $ConsolePort" }   # no dev:mock, so its second variant runs
Write-File '.gitignore' "bin/`nobj/`ndist/`nnode_modules/`n"
& git -C $Path init -q -b main 2>&1 | Out-Null
& git -C $Path -c commit.gpgsign=false -c user.name=tests -c user.email=tests@localhost add -A 2>&1 | Out-Null
& git -C $Path -c commit.gpgsign=false -c user.name=tests -c user.email=tests@localhost commit -q -m 'sample' 2>&1 | Out-Null
$Path
