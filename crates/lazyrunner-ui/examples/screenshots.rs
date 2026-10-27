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
use lazyrunner_core::testing::{TempDir, sample_profile, sample_repo};
use lazyrunner_ui::screen::{self, Overlay, Ui};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use serde_json::json;

const SIZE: (u16, u16) = (170, 40);
/// A Windows Terminal pane or a laptop screen: narrow enough that the apps stack.
const NARROW: (u16, u16) = (100, 40);
const THEME: &str = "catppuccin-mocha";
/// Cell size and font in the SVG (px): a 14 px monospace font is about 8.4 px wide.
const CELL: (f64, f64) = (8.4, 18.0);
const FONT_SIZE: f64 = 14.0;
const FONT: &str = "'Cascadia Mono', 'Cascadia Code', Consolas, 'DejaVu Sans Mono', Menlo, monospace";
/// The window around the screen: padding and the title bar.
const PAD: f64 = 14.0;
const TITLE_BAR: f64 = 34.0;
/// A terminal's default text and background when the theme leaves them to it.
const DEFAULT_FG: &str = "#cccccc";
const DEFAULT_BG: &str = "#0c0c0c";

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let out = std::env::args()
        .nth(1)
        .map_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/screenshots"), PathBuf::from);
    std::fs::create_dir_all(&out).expect("the screenshots folder");
    let scenes: [(&str, Scene, (u16, u16)); 9] = [
        ("overview", overview, SIZE),
        ("palette", palette, SIZE),
        ("durations", durations, SIZE),
        ("themes", themes, SIZE),
        ("stacked", overview, NARROW),
        ("tests", tests, SIZE),
        ("migrations", migrations, SIZE),
        ("requests", requests, SIZE),
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
}

// ---------------------------------------------------------------- the scenes

/// What a screenshot shows, on top of every app running.
type Scene = fn(&mut Demo);

fn overview(_: &mut Demo) {}

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
    demo.ui.overlay = Some(Overlay::Durations { top: 0 });
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
    (orders.status, orders.last_run, orders.seconds) = (SuiteStatus::Failed, Some(at), 6.4);
    let mut billing = Suite::new("dotnet:billing", "Billing.Tests", dotnet("Billing.Tests"));
    billing.set_tests(vec![
        test("Billing.Tests.InvoiceTests.Sums", Outcome::Passed, 9.0, "", ""),
        test("Billing.Tests.InvoiceTests.RoundsTheTax", Outcome::Passed, 6.0, "", ""),
    ]);
    (billing.status, billing.last_run, billing.seconds) = (SuiteStatus::Passed, Some(at), 2.1);
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
        }
        demo.controller.requests.add(app, request, today(9, 40, second));
    }
    demo.press(KeyCode::Char('H'));
    for _ in 0..5 {
        demo.press(KeyCode::Char('j'));
    }
    demo.press(KeyCode::Enter);
}

fn about(demo: &mut Demo) {
    demo.ui.overlay = Some(Overlay::About { index: 0 });
}

/// A time today (the views show the day too for an earlier one).
fn today(hour: u32, minute: u32, second: u32) -> chrono::DateTime<chrono::Local> {
    let at = chrono::Local::now().date_naive().and_hms_opt(hour, minute, second).expect("a time");
    at.and_local_timezone(chrono::Local).single().expect("a local time")
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
    _temp: TempDir,
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
        Self { _temp: temp, controller, ui }
    }

    fn press(&mut self, code: KeyCode) {
        screen::keys::press(&mut self.ui, &mut self.controller, KeyEvent::new(code, KeyModifiers::NONE));
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
            self.controller.log.push_back(LogEntry { time: "09:41:07".into(), text: text.into(), hue });
        }
    }

    /// The screen as a terminal window: an SVG of every cell, in the theme's colours.
    fn svg(&mut self, size: (u16, u16)) -> String {
        let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).expect("a test terminal");
        terminal.draw(|frame| screen::draw(frame, &mut self.ui, &self.controller)).expect("a frame");
        let (fg, bg) = self.ui.theme.terminal.clone().unwrap_or((None, None));
        let fg = fg.unwrap_or_else(|| DEFAULT_FG.into());
        let bg = bg.unwrap_or_else(|| DEFAULT_BG.into());
        render_svg(terminal.backend().buffer(), &fg, &bg)
    }
}

// ---------------------------------------------------------------- the buffer as SVG

/// One run of cells in the same colours and weight.
struct Run {
    x: u16,
    cells: u16,
    text: String,
    fg: String,
    bg: Option<String>,
    bold: bool,
    underline: bool,
}

fn render_svg(buffer: &Buffer, default_fg: &str, default_bg: &str) -> String {
    let area = buffer.area;
    let width = PAD * 2.0 + f64::from(area.width) * CELL.0;
    let height = TITLE_BAR + PAD * 2.0 + f64::from(area.height) * CELL.1;
    let mut svg = String::new();
    let _ = write!(
        svg,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width:.0}" height="{height:.0}" viewBox="0 0 {width:.0} {height:.0}" font-family="{FONT}" font-size="{FONT_SIZE}">
<rect width="100%" height="100%" rx="10" fill="{default_bg}"/>
<rect width="100%" height="{TITLE_BAR}" rx="10" fill="#000000" fill-opacity="0.28"/>
<circle cx="22" cy="17" r="6" fill="#ff5f57"/><circle cx="42" cy="17" r="6" fill="#febc2e"/><circle cx="62" cy="17" r="6" fill="#28c840"/>
<text x="{:.0}" y="22" fill="{default_fg}" fill-opacity="0.6" font-size="13" text-anchor="middle">lazyrunner — shop</text>
"##,
        width / 2.0
    );
    for y in 0..area.height {
        let runs = row_runs(buffer, y, default_fg);
        let top = TITLE_BAR + PAD + f64::from(y) * CELL.1;
        for run in &runs {
            if let Some(bg) = &run.bg {
                let _ = writeln!(
                    svg,
                    r#"<rect x="{:.1}" y="{top:.1}" width="{:.1}" height="{}" fill="{bg}"/>"#,
                    PAD + f64::from(run.x) * CELL.0,
                    f64::from(run.cells) * CELL.0,
                    CELL.1
                );
            }
        }
        let baseline = top + CELL.1 * 0.76;
        for run in runs.iter().filter(|run| !run.text.trim().is_empty()) {
            let weight = if run.bold { r#" font-weight="bold""# } else { "" };
            let underline = if run.underline { r#" text-decoration="underline""# } else { "" };
            let _ = writeln!(
                svg,
                r#"<text x="{:.1}" y="{baseline:.1}" fill="{}" textLength="{:.1}" lengthAdjust="spacingAndGlyphs"{weight}{underline} xml:space="preserve">{}</text>"#,
                PAD + f64::from(run.x) * CELL.0,
                run.fg,
                f64::from(run.cells) * CELL.0,
                escape(&run.text)
            );
        }
    }
    svg.push_str("</svg>\n");
    svg
}

fn row_runs(buffer: &Buffer, y: u16, default_fg: &str) -> Vec<Run> {
    let area = buffer.area;
    let mut runs: Vec<Run> = Vec::new();
    for x in 0..area.width {
        let cell = &buffer[(x, y)];
        let (mut fg, mut bg) = (hex(cell.fg), hex(cell.bg));
        if cell.modifier.contains(Modifier::REVERSED) {
            (fg, bg) = (bg.or_else(|| Some(DEFAULT_BG.into())), fg.or_else(|| Some(default_fg.into())));
        }
        let fg = fg.unwrap_or_else(|| default_fg.into());
        let bold = cell.modifier.contains(Modifier::BOLD);
        let underline = cell.modifier.contains(Modifier::UNDERLINED);
        match runs.last_mut() {
            Some(run) if run.fg == fg && run.bg == bg && run.bold == bold && run.underline == underline => {
                run.cells += 1;
                run.text.push_str(cell.symbol());
            }
            _ => runs.push(Run { x, cells: 1, text: cell.symbol().to_string(), fg, bg, bold, underline }),
        }
    }
    runs
}

/// A colour as #rrggbb; `None` is the terminal's default.
fn hex(color: Color) -> Option<String> {
    let (r, g, b) = match color {
        Color::Reset => return None,
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(n) => xterm(n),
        named => xterm(ansi_index(named)),
    };
    Some(format!("#{r:02x}{g:02x}{b:02x}"))
}

fn ansi_index(color: Color) -> u8 {
    match color {
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::Gray => 7,
        Color::DarkGray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        _ => 15,
    }
}

/// The xterm 256-colour palette (the first 16 as Windows Terminal's Campbell scheme has them).
fn xterm(n: u8) -> (u8, u8, u8) {
    const BASIC: [(u8, u8, u8); 16] = [
        (12, 12, 12),
        (197, 15, 31),
        (19, 161, 14),
        (193, 156, 0),
        (0, 55, 218),
        (136, 23, 152),
        (58, 150, 221),
        (204, 204, 204),
        (118, 118, 118),
        (231, 72, 86),
        (22, 198, 12),
        (249, 241, 165),
        (59, 120, 255),
        (180, 0, 158),
        (97, 214, 214),
        (242, 242, 242),
    ];
    match n {
        0..=15 => BASIC[usize::from(n)],
        16..=231 => {
            let n = n - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (level(n / 36), level((n / 6) % 6), level(n % 6))
        }
        _ => {
            let gray = 8 + (n - 232) * 10;
            (gray, gray, gray)
        }
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
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
