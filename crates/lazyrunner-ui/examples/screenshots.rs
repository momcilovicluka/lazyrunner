//! The README's screenshots: lazyrunner's real screens, drawn headless for a made-up shop project (the test sample:
//! two .NET APIs and three npm apps) and written as terminal-window SVGs to docs/screenshots.
//!
//! Run it after visible changes (not on every build): `cargo run -p lazyrunner-ui --example screenshots`.
//! The folder, branch, database, clock and log times are fixed, so a run changes only what the screens changed.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use lazyrunner_core::output::Hue;
use lazyrunner_core::paths::Paths;
use lazyrunner_core::profile::{ProfileDirs, import};
use lazyrunner_core::run::Controller;
use lazyrunner_core::run::app::Status;
use lazyrunner_core::run::controller::{LogEntry, Setup};
use lazyrunner_core::run::history::Kind;
use lazyrunner_core::run::migrations::{Context, Migration, Status as MigrationStatus};
use lazyrunner_core::run::requests::Finished;
use lazyrunner_core::run::suites::{Outcome, Runner, Status as SuiteStatus, Suite, TestResult};
use lazyrunner_core::testing::{BILLING_PORT, ORDERS_PORT, TempDir, sample_profile, sample_repo};
use lazyrunner_ui::screen::overlays::LOGO;
use lazyrunner_ui::screen::{self, Overlay, Ui};
use lazyrunner_ui::svg::{self, FONT};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use serde_json::json;

const SIZE: (u16, u16) = (170, 40);
/// A Windows Terminal pane or a laptop screen: narrow enough that the apps stack.
const NARROW: (u16, u16) = (100, 40);
const THEME: &str = "catppuccin-mocha";
/// The made-up machine's memory: not this one's.
const MACHINE_MEMORY_MB: f32 = 16_384.0;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let out = std::env::args()
        .nth(1)
        .map_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/screenshots"), PathBuf::from);
    std::fs::create_dir_all(&out).expect("the screenshots folder");
    let scenes: [(&str, Scene, (u16, u16)); 19] = [
        ("overview", overview, SIZE),
        ("languages", languages, SIZE),
        ("palette", palette, SIZE),
        ("durations", durations, SIZE),
        ("themes", themes, SIZE),
        ("stacked", overview, NARROW),
        ("tests", tests, SIZE),
        ("migrations", migrations, SIZE),
        ("requests", requests, SIZE),
        ("stack", stack, SIZE),
        ("faults", faults, SIZE),
        ("health", health, SIZE),
        ("codemap", codemap, SIZE),
        ("problems", problems, SIZE),
        ("doctor", doctor, SIZE),
        ("database", database, SIZE),
        ("spec", spec, SIZE),
        ("timeline", timeline, SIZE),
        ("about", about, SIZE),
    ];
    for (name, scene, size) in scenes {
        let mut demo = Demo::new(name);
        demo.running();
        scene(&mut demo);
        let path = out.join(format!("{name}.svg"));
        std::fs::write(&path, demo.svg(size)).expect("the screenshot");
        println!("wrote {}", path.display());
    }
    let path = out.join("banner.svg");
    std::fs::write(&path, banner()).expect("the banner");
    println!("wrote {}", path.display());
}

// ---------------------------------------------------------------- the README's banner

/// The banner's logo: one square per half block.
const PIXEL: f64 = 14.0;
const BANNER: (f64, f64) = (860.0, 190.0);
/// catppuccin-mocha's red, yellow, green, sky, blue and mauve, the About box's rainbow.
const RAINBOW: [&str; 6] = ["#f38ba8", "#f9e2af", "#a6e3a1", "#89dceb", "#89b4fa", "#cba6f7"];

/// The README's banner: the logo drawn in squares (no font can change it), in the About box's rainbow, and the
/// tagline under it.
fn banner() -> String {
    let (width, height) = BANNER;
    let columns = LOGO[0].chars().count() as f64;
    let left = (width - columns * PIXEL) / 2.0;
    let top = 34.0;
    let stops: String = RAINBOW
        .iter()
        .enumerate()
        .map(|(at, color)| {
            format!(r#"<stop offset="{:.2}" stop-color="{color}"/>"#, at as f64 / (RAINBOW.len() - 1) as f64)
        })
        .collect();
    let mut svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width:.0}" height="{height:.0}" viewBox="0 0 {width:.0} {height:.0}" font-family="{FONT}">
<defs><linearGradient id="rainbow" gradientUnits="userSpaceOnUse" x1="{left:.0}" y1="0" x2="{:.0}" y2="0">{stops}</linearGradient></defs>
<rect width="100%" height="100%" rx="14" fill="#1e1e2e"/>
<g fill="url(#rainbow)" shape-rendering="crispEdges">
"##,
        left + columns * PIXEL
    );
    for (row, line) in LOGO.iter().enumerate() {
        for (column, block) in line.chars().enumerate() {
            let (upper, lower) = match block {
                '█' => (true, true),
                '▀' => (true, false),
                '▄' => (false, true),
                _ => (false, false),
            };
            for (half, filled) in [(0.0, upper), (1.0, lower)] {
                if filled {
                    let (x, y) = (left + column as f64 * PIXEL, top + (row as f64 * 2.0 + half) * PIXEL);
                    let _ = writeln!(svg, r#"<rect x="{x:.0}" y="{y:.0}" width="{PIXEL:.0}" height="{PIXEL:.0}"/>"#);
                }
            }
        }
    }
    let tagline = top + LOGO.len() as f64 * 2.0 * PIXEL + 42.0;
    let _ = write!(
        svg,
        r##"</g>
<text x="{:.0}" y="{tagline:.0}" fill="#bac2de" font-size="18" text-anchor="middle">Your .NET APIs and npm frontends, side by side, from one terminal</text>
</svg>
"##,
        width / 2.0
    );
    svg
}

// ---------------------------------------------------------------- the scenes

/// What a screenshot shows, on top of every app running.
type Scene = fn(&mut Demo);

fn overview(_: &mut Demo) {}

/// A Python app and Docker Compose services beside the .NET and npm apps: Docs becomes Infra (postgres, redis and
/// rabbitmq, a gutter each), Console becomes Pricing (uvicorn, a traceback with its library frames dimmed).
fn languages(demo: &mut Demo) {
    use lazyrunner_core::profile::{AppKind, Command, Compose, Ready};
    use lazyrunner_core::run::compose::{Service, State};
    use lazyrunner_core::run::{Event, Tag};
    let command = |run: &[&str], frames: Option<&str>, errors: &[&str], compose: Option<Compose>| Command {
        run: run.iter().map(|word| word.to_string()).collect(),
        cwd: String::new(),
        port: None,
        build: Vec::new(),
        install: Vec::new(),
        setup: Vec::new(),
        install_from: Vec::new(),
        ready: Ready::Started,
        watch: Vec::new(),
        errors: errors.iter().map(|error| error.to_string()).collect(),
        warnings: Vec::new(),
        frames: frames.map(str::to_string),
        preset: None,
        path: Vec::new(),
        tools: Vec::new(),
        compose,
    };
    let python = command(
        &["uvicorn", "pricing.api:app", "--port", "8000"],
        Some(r#"File "(?P<file>[^"]+)", line (?P<line>\d+)"#),
        &[r"^[A-Za-z_][\w.]*(Error|Exception)(: |$)", r"\b(ERROR|CRITICAL)\b"],
        None,
    );
    let services = Compose { file: None, services: Vec::new() };
    let infra = command(&["docker", "compose", "up"], None, &[r"(?i)\b(error|fatal)\b"], Some(services));
    for (index, name, kind) in [(4, "Pricing", python), (3, "Infra", infra)] {
        let app = &mut demo.controller.apps[index];
        (app.name, app.icon, app.kind) = (name.into(), None, AppKind::Command(Box::new(kind)));
        let state = &mut demo.controller.states[index];
        state.output.clear();
        (state.errors, state.warnings) = (0, 0);
    }
    let said = |demo: &mut Demo, index: usize, lines: &[&str]| {
        let key = demo.controller.apps[index].key.clone();
        for line in lines {
            let tag = Tag::App { key: key.clone(), generation: 1 };
            demo.controller.handle(Event::Output { tag, line: line.to_string() });
        }
    };
    said(demo, 4, PRICING);
    said(demo, 3, INFRA);
    let service = |name: &str, state, detail: &str, image: &str| Service {
        name: name.into(),
        state,
        detail: detail.into(),
        ports: Vec::new(),
        image: image.into(),
    };
    let polled = vec![
        service("cache", State::Up, "running (healthy)", "redis:7"),
        service("db", State::Up, "running (healthy)", "postgres:16"),
        service("queue", State::Starting, "running (starting)", "rabbitmq:3-management"),
    ];
    let key = demo.controller.apps[3].key.clone();
    demo.controller.handle(Event::ComposeServices { key, generation: 1, services: Ok(polled) });
}

const PRICING: &[&str] = &[
    "INFO:     Started server process [8124]",
    "INFO:     Waiting for application startup.",
    "INFO:     Application startup complete.",
    "INFO:     Uvicorn running on http://127.0.0.1:8000",
    "INFO:     127.0.0.1:52144 - \"GET /prices/basket HTTP/1.1\" 200 OK",
    "ERROR:    Exception in ASGI application",
    "Traceback (most recent call last):",
    "  File \"C:\\work\\shop\\.venv\\Lib\\site-packages\\starlette\\routing.py\", line 74, in app",
    "  File \"C:\\work\\shop\\.venv\\Lib\\site-packages\\fastapi\\routing.py\", line 301, in run",
    "  File \"C:\\work\\shop\\pricing\\api.py\", line 31, in quote",
    "    return rules.apply(basket)",
    "  File \"C:\\work\\shop\\pricing\\rules.py\", line 12, in apply",
    "    raise ValueError(f\"no price rule for SKU {sku}\")",
    "ValueError: no price rule for SKU 4711",
];

const INFRA: &[&str] = &[
    "Attaching to cache-1, db-1, queue-1",
    "db-1     | PostgreSQL init process complete; ready for start up.",
    "db-1     | LOG:  database system is ready to accept connections",
    "cache-1  | * Ready to accept connections tcp",
    "queue-1  | Starting RabbitMQ 3.13.7 on Erlang 26.2.5",
    "queue-1  | Enabling plugins on node rabbit@queue: rabbitmq_management",
    "db-1     | LOG:  checkpoint starting: time",
    "queue-1  | Server startup complete; 3 plugins started.",
];

fn palette(demo: &mut Demo) {
    demo.ui.overlay = Some(Overlay::Palette { query: "start".into(), index: 0 });
}

fn durations(demo: &mut Demo) {
    let runs: [(Kind, &str, &str, &[f64]); 5] = [
        (Kind::Build, "orders", "Orders", &[21.4, 19.8, 22.1, 20.3, 23.0, 21.7, 34.6]),
        (Kind::Build, "billing", "Billing", &[9.2, 8.7, 9.9, 9.1, 8.8, 9.4]),
        (Kind::Start, "orders", "Orders", &[4.1, 3.8, 4.4, 3.9, 4.2]),
        (Kind::Start, "shop", "Shop", &[6.2, 5.9, 4.1, 4.0, 3.8]),
        (Kind::NpmCi, "shop", "Shop", &[48.0, 52.3, 47.1]),
    ];
    for (kind, key, label, seconds) in runs {
        for (n, seconds) in seconds.iter().enumerate() {
            let failed = kind == Kind::Build && key == "orders" && n == 3;
            demo.controller.durations.add(kind, key, label, *seconds, !failed);
        }
    }
    demo.ui.overlay = Some(Overlay::Durations { index: 1, picked: demo.ui.now });
}

fn themes(demo: &mut Demo) {
    demo.press(KeyCode::Char('t'));
}

/// The tests view: the backend's suites (one with a failure), the frontend's, and one Nx project without tests.
fn tests(demo: &mut Demo) {
    let test = |name: &str, outcome, ms, message: &str, stack: &str| TestResult {
        name: name.into(),
        outcome,
        ms,
        message: message.into(),
        stack: stack.into(),
    };
    let at = today(9, 38, 52);
    let dotnet = |name: &str| Runner::Dotnet { project: format!("/work/shop/tests/{name}/{name}.csproj").into() };
    let mut orders = Suite::new("dotnet:orders", "Orders.Tests", dotnet("Orders.Tests"));
    orders.set_tests(vec![
        test("Orders.Tests.CartTests.AddsAnItem", Outcome::Passed, 12.0, "", ""),
        test("Orders.Tests.CartTests.AppliesTheVoucher", Outcome::Passed, 18.0, "", ""),
        test(
            "Orders.Tests.CartTests.Totals(quantity: 2, price: 3)",
            Outcome::Failed,
            41.0,
            "Assert.Equal() Failure: Values differ\nExpected: 6\nActual:   5",
            "at Shop.Orders.Tests.CartTests.Totals(Int32 quantity, Decimal price) in /work/shop/tests/Orders.Tests/CartTests.cs:line 42",
        ),
        test("Orders.Tests.CheckoutTests.PaysTheInvoice", Outcome::Passed, 230.0, "", ""),
        test("Orders.Tests.CheckoutTests.RetriesThePayment", Outcome::Skipped, 0.0, "", ""),
    ]);
    (orders.status, orders.last_run, orders.seconds) = (SuiteStatus::Failed, Some(at.clone()), 6.4);
    let mut billing = Suite::new("dotnet:billing", "Billing.Tests", dotnet("Billing.Tests"));
    billing.set_tests(vec![
        test("Billing.Tests.InvoiceTests.Sums", Outcome::Passed, 9.0, "", ""),
        test("Billing.Tests.InvoiceTests.RoundsTheTax", Outcome::Passed, 6.0, "", ""),
    ]);
    (billing.status, billing.last_run, billing.seconds) = (SuiteStatus::Passed, Some(at.clone()), 2.1);
    let mut shop = Suite::new("npm:shop", "Shop", Runner::Npm { dir: "/work/shop/web/shop".into(), vitest: true });
    shop.set_tests(vec![
        test("CartPage > shows the total", Outcome::Passed, 31.0, "", ""),
        test("CartPage > removes an item", Outcome::Passed, 22.0, "", ""),
    ]);
    (shop.status, shop.last_run, shop.seconds) = (SuiteStatus::Passed, Some(at), 3.8);
    let mut ui =
        Suite::new("nx:ui", "shared-ui", Runner::Nx { workspace: "/work/shop/web".into(), project: "ui".into() });
    (orders.app, billing.app, shop.app) = (Some("orders".into()), Some("billing".into()), Some("shop".into()));
    (ui.app, ui.untested) = (Some("shop".into()), true);
    demo.controller.suites = Some(vec![orders, billing, shop, ui]);
    demo.press(KeyCode::Char('T'));
}

/// The migrations view: the shop's database up to date but for one migration, the audit log's up to date.
fn migrations(demo: &mut Demo) {
    let migration = |id: &str, applied| Migration { id: id.into(), name: id[15..].into(), applied: Some(applied) };
    let project = PathBuf::from("/work/shop/services/Data/Data.csproj");
    let startup = PathBuf::from("/work/shop/services/Orders/Orders.csproj");
    let mut shop = Context::new("orders", "ShopDbContext", project.clone(), startup.clone());
    shop.items = vec![
        migration("20260112093011_Init", true),
        migration("20260203141522_AddOrders", true),
        migration("20260311101944_AddVouchers", true),
        migration("20260520160307_InvoiceNumbers", true),
        migration("20261019114025_AddLoyaltyPoints", false),
    ];
    let mut audit = Context::new("orders", "AuditDbContext", project, startup);
    audit.items = vec![migration("20260112093514_Init", true), migration("20260808084210_TrackRefunds", true)];
    for context in [&mut shop, &mut audit] {
        context.status = MigrationStatus::Ok;
        (context.checked_at, context.db) = (Some(today(9, 41, 7)), "ShopLocal @ localhost".into());
    }
    demo.controller.contexts = Some(vec![shop, audit]);
    demo.ui.overlay = Some(Overlay::Migrations { index: 0, picking: None });
}

/// What Billing logged while it failed the slow request: two queries, the second slow, then the exception.
const FAILED_LOG: [(&str, Hue); 10] = [
    ("info: Microsoft.EntityFrameworkCore.Database.Command[20101]", Hue::Gray),
    (
        "      Executed DbCommand (11ms) [Parameters=[@__id_0='1042'], CommandType='Text', CommandTimeout='30']",
        Hue::Gray,
    ),
    ("      SELECT TOP(1) [i].[Id], [i].[Total] FROM [Invoices] AS [i] WHERE [i].[Id] = @__id_0", Hue::Gray),
    ("info: Microsoft.EntityFrameworkCore.Database.Command[20101]", Hue::Gray),
    (
        "      Executed DbCommand (1488ms) [Parameters=[@__id_0='1042'], CommandType='Text', CommandTimeout='30']",
        Hue::Gray,
    ),
    ("      SELECT [l].[Id], [l].[Amount] FROM [InvoiceLines] AS [l] WHERE [l].[InvoiceId] = @__id_0", Hue::Gray),
    ("fail: Microsoft.AspNetCore.Diagnostics.DeveloperExceptionPageMiddleware[1]", Hue::Red),
    ("      An unhandled exception has occurred while executing the request.", Hue::Red),
    ("      System.InvalidOperationException: Invoice 1042 has no currency.", Hue::Red),
    ("         at Billing.Invoices.InvoiceTotals.Sum(Invoice invoice) in InvoiceTotals.cs:line 31", Hue::Red),
];

/// The requests view: what the APIs served over the last minute, a slow one and a failed one among them - that one
/// selected, with what it logged.
fn requests(demo: &mut Demo) {
    served(demo, today(9, 40, 0));
    demo.press(KeyCode::Char('H'));
    for _ in 0..5 {
        demo.press(KeyCode::Char('j'));
    }
    demo.press(KeyCode::Enter);
}

/// What the APIs served over the last minute: a slow request and a failed one among them, with what it logged.
fn served(demo: &mut Demo, start: jiff::ZonedDateTime) {
    let served = [
        ("orders", "GET", "/api/orders?page=1", 200, 14.2, 0),
        ("orders", "GET", "/api/orders/1042", 200, 7.1, 4),
        ("billing", "GET", "/api/invoices", 200, 9.4, 9),
        ("orders", "POST", "/api/orders", 201, 42.8, 15),
        ("billing", "GET", "/api/invoices/1042", 500, 1532.0, 21),
        ("orders", "GET", "/api/orders/1043", 200, 6.3, 28),
        ("orders", "PUT", "/api/orders/1043/items", 204, 31.5, 33),
        ("billing", "GET", "/api/invoices/1041", 200, 5.2, 41),
        ("orders", "GET", "/api/orders?page=2", 200, 12.0, 47),
        ("orders", "GET", "/api/vouchers/SPRING", 404, 3.9, 52),
    ];
    let now = demo.ui.now;
    for (app, method, path, status, ms, second) in served {
        let mut request = Finished {
            method: method.into(),
            path: path.into(),
            status,
            ms,
            length: "512".into(),
            content: "application/json".into(),
            proto: "HTTP/1.1".into(),
            ..Finished::default()
        };
        if method != "GET" {
            (request.sent_content, request.sent_length) = ("application/json".into(), "86".into());
        }
        if let Some(state) = demo.controller.states.iter_mut().find(|state| state.key == app) {
            let from = state.output.total() + 1;
            if status == 500 {
                for (line, hue) in FAILED_LOG {
                    state.output.push_hued(line, Some(hue), now);
                }
            }
            request.logs = Some((from, state.output.total()));
            request.db = lazyrunner_core::run::requests::db_summary(
                state.output.numbered(from, state.output.total()).map(|line| line.text.as_str()),
            );
        }
        demo.controller.requests.add(app, request, &start + jiff::SignedDuration::from_secs(second));
    }
}

/// The stack map (K) on Shop: the apps' calls to each other over the last minute, a fault on Shop's to Billing.
fn stack(demo: &mut Demo) {
    demo.calls();
    demo.fault();
    demo.ui.focus = demo.index("shop");
    demo.press(KeyCode::Char('K'));
}

/// The faults box (f on the map) over Shop's calls: the fault already set listed, the chaos preset picked.
fn faults(demo: &mut Demo) {
    stack(demo);
    demo.press(KeyCode::Char('f'));
    demo.press(KeyCode::Up);
    for _ in 0..CHAOS_PRESET {
        demo.press(KeyCode::Right);
    }
}

/// K's Health tab: Orders' memory over its target for a minute and a half (an alert), the slowest endpoints.
fn health(demo: &mut Demo) {
    use lazyrunner_core::run::health::Metric;
    served(demo, jiff::ZonedDateTime::now() - jiff::SignedDuration::from_mins(1));
    let orders = demo.index("orders");
    let state = &mut demo.controller.states[orders];
    state.memory_mb = Some(ORDERS_MEMORY);
    state.memory_history.clear();
    for n in 0..30 {
        state.memory_history.push_back(ORDERS_MEMORY - 1200.0 + n as f32 * 40.0);
    }
    let now = demo.ui.now;
    for ago in [90, 30] {
        let reading = Some((f64::from(ORDERS_MEMORY), MEMORY_TARGET));
        demo.controller.health.step("orders", Metric::Memory, reading, now - Duration::from_secs(ago));
    }
    for code in [KeyCode::Char('K'), KeyCode::Tab, KeyCode::Tab, KeyCode::Char('v')] {
        demo.press(code);
    }
}

/// K's code map, every project: the shop's .NET projects and TypeScript libraries, what uses what, a cycle between
/// two libraries and a broken layer rule.
fn codemap(demo: &mut Demo) {
    use lazyrunner_core::run::code::{Break, CodeMap, Link};
    let link = |from, to| Link { from, to, lazy: false, implicit: false };
    let mut links: Vec<Link> = CODE_LINKS.iter().map(|&(from, to)| link(from, to)).collect();
    links.push(Link { from: 6, to: 9, lazy: true, implicit: false });
    let why = "layers: Billing.Core must not reference Orders.Core".to_string();
    let breaks = vec![Break { from: 3, to: 1, why }];
    demo.controller.set_code_map(CodeMap { parts: code_parts(), links, breaks, files: Vec::new() });
    for code in [KeyCode::Char('K'), KeyCode::Tab, KeyCode::Char('a')] {
        demo.press(code);
    }
}

/// The code map's parts: the .NET projects, the TypeScript apps and libraries, then the profile's apps.
fn code_parts() -> Vec<lazyrunner_core::run::code::Part> {
    use lazyrunner_core::run::code::{Kind as Part, STACK, Stack};
    let part = |id: &str, name: &str, kind: Part, app: Option<&str>| {
        let stack = if id.starts_with("node:") || app.is_some_and(|app| NODE_APPS.contains(&app)) {
            Stack::Node
        } else {
            Stack::Dotnet
        };
        let inside = id.starts_with("app:").then(|| STACK.to_string());
        lazyrunner_core::run::code::Part {
            id: id.into(),
            name: name.into(),
            kind,
            stack,
            path: PathBuf::from(format!("/work/shop/{}", id.split_once(':').map_or(id, |(_, path)| path))),
            framework: (stack == Stack::Dotnet && inside.is_none()).then(|| "net10.0".to_string()),
            inside,
            app: app.map(str::to_string),
            files: 12 + name.len(),
            packages: if stack == Stack::Dotnet { 3 } else { 0 },
            modules: if stack == Stack::Node { 4 } else { 0 },
            tags: Vec::new(),
        }
    };
    let dotnet = |name: &str| format!("dotnet:services/{name}/{name}.csproj");
    vec![
        part(&dotnet("Orders.Api"), "Orders.Api", Part::App, Some("orders")),
        part(&dotnet("Orders.Core"), "Orders.Core", Part::Library, None),
        part(&dotnet("Billing.Api"), "Billing.Api", Part::App, Some("billing")),
        part(&dotnet("Billing.Core"), "Billing.Core", Part::Library, None),
        part(&dotnet("Shared.Kernel"), "Shared.Kernel", Part::Library, None),
        part("dotnet:tests/Orders.Tests/Orders.Tests.csproj", "Orders.Tests", Part::Tests, None),
        part("node:web/apps/shop", "shop", Part::App, Some("shop")),
        part("node:web/apps/console", "console", Part::App, Some("console")),
        part("node:web/libs/ui", "ui", Part::Library, Some("shop")),
        part("node:web/libs/data", "data", Part::Library, Some("shop")),
        part("app:orders", "Orders", Part::App, Some("orders")),
        part("app:billing", "Billing", Part::App, Some("billing")),
        part("app:shop", "Shop", Part::App, Some("shop")),
        part("app:console", "Console", Part::App, Some("console")),
    ]
}

/// The Problems screen (E): Billing's unhandled exception four times (its stack in the details), Orders' slow
/// payment provider and missing orders, the shop's broken import.
fn problems(demo: &mut Demo) {
    let now = demo.ui.now;
    let ago = |seconds: u64| now - Duration::from_secs(seconds);
    for (seconds, invoice) in [(170, 1042), (95, 1047), (40, 1051), (12, 1058)] {
        demo.said("billing", &filled(UNHANDLED, invoice), Hue::Red, ago(seconds));
    }
    for (seconds, order) in [(140, 1077), (61, 1081)] {
        demo.said("orders", &filled(NOT_FOUND, order), Hue::Red, ago(seconds));
    }
    for (seconds, ms) in [(120, 1204), (50, 954)] {
        demo.said("orders", &filled(SLOW_PAYMENT, ms), Hue::Yellow, ago(seconds));
    }
    demo.said("shop", &filled(UNRESOLVED_IMPORT, 0), Hue::Red, ago(25));
    demo.press(KeyCode::Char('E'));
}

/// An entry's lines with `{id}` filled in.
fn filled(lines: &[&str], id: u32) -> Vec<String> {
    lines.iter().map(|line| line.replace("{id}", &id.to_string())).collect()
}

/// The doctor (`,` then `r`): every check passed or warned, each group open, the first warning picked.
fn doctor(demo: &mut Demo) {
    use lazyrunner_core::run::checks::Status;
    use lazyrunner_ui::screen::checklist::{Checklist, Item};
    let version = format!("v{}", lazyrunner_core::changelog::VERSION);
    let mut checks = vec![("lazyrunner", Status::Ok, version.as_str(), 240)];
    checks.extend(CHECKS);
    let items =
        checks.iter().map(|&(name, status, detail, _)| Item { name: name.into(), status, detail: detail.into() });
    let mut list = Checklist::fixed(items.collect(), Vec::new(), "");
    for (row, &(.., ms)) in list.rows.iter_mut().zip(&checks) {
        row.took = Some(Duration::from_millis(ms));
    }
    let logged = demo.controller.log.len();
    list.update(demo.ui.now, &mut demo.controller);
    demo.controller.log.truncate(logged); // its warnings, logged at the real time
    list.selected = checks.iter().position(|&(_, status, ..)| status == Status::Warn).unwrap_or(0);
    demo.ui.overlay = Some(Overlay::Checklist(Box::new(list)));
}

/// The database screen (d): the user secrets' connection strings, the local one in use, a server not answering.
fn database(demo: &mut Demo) {
    use lazyrunner_ui::screen::Picker;
    use lazyrunner_ui::screen::database::{Reach, SWITCHED};
    let path = demo.temp.join("secrets.json");
    std::fs::write(&path, SECRETS).expect("the made-up secrets");
    let groups = lazyrunner_core::run::secrets::groups(SECRETS);
    let mut picker =
        Picker { path, groups, group: 0, index: 0, top: 0, filter: String::new(), filtering: false, new_name: None };
    picker.index = picker.active_index();
    let reach = [("localhost", Reach::Up(2)), ("dev-sql", Reach::Up(38)), ("qa-sql", Reach::Down)];
    let reach = reach.map(|(server, reach)| (server.to_string(), reach));
    demo.ui.db_reach = Some(std::sync::Arc::new(std::sync::Mutex::new(reach.into_iter().collect())));
    for (label, minutes) in [("ShopLocal @ localhost", 12), ("Shop @ dev-sql", 47)] {
        let at = jiff::ZonedDateTime::now() - jiff::SignedDuration::from_mins(minutes);
        demo.controller.config[SWITCHED][label] = json!(lazyrunner_core::time::rfc3339(&at));
    }
    demo.ui.picker = Some(picker);
    demo.buffer(SIZE); // the answers come in; a moment later they no longer flash
    demo.ui.now += Duration::from_secs(1);
}

/// H's spec tab: what the APIs' OpenAPI documents say they serve, by tag - how often each endpoint was called this
/// session and its p95, the ones never called.
fn spec(demo: &mut Demo) {
    use lazyrunner_ui::screen::spec::Loaded;
    served(demo, today(9, 40, 0));
    for (key, document) in [("orders", orders_document()), ("billing", billing_document())] {
        let operations = lazyrunner_core::run::openapi::parse(&document);
        demo.ui.specs.insert(key.into(), Loaded::Ready { from: "/swagger/v1/swagger.json".into(), operations });
    }
    for code in [KeyCode::Char('H'), KeyCode::Tab, KeyCode::Tab] {
        demo.press(code);
    }
}

/// An operation of an OpenAPI document: its tag, summary and path parameter.
fn operation(tag: &str, summary: &str, parameter: Option<&str>) -> serde_json::Value {
    let parameters: Vec<serde_json::Value> = parameter
        .map(|name| json!({ "name": name, "in": "path", "required": true, "schema": { "type": "string" } }))
        .into_iter()
        .collect();
    json!({ "tags": [tag], "summary": summary, "parameters": parameters })
}

fn orders_document() -> serde_json::Value {
    json!({ "paths": {
        "/api/orders": { "get": operation("Orders", "A page of orders", None),
                         "post": operation("Orders", "Place an order", None) },
        "/api/orders/{id}": { "get": operation("Orders", "One order", Some("id")),
                              "delete": operation("Orders", "Cancel it", Some("id")) },
        "/api/orders/{id}/items": { "put": operation("Orders", "Replace its items", Some("id")) },
        "/api/vouchers": { "get": operation("Vouchers", "Every voucher", None),
                           "post": operation("Vouchers", "Issue a voucher", None) },
    } })
}

fn billing_document() -> serde_json::Value {
    json!({ "paths": {
        "/api/invoices": { "get": operation("Invoices", "Every invoice", None) },
        "/api/invoices/{id}": { "get": operation("Invoices", "One invoice", Some("id")) },
        "/api/invoices/{id}/send": { "post": operation("Invoices", "Email it", Some("id")) },
    } })
}

/// The timeline (~): every app's output in the order it came - the start, a few requests, Billing failing an invoice
/// and Orders hearing of it - quiet stretches as dim rows.
fn timeline(demo: &mut Demo) {
    for state in &mut demo.controller.states {
        state.output.clear();
    }
    let start = demo.ui.now - Duration::from_secs(TIMELINE_SPAN);
    for (key, ms, line, hue) in TIMELINE {
        let index = demo.index(key);
        demo.controller.states[index].add_line(line, hue, start + Duration::from_millis(ms));
    }
    demo.press(KeyCode::Char('~'));
}

fn about(demo: &mut Demo) {
    demo.ui.overlay = Some(Overlay::About { index: 0 });
}

// ---------------------------------------------------------------- the new scenes' data

/// Chaos is the faults box's fifth preset.
const CHAOS_PRESET: usize = 4;
/// The share of Shop's calls to Billing the fault fails, and every how many of them failed already.
const FAULT_SHARE: u8 = 30;
const FAILING_EVERY: usize = 3;
const ORDERS_MEMORY: f32 = 3000.0;
const MEMORY_TARGET: f64 = 2048.0;
const NODE_APPS: [&str; 2] = ["shop", "console"];
/// Who calls whom, what, how often over the last minute and how fast.
const CALLS: [(&str, u16, &str, &str, usize, f64); 4] = [
    ("shop", ORDERS_PORT, "GET", "/api/orders?page=1", 18, 14.0),
    ("shop", BILLING_PORT, "GET", "/api/invoices/1042", 7, 9.0),
    ("orders", BILLING_PORT, "POST", "/api/invoices", 5, 22.0),
    ("console", ORDERS_PORT, "GET", "/api/orders/stats", 4, 31.0),
];
/// The code map's links, by part: the APIs use their cores, the cores the kernel, Billing.Core reaches into
/// Orders.Core (the rule it breaks), the web apps use ui and data, and ui and data use each other.
const CODE_LINKS: [(usize, usize); 11] =
    [(0, 1), (0, 4), (1, 4), (2, 3), (3, 4), (3, 1), (5, 1), (6, 8), (7, 8), (8, 9), (9, 8)];

const UNHANDLED: &[&str] = &[
    "fail: Microsoft.AspNetCore.Diagnostics.DeveloperExceptionPageMiddleware[1]",
    "      An unhandled exception has occurred while executing the request.",
    "      System.InvalidOperationException: Invoice {id} has no currency.",
    "         at Shop.Billing.Invoices.InvoiceTotals.Sum(Invoice invoice) in /work/shop/services/Billing.Core/Invoices/InvoiceTotals.cs:line 31",
    "         at Shop.Billing.Api.InvoicesController.Get(Int32 id) in /work/shop/services/Billing.Api/InvoicesController.cs:line 48",
    "         at Microsoft.AspNetCore.Mvc.Infrastructure.ActionMethodExecutor.TaskOfIActionResultExecutor.Execute(ActionContext context)",
];
const NOT_FOUND: &[&str] = &["fail: Shop.Orders.Api[0]", "      Order {id} was not found"];
const SLOW_PAYMENT: &[&str] = &["warn: Shop.Orders.Checkout[0]", "      Payment provider slow ({id} ms)"];
const UNRESOLVED_IMPORT: &[&str] = &[
    "09:41:22 [vite] Internal server error: Failed to resolve import \"./Coupon\" from \"src/cart/CartPage.tsx\". Does the file exist?",
    "  Plugin: vite:import-analysis",
    "  File: /work/shop/web/apps/shop/src/cart/CartPage.tsx:4:19",
];

/// The doctor's checks after lazyrunner's own: a name, how it went, what it found and how long it took (ms).
const CHECKS: [(&str, lazyrunner_core::run::checks::Status, &str, u64); 7] = {
    use lazyrunner_core::run::checks::Status::{Ok, Warn};
    [
        ("git", Ok, "2.47.1", 110),
        (".NET SDK", Ok, "10.0.100", 420),
        ("Node.js", Ok, "22.11.0", 90),
        ("Ports", Ok, "47201, 47202, 47301 and 47303 are free", 30),
        ("Profile", Ok, "Shop from this repo's lazyrunner.json", 30),
        ("Web packages", Warn, "web/node_modules is older than package-lock.json - press i (npm ci)", 60),
        ("Database", Warn, "ShopLocal @ localhost · 1 migration to apply - M shows it", 180),
    ]
};

/// The made-up user secrets: the shop's database on this machine (in use), on the dev and QA servers, and a copy.
const SECRETS: &str = r#"{
  "DefaultConnection": "Server=localhost;Database=ShopLocal;Trusted_Connection=True;TrustServerCertificate=True",
  //"DefaultConnection": "Server=localhost;Database=ShopLocal_Copy;Trusted_Connection=True;TrustServerCertificate=True",
  //"DefaultConnection": "Server=dev-sql;Database=Shop;User Id=shop_dev;Password=made-up-dev",
  //"DefaultConnection": "Server=qa-sql;Database=ShopQA;User Id=shop_qa;Password=made-up-qa",
  "AuditConnection": "Server=localhost;Database=ShopAudit;Trusted_Connection=True;TrustServerCertificate=True"
}
"#;

/// The timeline's stretch, and what each app printed in it: when (ms in), the line and its level's colour.
const TIMELINE_SPAN: u64 = 60;
const TIMELINE: [(&str, u64, &str, Hue); 23] = [
    ("orders", 0, "info: Microsoft.Hosting.Lifetime[14]", Hue::Gray),
    ("orders", 0, "      Now listening on: http://localhost:47201", Hue::Gray),
    ("billing", 380, "info: Microsoft.Hosting.Lifetime[14]", Hue::Gray),
    ("billing", 380, "      Now listening on: http://localhost:47202", Hue::Gray),
    ("shop", 1100, "  VITE v5.4.10  ready in 812 ms", Hue::Gray),
    ("shop", 1100, "  ➜  Local:   http://localhost:47301/", Hue::Gray),
    ("console", 1600, "  VITE v5.4.10  ready in 604 ms", Hue::Gray),
    ("docs", 3500, "built in 1204ms.", Hue::Gray),
    ("orders", 12_000, "info: Shop.Orders.Api[0]", Hue::Gray),
    ("orders", 12_000, "      GET /api/orders?page=1 200 14 ms", Hue::Gray),
    ("shop", 18_200, "09:40:12 [vite] hmr update /src/cart/CartPage.tsx", Hue::Gray),
    ("orders", 24_500, "info: Shop.Orders.Api[0]", Hue::Gray),
    ("orders", 24_500, "      POST /api/orders 201 42 ms", Hue::Gray),
    ("billing", 24_560, "info: Shop.Billing.Api[0]", Hue::Gray),
    ("billing", 24_560, "      POST /api/invoices 201 18 ms", Hue::Gray),
    ("orders", 31_000, "warn: Shop.Orders.Checkout[0]", Hue::Yellow),
    ("orders", 31_000, "      Payment provider slow (812 ms)", Hue::Yellow),
    ("billing", 33_200, "fail: Shop.Billing.Invoices[0]", Hue::Red),
    ("billing", 33_200, "      Invoice 1042 has no currency", Hue::Red),
    ("orders", 33_260, "fail: Shop.Orders.Checkout[0]", Hue::Red),
    ("orders", 33_260, "      Billing answered 500 for order 1042", Hue::Red),
    ("orders", 47_000, "info: Shop.Orders.Api[0]", Hue::Gray),
    ("orders", 47_000, "      GET /api/orders/1043 200 6 ms", Hue::Gray),
];

/// A time today (the views show the day too for an earlier one).
fn today(hour: i8, minute: i8, second: i8) -> jiff::ZonedDateTime {
    let at = jiff::ZonedDateTime::now().date().at(hour, minute, second, 0);
    at.to_zoned_datetime(jiff::tz::TimeZone::system()).expect("a local time")
}

// ---------------------------------------------------------------- the made-up project

/// One app of the made-up project: how long it took to come up, what it costs, how fast it answers (0: no
/// probe, a watch build), and what it printed.
struct DemoApp {
    key: &'static str,
    up_after: f64,
    cpu: f32,
    memory: f32,
    ms: u64,
    lines: &'static [&'static str],
}

struct Demo {
    temp: TempDir,
    controller: Controller,
    ui: Ui,
}

impl Demo {
    fn new(label: &str) -> Self {
        let temp = TempDir::new(&format!("screenshots-{label}"));
        let repo = sample_repo(&temp.join("repo"));
        let paths = Paths {
            config_file: temp.join("config.json"),
            state_dir: temp.join("state"),
            profiles: ProfileDirs {
                user: temp.join("profiles"),
                bundled: temp.join("bundled"),
                only: Some(sample_profile()),
            },
        };
        // Plain icons: a browser showing the README has no Nerd Font.
        let config = json!({ "theme": THEME, "startupChecklist": false, "nerdIcons": false });
        lazyrunner_core::config::write(&paths.config_file, &config).expect("settings");
        let project = import(&sample_profile(), &paths.profiles).expect("the sample profile");
        let (events, _) = tokio::sync::mpsc::unbounded_channel();
        let mut controller =
            Controller::new(Setup { project, root: repo, paths, events, guard: None }).expect("a controller");
        controller.root = PathBuf::from("/work/shop");
        controller.branch = "feature/checkout-v2".into();
        controller.db_label = Some("ShopLocal @ localhost".into());
        controller.log.clear();
        let started = Instant::now();
        let (mut ui, _) = Ui::new(&controller, None, started);
        ui.now = started + Duration::from_secs(600);
        ui.motion.enabled = false; // one frame at one instant: a box would still be opening
        ui.machine_memory = || MACHINE_MEMORY_MB;
        Self { temp, controller, ui }
    }

    fn press(&mut self, code: KeyCode) {
        screen::keys::press(&mut self.ui, &mut self.controller, KeyEvent::new(code, KeyModifiers::NONE));
    }

    /// The column of the sample app `key`.
    fn index(&self, key: &str) -> usize {
        self.controller.apps.iter().position(|app| app.key == key).expect("a sample app")
    }

    /// An entry in app `key`'s output: its lines, in one colour, at one moment.
    fn said(&mut self, key: &str, lines: &[String], hue: Hue, at: Instant) {
        let index = self.index(key);
        for line in lines {
            self.controller.states[index].output.push_hued(line, Some(hue), at);
        }
    }

    /// The apps' calls to each other over the last minute (the stack map's edges): some of Shop's to Billing failed.
    fn calls(&mut self) {
        use lazyrunner_core::run::calls::Call;
        let now = Instant::now();
        let mut calls = Vec::new();
        for (from, port, method, path, count, ms) in CALLS {
            for n in 0..count {
                let failed = from == "shop" && port == BILLING_PORT && n % FAILING_EVERY == 0;
                let ok = if method == "POST" { 201 } else { 200 };
                calls.push(Call {
                    from: from.into(),
                    method: method.into(),
                    url: format!("http://localhost:{port}{path}"),
                    at: now - Duration::from_secs(n as u64 * 3),
                    ms: Some(ms * (1.0 + (n % 4) as f64 / 3.0)),
                    status: Some(if failed { 500 } else { ok }),
                });
            }
        }
        self.controller.calls.add(calls, now);
    }

    /// A fault on Shop's calls to Billing under /api: a share of them fail with 500. The log stays as it was.
    fn fault(&mut self) {
        use lazyrunner_core::run::faults::Kind as Fault;
        let logged = self.controller.log.len();
        let target = (format!("localhost:{BILLING_PORT}"), Some("billing".to_string()));
        self.controller.set_fault("shop", target, "/api", (Fault::Fail(500), FAULT_SHARE), None);
        self.controller.log.truncate(logged);
    }

    /// Every app up for a few minutes, with output, metrics and a log of how they got there.
    fn running(&mut self) {
        let long_ago = self.ui.now - Duration::from_secs(180);
        let shown = self.ui.now - Duration::from_secs(30);
        let apps = [
            DemoApp { key: "orders", up_after: 4.2, cpu: 3.0, memory: 212.0, ms: 12, lines: ORDERS },
            DemoApp { key: "billing", up_after: 3.7, cpu: 1.0, memory: 168.0, ms: 8, lines: BILLING },
            DemoApp { key: "shop", up_after: 3.8, cpu: 6.0, memory: 341.0, ms: 3, lines: SHOP },
            DemoApp { key: "docs", up_after: 7.9, cpu: 0.0, memory: 96.0, ms: 0, lines: DOCS },
            DemoApp { key: "console", up_after: 2.6, cpu: 2.0, memory: 129.0, ms: 4, lines: CONSOLE },
        ];
        for (index, DemoApp { key, up_after, cpu, memory, ms, lines }) in apps.into_iter().enumerate() {
            if ms > 0 {
                self.controller.answered(key, ms);
            }
            let app = self.controller.apps.iter().find(|app| app.key == key).cloned().expect("a sample app");
            let state = &mut self.controller.states[index];
            state.status = Status::Running;
            state.generation = 1;
            state.started_at = Some(long_ago);
            state.up_generation = Some(1);
            state.up_after = Some(up_after);
            for line in lines {
                state.receive(&app, line, shown);
            }
            state.flash_until = None;
            state.cpu = Some(cpu);
            state.memory_mb = Some(memory);
            for n in 0..30 {
                let wave = ((n as f32) * 0.7 + index as f32).sin().abs();
                state.cpu_history.push_back(cpu * (0.5 + wave * 3.0)); // a busy moment or two
                state.memory_history.push_back(memory - 8.0 + wave * 6.0);
                state.response_history.push_back(9.0 + wave * 14.0);
                if index < 2 {
                    state.request_history.push_back(1.0 + wave * 4.0);
                }
            }
        }
        for (text, hue) in [
            ("Starting Shop + Docs + Console", Hue::Cyan),
            ("✓ Orders built in 21s (14 projects)", Hue::Green),
            ("✓ Billing built in 9s (6 projects)", Hue::Green),
            ("Starting Orders + Billing", Hue::Cyan),
            ("● Orders is listening on :47201 - up in 4s", Hue::Green),
            ("Ready in 38s", Hue::Green),
        ] {
            self.controller.log.push_back(LogEntry { time: "09:41:07".into(), text: text.into(), hue, task: None });
        }
    }

    /// The screen as a terminal window: an SVG of every cell, in the theme's colours.
    fn svg(&mut self, size: (u16, u16)) -> String {
        let buffer = self.buffer(size);
        let (fg, bg) = self.colors();
        svg::render(&buffer, "lazyrunner — shop", &fg, &bg)
    }

    fn buffer(&mut self, size: (u16, u16)) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).expect("a test terminal");
        terminal.draw(|frame| screen::draw(frame, &mut self.ui, &self.controller)).expect("a frame");
        terminal.backend().buffer().clone()
    }

    /// The theme's text and background, or a terminal's.
    fn colors(&self) -> (String, String) {
        let (fg, bg) = self.ui.theme.terminal.clone().unwrap_or((None, None));
        (fg.unwrap_or_else(|| svg::DEFAULT_FG.into()), bg.unwrap_or_else(|| svg::DEFAULT_BG.into()))
    }
}

// ---------------------------------------------------------------- what the apps print

const ORDERS: &[&str] = &[
    "info: Microsoft.Hosting.Lifetime[14]",
    "      Now listening on: http://localhost:47201",
    "info: Microsoft.Hosting.Lifetime[0]",
    "      Application started.",
    "info: Shop.Orders.Api[0]",
    "      GET /api/orders?page=1 200 14 ms",
    "info: Shop.Orders.Api[0]",
    "      GET /api/orders/1042 200 7 ms",
    "warn: Shop.Orders.Checkout[0]",
    "      Payment provider slow (812 ms)",
    "info: Shop.Orders.Api[0]",
    "      POST /api/orders 201 42 ms",
    "info: Shop.Orders.Api[0]",
    "      GET /api/orders?page=2 200 12 ms",
    "info: Shop.Orders.Api[0]",
    "      GET /api/orders/1043 200 6 ms",
];

const BILLING: &[&str] = &[
    "info: Microsoft.Hosting.Lifetime[14]",
    "      Now listening on: http://localhost:47202",
    "info: Microsoft.Hosting.Lifetime[0]",
    "      Application started.",
    "info: Shop.Billing.Api[0]",
    "      GET /api/invoices 200 9 ms",
    "fail: Shop.Billing.Invoices[0]",
    "      Invoice 1042 has no customer",
    "info: Shop.Billing.Api[0]",
    "      GET /api/invoices/1041 200 5 ms",
];

const SHOP: &[&str] = &[
    "> shop@1.4.0 dev",
    "> vite --port 47301",
    "",
    "  VITE v5.4.10  ready in 812 ms",
    "",
    "  ➜  Local:   http://localhost:47301/",
    "  ➜  Network: use --host to expose",
    "09:39:12 [vite] hmr update /src/cart/CartPage.tsx",
    "09:40:31 [vite] hmr update /src/cart/CartPage.tsx, /src/cart/cart.css",
    "09:41:02 [vite] page reload src/main.tsx",
];

const DOCS: &[&str] = &[
    "> docs@1.4.0 watch",
    "> vite build --watch",
    "",
    "vite v5.4.10 building for production...",
    "✓ 38 modules transformed.",
    "dist/index.html                0.46 kB",
    "dist/assets/index-4f2a.css     2.10 kB",
    "dist/assets/index-9c1e.js     61.37 kB",
    "built in 1204ms.",
];

const CONSOLE: &[&str] = &[
    "> console@1.4.0 dev",
    "> vite --port 47303",
    "",
    "  VITE v5.4.10  ready in 604 ms",
    "",
    "  ➜  Local:   http://localhost:47303/",
    "09:40:55 [vite] hmr update /src/reports/Daily.tsx",
];
