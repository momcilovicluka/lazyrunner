//! A connection string whose database is no longer on the server: its row says so, and `Del` only removes the
//! connection string.

use lazyrunner_core::run::controller::database_files::Sizes;
use serde_json::json;

use super::super::{Overlay, Picker};
use super::{Silent, screen};

#[tokio::test]
async fn a_database_gone_from_the_server_is_marked_and_del_offers_to_remove_only_its_connection_string() {
    let mut screen = screen("missing-db", json!({}));
    let path = screen.temp.join("secrets.json");
    let secrets = "{
  \"DefaultConnection\": \"Server=localhost;Database=Shop;Trusted_Connection=True\",
  //\"DefaultConnection\": \"Server=localhost;Database=Shop_old;Trusted_Connection=True\",
}
";
    std::fs::write(&path, secrets).unwrap();
    screen.controller.use_sql(std::sync::Arc::new(Silent), std::sync::Arc::new(Silent), path.clone());
    let groups = lazyrunner_core::run::secrets::groups(secrets);
    let picker =
        Picker { path, groups, group: 0, index: 1, top: 0, filter: String::new(), filtering: false, new_name: None };
    screen.ui.picker = Some(picker);
    let databases = vec![("Shop".to_string(), Some(1024))];
    let sizes = Sizes { databases, at: Some(screen.ui.now), error: None, reading: None };
    screen.controller.db_sizes.insert("localhost".into(), sizes);
    let frame = screen.frame();
    let row = |name: &str| frame.lines().find(|line| line.contains(name)).unwrap_or_default().to_string();
    assert!(row("Shop_old @ localhost").contains("✗ no database"), "{frame}");
    assert!(!row("Shop @ localhost").contains("✗ no database"), "{frame}");
    assert!(
        screen.controller.is_missing("localhost", "Shop_old") && !screen.controller.is_missing("localhost", "Shop")
    );

    screen.press("Delete");
    let frame = screen.frame();
    assert!(matches!(screen.ui.overlay, Some(Overlay::Confirm(_))), "asked first: {frame}");
    assert!(frame.contains("Shop_old @ localhost isn't on the server"), "{frame}");
    assert!(frame.contains("Remove its connection string from secrets.json?"), "{frame}");
    assert!(!frame.contains("Its data is gone for good"), "nothing to lose: {frame}");
}

#[tokio::test]
async fn databases_on_the_server_missing_from_secrets_json_are_offered_and_tab_fills_one_in() {
    let mut screen = screen("discovered-db", json!({}));
    let path = screen.temp.join("secrets.json");
    let secrets = "{\n  \"DefaultConnection\": \"Server=localhost;Database=Shop;Trusted_Connection=True\",\n}\n";
    std::fs::write(&path, secrets).unwrap();
    screen.controller.use_sql(std::sync::Arc::new(Silent), std::sync::Arc::new(Silent), path.clone());
    let groups = lazyrunner_core::run::secrets::groups(secrets);
    let picker = Picker {
        path: path.clone(),
        groups,
        group: 0,
        index: 0,
        top: 0,
        filter: String::new(),
        filtering: false,
        new_name: None,
    };
    screen.ui.picker = Some(picker);
    let on_server = ["master", "Shop", "Tenant_Acme", "Shop_Audit", "tempdb"];
    let databases = on_server.iter().map(|name| (name.to_string(), Some(1024))).collect();
    let sizes = Sizes { databases, at: Some(screen.ui.now), error: None, reading: None };
    screen.controller.db_sizes.insert("localhost".into(), sizes);
    assert_eq!(
        screen.controller.not_listed("localhost"),
        ["Shop_Audit", "Tenant_Acme"],
        "sorted, SQL Server's own left out"
    );
    let frame = screen.frame();
    assert!(
        frame.contains("+ 2 more on localhost not in secrets.json (n adds one): Shop_Audit, Tenant_Acme"),
        "{frame}"
    );

    screen.press("n t e n");
    let frame = screen.frame();
    assert!(frame.contains("on localhost, not in secrets.json (Tab fills one in): Tenant_Acme"), "{frame}");
    assert!(!frame.contains("Shop_Audit"), "only what matches: {frame}");
    screen.press("Tab");
    assert_eq!(screen.ui.picker.as_ref().unwrap().new_name.as_deref(), Some("Tenant_Acme"));
    screen.press("Tab");
    assert_eq!(screen.ui.picker.as_ref().unwrap().new_name.as_deref(), Some("Shop_Audit"), "Tab again: the next");
    screen.press("Enter");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("Database=Shop_Audit;"), "{text}");
    assert_eq!(screen.controller.not_listed("localhost"), ["Tenant_Acme"]);
    assert!(screen.frame().contains("+ 1 more on localhost not in secrets.json (n adds one): Tenant_Acme"));
}
