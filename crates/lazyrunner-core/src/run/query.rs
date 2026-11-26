//! A read-only query an agent asks for (#112, `query_database`): checking data is the most common side trip while
//! debugging. It's held to one SELECT before it runs - its words read without its strings, names and comments - and
//! runs where a write couldn't stick anyway: in a transaction that's rolled back, its locks and time limited, its rows
//! capped. The server writes its rows as JSON (`FOR JSON`, so numbers stay numbers) and sends them hex-encoded, so no
//! value - a `|`, a line break, a character the console can't show - can break them; its text reaches the server
//! hex-encoded too, so sqlcmd never reads it (`GO`, `$(name)`, `:!!`).

use serde_json::Value;

/// Rows an answer has by default and at most.
pub const ROWS: usize = 100;
pub const MOST_ROWS: usize = 1000;
/// A text value longer than this, in characters, is cut.
pub const CELL_CHARS: usize = 500;
/// The query's length at most, in UTF-16 units: hex-encoded, it fits on sqlcmd's command line.
pub const MOST_CHARS: usize = 4000;
/// How long the query may run, in seconds, and wait for a lock, in milliseconds.
pub const SECONDS: u32 = 30;
const LOCK_MS: u32 = 5000;
/// What starts the lines the batch writes: the columns, a row, and the end, once it's rolled back.
const COLUMNS_LINE: char = 'N';
const ROW_LINE: char = 'R';
const END_LINE: &str = "E";
/// What sqlcmd prints when the time is up (and then exits as if all went well).
const TIMED_OUT: &str = "Timeout expired";
/// Why a row that doesn't read as JSON didn't: sqlcmd cut it.
const TOO_LONG: &str =
    "a row didn't come back whole (sqlcmd reads 1 MB of one at most) - select fewer or shorter columns";
/// Words that write, run something else or reach outside the database, refused wherever they stand outside strings,
/// names and comments.
const REFUSED: [&str; 36] = [
    "ALTER",
    "BACKUP",
    "BEGIN",
    "BULK",
    "COMMIT",
    "CREATE",
    "DBCC",
    "DECLARE",
    "DELETE",
    "DENY",
    "DROP",
    "EXEC",
    "EXECUTE",
    "GRANT",
    "INSERT",
    "INTO",
    "KILL",
    "MERGE",
    "OPENDATASOURCE",
    "OPENQUERY",
    "OPENROWSET",
    "RECEIVE",
    "RECONFIGURE",
    "RESTORE",
    "REVOKE",
    "ROLLBACK",
    "SAVE",
    "SEND",
    "SET",
    "SHUTDOWN",
    "TRUNCATE",
    "UPDATE",
    "UPDATETEXT",
    "USE",
    "WAITFOR",
    "WRITETEXT",
];
/// Two words that refuse a query when they stand together, and why.
const REFUSED_PAIRS: [(&str, &str, &str); 3] = [
    ("ENABLE", "TRIGGER", "ENABLE TRIGGER changes a table"),
    ("DISABLE", "TRIGGER", "DISABLE TRIGGER changes a table"),
    ("NEXT", "VALUE", "NEXT VALUE FOR moves a sequence on"),
];
/// What starts the names of the procedures SQL Server ships (`sp_executesql`, `xp_cmdshell`).
const PROCEDURES: [&str; 2] = ["SP_", "XP_"];

/// What a query read: its columns' names (none for a column without one), its rows - each value as JSON - whether
/// it had more than were asked for, how many values were cut, and how long it took.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Read {
    pub columns: Vec<Option<String>>,
    pub rows: Vec<Vec<Value>>,
    pub more: bool,
    pub cut: usize,
    pub ms: u64,
}

/// Why a query is refused before it runs, when it is: it must be one SELECT (or WITH ... SELECT) that writes nothing.
pub fn check(sql: &str) -> Result<(), String> {
    let refused = |why: &str| Err(format!("refused before it ran: {why} - query_database runs one SELECT only"));
    if sql.trim().is_empty() {
        return refused("there's no SQL");
    }
    if sql.encode_utf16().count() > MOST_CHARS {
        return refused(&format!("it's over {MOST_CHARS} characters"));
    }
    let Some(code) = code_of(sql) else { return refused("a string, [name] or /* comment */ isn't closed") };
    let statement = code.trim_matches(|c: char| c == ';' || c.is_whitespace());
    if statement.contains(';') {
        return refused("something follows a ; (one statement only)");
    }
    let words = words(statement);
    let leading = statement.trim_start_matches(|c: char| c == '(' || c.is_whitespace());
    let first = words.first().filter(|word| leading.to_uppercase().starts_with(word.as_str()));
    if !first.is_some_and(|word| word == "SELECT" || word == "WITH") {
        return refused("it isn't a SELECT (or WITH ... SELECT)");
    }
    for (at, word) in words.iter().enumerate() {
        if word == "INTO" {
            return refused("SELECT ... INTO makes a table");
        }
        if REFUSED.contains(&word.as_str()) {
            return refused(&format!("{word} isn't allowed (a name that is one goes in [brackets])"));
        }
        if PROCEDURES.iter().any(|prefix| word.starts_with(prefix)) {
            return refused(&format!("it calls a procedure ({})", word.to_lowercase()));
        }
        let next = words.get(at + 1).map_or("", String::as_str);
        if let Some((.., why)) = REFUSED_PAIRS.iter().find(|(first, second, _)| *first == word && *second == next) {
            return refused(why);
        }
    }
    Ok(())
}

/// The SQL without its strings, quoted names and comments - each a blank - so only its own words are left; `None`
/// when one isn't closed. Block comments nest, as SQL Server reads them.
fn code_of(sql: &str) -> Option<String> {
    let chars: Vec<char> = sql.chars().collect();
    let mut code = String::with_capacity(sql.len());
    let mut at = 0;
    while at < chars.len() {
        let next = chars.get(at + 1).copied();
        match chars[at] {
            '-' if next == Some('-') => {
                while at < chars.len() && chars[at] != '\n' {
                    at += 1;
                }
                code.push(' ');
            }
            '/' if next == Some('*') => {
                let mut depth = 0;
                loop {
                    match (chars.get(at)?, chars.get(at + 1)) {
                        ('/', Some('*')) => (depth, at) = (depth + 1, at + 2),
                        ('*', Some('/')) => (depth, at) = (depth - 1, at + 2),
                        _ => at += 1,
                    }
                    if depth == 0 {
                        break;
                    }
                }
                code.push(' ');
            }
            open @ ('\'' | '"' | '[') => {
                let close = if open == '[' { ']' } else { open };
                at += 1;
                loop {
                    if *chars.get(at)? == close {
                        if chars.get(at + 1) != Some(&close) {
                            break;
                        }
                        at += 1;
                    }
                    at += 1;
                }
                at += 1;
                code.push(' ');
            }
            other => {
                code.push(other);
                at += 1;
            }
        }
    }
    Some(code)
}

/// The words of SQL without its strings and comments, in capitals: names, keywords, `@variables`, `#tables`.
fn words(code: &str) -> Vec<String> {
    let part = |c: char| c.is_alphanumeric() || matches!(c, '_' | '@' | '#' | '$');
    code.split(|c: char| !part(c)).filter(|word| !word.is_empty()).map(str::to_uppercase).collect()
}

/// The batch that runs a checked query read-only, at most `rows + 1` rows (the one more says there were more): in a
/// transaction it rolls back, waiting 5 s at most for a lock. Its columns are described first
/// (`sys.dm_exec_describe_first_result_set`, the server's own error when it can't be), a temporary table made of
/// them - a timestamp as binary, the old text and image types as their max ones - and filled from the query; then the
/// columns' names, each row as JSON (a CLR value - geography, hierarchyid - as its text) and the end are written, each
/// as hex on a line of its own.
pub fn batch(sql: &str, rows: usize) -> String {
    let hex: String = sql.encode_utf16().flat_map(u16::to_le_bytes).map(|byte| format!("{byte:02X}")).collect();
    let column = "QUOTENAME(CONCAT(N'c', n))";
    format!(
        "SET NOCOUNT ON; SET XACT_ABORT ON; SET LOCK_TIMEOUT {LOCK_MS}; \
         SET TRANSACTION ISOLATION LEVEL READ COMMITTED; BEGIN TRANSACTION; \
         DECLARE @query nvarchar(max) = CONVERT(nvarchar(max), 0x{hex}); \
         DECLARE @error nvarchar(2048) = (SELECT TOP (1) LEFT(error_message, 2048) \
         FROM sys.dm_exec_describe_first_result_set(@query, NULL, 0) WHERE error_message IS NOT NULL); \
         IF @error IS NOT NULL THROW 50000, @error, 1; \
         DECLARE @columns TABLE (n int PRIMARY KEY, name sysname NULL, type nvarchar(256) NOT NULL, clr bit NOT NULL); \
         INSERT INTO @columns (n, name, type, clr) SELECT column_ordinal, name, CASE system_type_id \
         WHEN 189 THEN N'binary(8)' WHEN 35 THEN N'varchar(max)' WHEN 99 THEN N'nvarchar(max)' \
         WHEN 34 THEN N'varbinary(max)' ELSE ISNULL(system_type_name, N'sql_variant') END, IIF(system_type_id = 240, 1, 0) \
         FROM sys.dm_exec_describe_first_result_set(@query, NULL, 0) WHERE is_hidden = 0; \
         CREATE TABLE #rows ([#] int IDENTITY(1, 1) PRIMARY KEY); \
         DECLARE @add nvarchar(max) = (SELECT STRING_AGG(CONVERT(nvarchar(max), {column}) + N' ' + type + N' NULL', N', ') \
         WITHIN GROUP (ORDER BY n) FROM @columns); \
         DECLARE @list nvarchar(max) = (SELECT STRING_AGG(CONVERT(nvarchar(max), {column}), N', ') \
         WITHIN GROUP (ORDER BY n) FROM @columns); \
         DECLARE @shown nvarchar(max) = (SELECT STRING_AGG(CONVERT(nvarchar(max), \
         IIF(clr = 1, {column} + N'.ToString() AS ' + {column}, {column})), N', ') WITHIN GROUP (ORDER BY n) FROM @columns); \
         EXEC (N'ALTER TABLE #rows ADD ' + @add); \
         SELECT '{COLUMNS_LINE}' + CONVERT(varchar(max), CONVERT(varbinary(max), \
         (SELECT name FROM @columns ORDER BY n FOR JSON PATH, INCLUDE_NULL_VALUES)), 2); \
         DECLARE @insert nvarchar(max) = N'INSERT INTO #rows (' + @list + N') EXEC (@query);'; \
         SET ROWCOUNT {}; EXEC sp_executesql @insert, N'@query nvarchar(max)', @query; SET ROWCOUNT 0; \
         DECLARE @select nvarchar(max) = N'SELECT ''{ROW_LINE}'' + CONVERT(varchar(max), CONVERT(varbinary(max), \
         (SELECT ' + @shown + N' FOR JSON PATH, WITHOUT_ARRAY_WRAPPER, INCLUDE_NULL_VALUES)), 2) FROM #rows ORDER BY [#];'; \
         EXEC (@select); ROLLBACK TRANSACTION; SELECT '{END_LINE}';",
        rows + 1
    )
}

/// What the batch wrote, as the query's columns and at most `rows` rows - each text value cut at [`CELL_CHARS`]; why
/// not when it didn't finish.
pub fn read(lines: &[String], rows: usize) -> Result<Read, String> {
    let mut columns = None;
    let mut found: Vec<Value> = Vec::new();
    let mut ended = false;
    for line in lines.iter().map(|line| line.trim()) {
        if line == END_LINE {
            ended = true;
            continue;
        }
        let Some(kind) = line.chars().next().filter(|kind| [COLUMNS_LINE, ROW_LINE].contains(kind)) else { continue };
        let hex = &line[kind.len_utf8()..];
        if hex.is_empty() || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            continue;
        }
        let json: Value =
            unhex(hex).and_then(|text| serde_json::from_str(&text).ok()).ok_or_else(|| TOO_LONG.to_string())?;
        if kind == COLUMNS_LINE {
            columns = Some(names(&json));
        } else {
            found.push(json);
        }
    }
    if !ended {
        if lines.iter().any(|line| line.contains(TIMED_OUT)) {
            return Err(format!("the query ran over {SECONDS} s and was stopped - narrow it (TOP, WHERE)"));
        }
        return Err("the query didn't finish (sqlcmd said nothing more)".into());
    }
    let columns = columns.ok_or_else(|| "the query's columns didn't come back".to_string())?;
    let more = found.len() > rows;
    found.truncate(rows);
    let mut cut = 0;
    let rows = found
        .iter()
        .map(|row| (1..=columns.len()).map(|n| shorter(row[format!("c{n}")].clone(), &mut cut)).collect())
        .collect();
    Ok(Read { columns, rows, more, cut, ms: 0 })
}

/// UTF-16 text from its hex (little-endian, as SQL Server keeps nvarchar).
fn unhex(hex: &str) -> Option<String> {
    if !hex.len().is_multiple_of(4) {
        return None;
    }
    let byte = |at: usize| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok();
    let units: Option<Vec<u16>> =
        (0..hex.len()).step_by(4).map(|at| Some(u16::from_le_bytes([byte(at)?, byte(at + 2)?]))).collect();
    String::from_utf16(&units?).ok()
}

/// The columns' names, from `[{"name": ...}, ...]`.
fn names(json: &Value) -> Vec<Option<String>> {
    json.as_array().into_iter().flatten().map(|column| column["name"].as_str().map(str::to_string)).collect()
}

/// A value, a text one cut at [`CELL_CHARS`] (counted in `cut`).
fn shorter(value: Value, cut: &mut usize) -> Value {
    match value {
        Value::String(text) if text.chars().count() > CELL_CHARS => {
            *cut += 1;
            Value::String(format!("{}…", text.chars().take(CELL_CHARS).collect::<String>()))
        }
        other => other,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::json;

    use super::*;

    /// A line as the batch writes it: its kind, then the text as UTF-16 hex.
    pub(crate) fn line(kind: char, text: &str) -> String {
        let hex: String = text.encode_utf16().flat_map(u16::to_le_bytes).map(|byte| format!("{byte:02X}")).collect();
        format!("{kind}{hex}")
    }

    #[test]
    fn a_select_or_a_with_ending_in_one_is_let_through() {
        let fine = [
            "SELECT TOP 5 * FROM dbo.Orders ORDER BY Id DESC",
            "select count(*) from Orders;",
            "  ;WITH recent AS (SELECT * FROM Orders WHERE Placed > '2026-01-01') SELECT * FROM recent;  \n",
            "SELECT [Update], [Set], \"Into\" FROM [Delete Me] WHERE Notes = 'please DELETE; DROP TABLE x' -- EXEC\n",
            "SELECT 1 /* INSERT /* nested */ UPDATE */ AS one",
            "SELECT N'it''s; fine', 'x' + ']' FROM Orders WITH (NOLOCK)",
            "(SELECT Id FROM Orders) UNION ALL (SELECT Id FROM Archive)",
            "SELECT o.Id, (SELECT COUNT(*) FROM Lines l WHERE l.OrderId = o.Id FOR JSON PATH) FROM Orders o",
            "SELECT Description, Updated, Settings, Executed, Created_At, @@ROWCOUNT FROM Orders",
        ];
        for sql in fine {
            assert_eq!(check(sql), Ok(()), "{sql}");
        }
    }

    #[test]
    fn anything_that_writes_runs_something_else_or_is_two_statements_is_refused_with_why() {
        let refused = [
            ("UPDATE Orders SET Total = 0", "it isn't a SELECT"),
            ("DELETE FROM Orders", "it isn't a SELECT"),
            ("exec sp_who", "it isn't a SELECT"),
            ("SELECT * INTO Copy FROM Orders", "SELECT ... INTO makes a table"),
            ("SELECT 1; DELETE FROM Orders", "something follows a ;"),
            ("SELECT 1; SELECT 2", "something follows a ;"),
            ("WITH x AS (SELECT Id FROM Orders) DELETE FROM x", "DELETE isn't allowed"),
            ("SELECT 1 exec('DROP TABLE Orders')", "EXEC isn't allowed"),
            ("SELECT * FROM OPENROWSET('SQLNCLI', 'x', 'SELECT 1')", "OPENROWSET isn't allowed"),
            ("SELECT 1 COMMIT", "COMMIT isn't allowed"),
            ("SELECT 1 WAITFOR DELAY '00:01'", "WAITFOR isn't allowed"),
            ("SELECT 1 AS a ENABLE TRIGGER t ON Orders", "ENABLE TRIGGER changes a table"),
            ("SELECT NEXT VALUE FOR dbo.OrderNumbers", "NEXT VALUE FOR moves a sequence on"),
            ("SELECT * FROM master..xp_cmdshell", "it calls a procedure (xp_cmdshell)"),
            ("SELECT * FROM sys.sp_who2", "it calls a procedure (sp_who2)"),
            ("SELECT 1 FROM Orders WHERE Notes = 'unclosed", "isn't closed"),
            ("SELECT 1 /* open", "isn't closed"),
            ("   ", "there's no SQL"),
            ("-- SELECT 1", "it isn't a SELECT"),
            ("SET ROWCOUNT 0 SELECT 1", "it isn't a SELECT"),
            ("1 SELECT 2", "it isn't a SELECT"),
        ];
        for (sql, why) in refused {
            let said = check(sql).unwrap_err();
            assert!(said.contains(why), "{sql}: {said}");
            assert!(said.starts_with("refused before it ran: "), "{said}");
        }
        assert!(check(&format!("SELECT '{}'", "x".repeat(MOST_CHARS))).unwrap_err().contains("over 4000 characters"));
    }

    #[test]
    fn the_batch_runs_the_query_from_hex_in_a_transaction_it_rolls_back_one_row_past_the_cap() {
        let sql = "SELECT N'é' AS [GO], '$(x)'\n:!! dir";
        let batch = batch(sql, 100);
        let hex: String = sql.encode_utf16().flat_map(u16::to_le_bytes).map(|byte| format!("{byte:02X}")).collect();
        assert!(batch.contains(&format!("CONVERT(nvarchar(max), 0x{hex})")), "the text reaches the server as hex");
        assert!(!batch.contains("$(") && !batch.contains(":!!") && !batch.contains('\n'), "sqlcmd never reads it");
        let order = [
            "SET XACT_ABORT ON",
            "SET LOCK_TIMEOUT 5000",
            "BEGIN TRANSACTION",
            "SET ROWCOUNT 101;",
            "EXEC sp_executesql @insert",
            "SET ROWCOUNT 0",
            "ROLLBACK TRANSACTION",
            "SELECT 'E';",
        ];
        let at: Vec<usize> = order.iter().map(|part| batch.find(part).unwrap_or_else(|| panic!("{part}"))).collect();
        assert!(at.windows(2).all(|pair| pair[0] < pair[1]), "{at:?}");
        assert!(batch.ends_with("ROLLBACK TRANSACTION; SELECT 'E';"));
    }

    #[test]
    fn the_rows_read_as_json_values_capped_and_cut() {
        let long = "x".repeat(CELL_CHARS + 20);
        let lines = vec![
            line('N', r#"[{"name":"Id"},{"name":"Customer"},{"name":null}]"#),
            line('R', r#"{"c1":1,"c2":"Ana | \"A\"\r\nČ 🦀","c3":12.50}"#),
            line('R', &format!(r#"{{"c1":2,"c2":"{long}","c3":null}}"#)),
            line('R', r#"{"c1":3,"c2":"Cy","c3":true}"#),
            "Warning: Null value is eliminated by an aggregate or other SET operation.".to_string(),
            "E".to_string(),
        ];
        let read = read(&lines, 2).unwrap();
        assert_eq!(read.columns, [Some("Id".to_string()), Some("Customer".to_string()), None]);
        assert_eq!(read.rows[0], [json!(1), json!("Ana | \"A\"\r\nČ 🦀"), json!(12.5)]);
        assert_eq!(read.rows[1][1].as_str().unwrap().chars().count(), CELL_CHARS + 1, "cut, with …");
        assert_eq!((read.rows.len(), read.more, read.cut), (2, true, 1));
        let all = super::read(&lines, 3).unwrap();
        assert_eq!((all.rows.len(), all.more), (3, false));
    }

    #[test]
    fn a_query_that_didnt_finish_or_came_back_broken_says_so() {
        let columns = line('N', r#"[{"name":"Id"}]"#);
        let timed_out = read(&[columns.clone(), "Timeout expired".to_string()], 10).unwrap_err();
        assert!(timed_out.contains("ran over 30 s"), "{timed_out}");
        assert!(read(std::slice::from_ref(&columns), 10).unwrap_err().contains("didn't finish"));
        let broken = [columns, "R7B002200".to_string(), "E".to_string()];
        assert!(read(&broken, 10).unwrap_err().contains("didn't come back whole"));
        let none = read(&["E".to_string()], 10).unwrap_err();
        assert!(none.contains("columns didn't come back"), "{none}");
    }
}
